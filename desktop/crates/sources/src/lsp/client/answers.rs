//! What a server says, as the feed's rows: servers, problems and the answers to questions,
//! every position in character columns counted from 1.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use yd_core::row::{Dot, Row, Tone, Value as Cell};

use super::edit::{self, EditError};
use super::server::Doc;
use super::{Language, Spot};
use crate::fs::MAX_TEXT;
use crate::lsp::protocol::{char_of, path_of, position};

/// A server capability and the words the pane shows for it.
const CAN_DO: &[(&str, &str)] = &[
    ("hoverProvider", "Hover"),
    ("definitionProvider", "Go to definition"),
    ("referencesProvider", "Find references"),
    ("renameProvider", "Rename"),
    ("documentFormattingProvider", "Format"),
    ("inlayHintProvider", "Type hints"),
    ("codeActionProvider", "Quick fixes"),
];

/// Most references one answer turns into rows.
pub const MAX_REFS: usize = 500;
/// Most problems or type hints one answer turns into rows for one file.
pub const MAX_PER_FILE: usize = 1000;
/// Most files one answer reads for line text; past it a row's text is empty.
const MAX_FILES: usize = 64;

/// What a server has answered since it started.
#[derive(Debug, Default)]
pub struct Stats {
    pub can: Vec<&'static str>,
    pub answered: u64,
    pub slowest: Duration,
    pub late: u64,
    /// The last capped answer's notice: what it kept of what it held.
    pub cut: Option<String>,
}

/// Where a server stands, as its row shows it.
pub enum Shown<'a> {
    Missing,
    Starting,
    Ready(&'a Stats),
    Failed(&'a str, &'a Stats),
}

pub fn server_row(lang: &Language, root: &Path, shown: &Shown) -> Row {
    let project = root.file_name().map(|n| n.to_string_lossy().into_owned());
    let (dot, status, can, actions): (Dot, String, &[&str], &[&str]) = match shown {
        Shown::Missing => (
            Dot::Stop,
            format!("not installed · {} was not found on PATH", lang.program),
            &[],
            &["Settings", "Attach"],
        ),
        Shown::Starting => (
            Dot::Work,
            "starting".to_owned(),
            &[],
            &["Restart", "Settings", "Attach"],
        ),
        Shown::Ready(stats) => (
            Dot::Run,
            format!("ready · {}", answered(stats)),
            &stats.can,
            &["Restart", "Logs", "Settings", "Attach"],
        ),
        Shown::Failed(why, stats) => (
            Dot::Fail,
            format!("error · {why}"),
            &stats.can,
            &["Restart", "Settings", "Attach"],
        ),
    };
    let mut row = Row::new(format!("server:{}", lang.server))
        .cell("state", Cell::Dot(dot))
        .text("server", lang.server)
        .text("language", lang.language)
        .text("project", project.unwrap_or_default())
        .text("memory", "–")
        .text("status", status)
        .cell(
            "can_do",
            Cell::Tags(can.iter().map(|c| (*c).to_owned()).collect()),
        )
        .cell("agent_may_ask", Cell::Flag(false))
        .cell(
            "actions",
            Cell::Tags(actions.iter().map(|a| (*a).to_owned()).collect()),
        );
    row.dim = matches!(shown, Shown::Missing);
    row
}

fn answered(stats: &Stats) -> String {
    let n = stats.answered;
    let mut text = format!("answered {n} request{}", if n == 1 { "" } else { "s" });
    if n > 0 {
        text.push_str(&format!(", slowest {} ms", stats.slowest.as_millis()));
    }
    if stats.late > 0 {
        text.push_str(&format!(", {} timed out", stats.late));
    }
    if let Some(cut) = &stats.cut {
        text.push_str(&format!(" · {cut}"));
    }
    text
}

/// What an `initialize` answer says the server can do; problems are always pushed.
pub fn can_do(result: &Value) -> Vec<&'static str> {
    let caps = result.get("capabilities");
    let has = |key: &str| {
        caps.and_then(|c| c.get(key))
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
    };
    let named = CAN_DO
        .iter()
        .filter(|(key, _)| has(key))
        .map(|(_, name)| *name);
    std::iter::once("Problems").chain(named).collect()
}

/// The text of the files answers point into: an open file's as sent, else as on disk.
pub struct Texts<'a> {
    root: &'a Path,
    docs: &'a BTreeMap<String, Doc>,
    read: BTreeMap<PathBuf, Option<String>>,
    pub cut: Option<String>,
}

impl<'a> Texts<'a> {
    pub fn new(root: &'a Path, docs: &'a BTreeMap<String, Doc>) -> Self {
        Self {
            root,
            docs,
            read: BTreeMap::new(),
            cut: None,
        }
    }

    /// The first `cap` of `list`, naming the cut in [`Texts::cut`] when there were more.
    fn take<'v>(&mut self, list: &'v [Value], cap: usize, what: &str) -> &'v [Value] {
        if list.len() > cap {
            self.cut = Some(format!(
                "[…] last answer kept {cap} of {} {what} (cap {cap}); the rest are not shown",
                list.len()
            ));
        }
        list.get(..cap).unwrap_or(list)
    }

    pub fn text(&mut self, abs: &Path) -> Option<&str> {
        let docs = self.docs;
        if let Some(doc) = docs.get(&rel_of(self.root, abs)) {
            return Some(&doc.text);
        }
        if !self.read.contains_key(abs) && self.read.len() >= MAX_FILES {
            return None;
        }
        let read = self
            .read
            .entry(abs.to_path_buf())
            .or_insert_with(|| read_text(abs));
        read.as_deref()
    }

    fn line(&mut self, abs: &Path, n: u64) -> String {
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        let line = self.text(abs).and_then(|text| text.lines().nth(n));
        line.unwrap_or_default().to_owned()
    }
}

/// A file's text when it is UTF-8 and at most [`MAX_TEXT`] bytes.
pub fn read_text(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).ok()?;
    let cap = u64::try_from(MAX_TEXT)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    file.take(cap).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_TEXT {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// `abs` under `root` as the rows name it, else whole.
pub fn rel_of(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .to_string_lossy()
        .into_owned()
}

/// A range in a file, in character columns from 1, with the text of its first line.
struct Place {
    path: String,
    line: usize,
    col: usize,
    end_col: usize,
    text: String,
}

fn place(texts: &mut Texts, uri: &str, range: &Value) -> Option<Place> {
    let abs = path_of(uri)?;
    let (line, start) = position(range.get("start")?)?;
    let (end_line, end) = position(range.get("end")?)?;
    let text = texts.line(&abs, line);
    let col = char_of(&text, start);
    let end_col = if end_line == line {
        char_of(&text, end)
    } else {
        text.chars().count()
    };
    Some(Place {
        path: rel_of(texts.root, &abs),
        line: usize::try_from(line).ok()?.saturating_add(1),
        col: col.saturating_add(1),
        end_col: end_col.saturating_add(1),
        text,
    })
}

impl Place {
    fn cells(self, row: Row) -> Row {
        row.text("path", self.path)
            .text("line", self.line.to_string())
            .text("col", self.col.to_string())
            .text("end_col", self.end_col.to_string())
    }
}

/// The uri and range of each location an answer holds: one, a list, or links.
fn locations(result: &Value) -> Vec<(&str, &Value)> {
    match result {
        Value::Array(list) => list.iter().filter_map(location).collect(),
        Value::Object(_) => location(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn location(v: &Value) -> Option<(&str, &Value)> {
    match v.get("targetUri") {
        Some(uri) => Some((uri.as_str()?, v.get("targetSelectionRange")?)),
        None => Some((v.get("uri")?.as_str()?, v.get("range")?)),
    }
}

/// Hover contents as Markdown: marked strings, markup content, or a list of either.
fn markdown(contents: &Value) -> String {
    match contents {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(markdown).collect::<Vec<_>>().join("\n\n"),
        Value::Object(part) => {
            let value = part
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match part.get("language").and_then(Value::as_str) {
                Some(language) => format!("```{language}\n{value}\n```"),
                None => value.to_owned(),
            }
        }
        _ => String::new(),
    }
}

pub fn hover(spot: &Spot, result: &Value) -> Vec<Row> {
    let text = result.get("contents").map(markdown).unwrap_or_default();
    if text.trim().is_empty() {
        return Vec::new();
    }
    vec![Row::new(format!("hover:{}", spot.id())).text("text", text)]
}

pub fn definition(spot: &Spot, result: &Value, texts: &mut Texts) -> Vec<Row> {
    let first = locations(result)
        .into_iter()
        .find_map(|(uri, range)| place(texts, uri, range));
    let Some(at) = first else {
        return Vec::new();
    };
    let row = Row::new(format!("def:{}", spot.id())).text("path", at.path);
    vec![
        row.text("line", at.line.to_string())
            .text("col", at.col.to_string()),
    ]
}

pub fn references(spot: &Spot, result: &Value, texts: &mut Texts) -> Vec<Row> {
    let all = match result {
        Value::Array(list) => texts.take(list, MAX_REFS, "references"),
        _ => std::slice::from_ref(result),
    };
    let places = all
        .iter()
        .filter_map(location)
        .filter_map(|(uri, range)| place(texts, uri, range));
    places
        .enumerate()
        .map(|(n, at)| {
            let row = Row::new(format!("ref:{}:{n}", spot.id()));
            let line = at.text.clone();
            at.cells(row).text("text", line).text("kind", "read")
        })
        .collect()
}

pub fn hints(path: &str, abs: &Path, result: &Value, texts: &mut Texts) -> Vec<Row> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let all = result.as_array().map(Vec::as_slice).unwrap_or_default();
    for hint in texts.take(all, MAX_PER_FILE, "type hints") {
        let Some((line, unit)) = hint.get("position").and_then(position) else {
            continue;
        };
        let label = match hint.get("label") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("value").and_then(Value::as_str))
                .collect(),
            _ => continue,
        };
        let col = char_of(&texts.line(abs, line), unit).saturating_add(1);
        let id = format!("hint:{path}:{}:{col}", line.saturating_add(1));
        let text = found.entry(id).or_default();
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&label);
    }
    found
        .into_iter()
        .map(|(id, text)| Row::new(id).text("text", text))
        .collect()
}

/// A severity's words and colour role.
fn severity(diagnostic: &Value) -> (&'static str, Tone) {
    match diagnostic.get("severity").and_then(Value::as_u64) {
        Some(2) => ("Warn", Tone::Warn),
        Some(3) => ("Info", Tone::Dim),
        Some(4) => ("Hint", Tone::Dim),
        _ => ("Error", Tone::Bad),
    }
}

pub fn problems(server: &str, uri: &str, list: &[Value], texts: &mut Texts) -> Vec<Row> {
    let mut on_line: BTreeMap<usize, usize> = BTreeMap::new();
    let mut rows = Vec::new();
    for diagnostic in texts.take(list, MAX_PER_FILE, "problems") {
        let Some(at) = diagnostic.get("range").and_then(|r| place(texts, uri, r)) else {
            continue;
        };
        let n = on_line.entry(at.line).or_default();
        let id = format!("problem:{}:{}:{n}", at.path, at.line);
        *n = n.saturating_add(1);
        let (words, tone) = severity(diagnostic);
        let message = diagnostic
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let actions = ["Quick fix", "Ask agent", "Attach", "Ignore here"];
        let row = Row::new(id)
            .cell("severity", Cell::Badge(words.to_owned(), tone))
            .text("message", message)
            .text("where", format!("{}:{}", at.path, at.line))
            .text("from", server)
            .cell("actions", Cell::Tags(actions.map(str::to_owned).to_vec()));
        rows.push(at.cells(row));
    }
    rows
}

/// The code actions that carry an edit, as rows, each with its edit and the diff it shows.
pub fn fixes(
    spot: &Spot,
    result: &Value,
    root: &Path,
    texts: &mut Texts,
) -> Vec<(Row, Value, String)> {
    let mut out = Vec::new();
    for action in result.as_array().into_iter().flatten() {
        let (Some(title), Some(change)) = (
            action.get("title").and_then(Value::as_str),
            action.get("edit"),
        ) else {
            continue;
        };
        let Ok(diff) = preview(root, change, |path| texts.text(path).map(str::to_owned)) else {
            continue;
        };
        let id = format!("fix:{}:{}:{}", spot.path, spot.line, out.len());
        let row = Row::new(id).text("title", title).text("edit", diff.clone());
        out.push((row, change.clone(), diff));
    }
    out
}

/// The unified diff of `change` over the texts `read` gives.
pub fn preview(
    root: &Path,
    change: &Value,
    mut read: impl FnMut(&Path) -> Option<String>,
) -> Result<String, EditError> {
    let mut diff = String::new();
    for (path, edits) in edit::files(root, change)? {
        let old = read(&path).ok_or(EditError::Shape)?;
        let new = edit::applied(&old, &edits)?;
        diff.push_str(&edit::diff(&rel_of(root, &path), &old, &new));
    }
    Ok(diff)
}

/// Applies a fix whose files still read as they did when it was offered; the paths written.
pub fn write_fix(root: &Path, change: &Value, shown: &str) -> Result<Vec<PathBuf>, EditError> {
    let now = preview(root, change, read_text)?;
    if now != shown {
        return Err(EditError::Stale);
    }
    let mut written = Vec::new();
    let mut texts = Vec::new();
    for (path, edits) in edit::files(root, change)? {
        let old = read_text(&path).ok_or(EditError::Shape)?;
        texts.push((path, edit::applied(&old, &edits)?));
    }
    for (path, text) in texts {
        edit::write_atomic(&path, &text)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;

    fn spot() -> Spot {
        Spot {
            path: "a.rs".to_owned(),
            line: 1,
            col: 1,
        }
    }

    #[test]
    fn references_past_the_cap_are_cut_and_the_cut_is_named() -> Result<(), Box<dyn Error>> {
        let root = crate::fs::tests::scratch("lsp-refs-cap")?;
        let range =
            json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
        let uri = format!("file://{}/a.rs", root.display());
        let all: Vec<Value> = (0..=MAX_REFS)
            .map(|_| json!({"uri": uri, "range": range}))
            .collect();
        let docs = BTreeMap::new();
        let mut texts = Texts::new(&root, &docs);
        let rows = references(&spot(), &Value::Array(all), &mut texts);
        assert_eq!(rows.len(), MAX_REFS);
        let cut = texts.cut.ok_or("no cut named")?;
        assert!(cut.contains("500 of 501 references"), "{cut}");
        Ok(())
    }

    #[test]
    fn an_answer_reads_at_most_its_cap_of_files_for_line_text() -> Result<(), Box<dyn Error>> {
        let root = crate::fs::tests::scratch("lsp-texts-cap")?;
        for n in 0..=MAX_FILES {
            std::fs::write(root.join(format!("{n}.rs")), "line")?;
        }
        let docs = BTreeMap::new();
        let mut texts = Texts::new(&root, &docs);
        let read = (0..=MAX_FILES)
            .filter(|n| texts.text(&root.join(format!("{n}.rs"))).is_some())
            .count();
        assert_eq!(read, MAX_FILES);
        Ok(())
    }
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::process::{OUTPUT_CAP, command, run_captured};
use crate::tool::{Tool, ToolContext, ToolKind, ToolOutput, text_output};

const LAYER_CAP: usize = 4_000;
/// The skeleton footer's longest form, held back from the layer so the footer always fits.
const FOOTER_ROOM: usize = 200;
const SKELETON_FILES: usize = 40;
const SKELETON_LINES: usize = 12;
const SKELETON_SCAN: usize = 2_000;
const HEAT_ROWS: usize = 15;
const HEAT_COMMITS: usize = 200;
const ISSUE_ROWS: usize = 10;
const ISSUES_PATH: &str = ".yi/mining/issues.jsonl";

pub(crate) const SKELETON_EXTS: [&str; 6] = ["rs", "py", "ts", "tsx", "js", "go"];

const DECL_HEADS: [&str; 15] = [
    "pub ",
    "fn ",
    "async ",
    "struct ",
    "enum ",
    "trait ",
    "impl ",
    "type ",
    "const ",
    "static ",
    "mod ",
    "macro_rules!",
    "class ",
    "def ",
    "export ",
];

const GATES: [(&str, &str); 6] = [
    ("justfile", "just check"),
    ("Justfile", "just check"),
    ("Makefile", "make"),
    ("Cargo.toml", "cargo test"),
    ("package.json", "npm test"),
    ("pyproject.toml", "pytest"),
];

/// Invariant: an absent layer is named, never dropped — Err carries the reason.
type LayerBody = Result<String, String>;

pub struct GetContextTool;

impl Tool for GetContextTool {
    fn name(&self) -> &str {
        "get_context"
    }

    fn description(&self) -> &str {
        "One orientation packet for the working directory: grid roots, symbol neighborhood, file skeletons, git change heat, gate commands, prior mining issues. Layers are clamped and name what they cut; the header says how many were available. Call it once, first, in a repository you have not read this session. PARTIAL - k of n layers means the missing layers are absent, not empty; read what the packet names."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "Optional callsign or type name for the symbol-neighborhood layer"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let symbol = input.get("symbol").and_then(Value::as_str);
        text_output(packet(symbol, context))
    }
}

fn packet(symbol: Option<&str>, context: &ToolContext) -> String {
    let root = context.cwd.as_path();
    // The grid calls wait on another process, so they run beside the heat and the walk.
    let (roots, near, heat, skeleton) = std::thread::scope(|scope| {
        let roots = scope.spawn(|| {
            let _span = yi_types::trace::span("context.grid");
            grid(context, &["roots"])
        });
        let near = scope.spawn(|| {
            let _span = yi_types::trace::span("context.grid");
            neighborhood(symbol, context)
        });
        let heat_span = yi_types::trace::span("context.heat");
        let heat = change_heat(context);
        drop(heat_span);
        let _span = yi_types::trace::span("context.skeletons");
        let skeleton = skeletons(root, symbol, heat.as_ref().ok(), context);
        (joined(roots), joined(near), heat, skeleton)
    });
    let layers: [(&str, LayerBody); 6] = [
        // Invariant: this tool only reads. `grid survey` charts the worktree,
        // so the packet takes a slice of an existing chart instead.
        ("grid roots", roots),
        ("symbol neighborhood", near),
        ("file skeletons", skeleton),
        ("git change heat", git_heat(heat)),
        ("gate commands", gates(root)),
        ("prior issues", issues(root, context)),
    ];
    let present = layers.iter().filter(|(_, body)| body.is_ok()).count();
    let total = layers.len();
    let mut out = String::from("# orientation packet\n");
    if present == total {
        out.push_str("COMPLETE\n");
    } else {
        out.push_str(&format!("PARTIAL - {present} of {total} layers\n"));
    }
    for (name, body) in layers {
        out.push_str(&format!("\n## {name}\n"));
        match body {
            Ok(text) => out.push_str(&clamp(name, text)),
            Err(reason) => out.push_str(&format!("absent: {reason}")),
        }
        out.push('\n');
    }
    out
}

fn joined(layer: std::thread::ScopedJoinHandle<'_, LayerBody>) -> LayerBody {
    layer
        .join()
        .unwrap_or_else(|_| Err("the layer's thread panicked".to_owned()))
}

fn clamp(name: &str, body: String) -> String {
    if body.len() <= LAYER_CAP {
        return body;
    }
    let mut end = body.floor_char_boundary(LAYER_CAP);
    // A cut mid-row reads as a row; the last whole one is where the layer ends.
    if let Some(row_end) = body.get(..end).and_then(|kept| kept.rfind('\n')) {
        end = row_end;
    }
    let kept = body.get(..end).unwrap_or_default();
    format!("{kept}\n[{name} truncated at {LAYER_CAP} bytes]")
}

fn grid(context: &ToolContext, args: &[&str]) -> LayerBody {
    let mut spawn = command("grid");
    spawn.args(args).current_dir(&context.cwd);
    let capture = run_captured(spawn, None, &context.cancelled, LAYER_CAP)
        .map_err(|error| format!("grid binary not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "grid {} exited {:?}: {}",
            args.join(" "),
            capture.exit_code,
            capture.stderr.lines().next().unwrap_or_default()
        ));
    }
    let stdout = capture.stdout.trim_end().to_owned();
    if stdout.is_empty() {
        return Err(format!("grid {} answered nothing", args.join(" ")));
    }
    Ok(stdout)
}

fn neighborhood(symbol: Option<&str>, context: &ToolContext) -> LayerBody {
    let symbol = symbol.ok_or_else(|| "no symbol argument was given".to_owned())?;
    grid(context, &["scope", symbol, "--depth", "1"])
}

fn skeletons(
    root: &Path,
    symbol: Option<&str>,
    heat: Option<&Heat>,
    context: &ToolContext,
) -> LayerBody {
    let mut files: Vec<PathBuf> = Vec::new();
    let walled = crate::builtins::walk_files(root, &context.deny_read, &mut |path| {
        let interesting = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| SKELETON_EXTS.contains(&ext));
        if interesting {
            files.push(path.to_path_buf());
        }
        files.len() < SKELETON_SCAN
    });
    if files.is_empty() {
        let rows = ["no source files under the working directory".to_owned()];
        return Err(rows
            .into_iter()
            .chain(walled.notices())
            .collect::<Vec<_>>()
            .join("\n"));
    }
    // Incident: name order spent the whole layer on the alphabetically first
    // crate, so a symbol's own file was never shown.
    let mut ranked: Vec<(u8, usize, String)> = files
        .iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            // ponytail: heat keys are repo-root relative, so from a subdirectory
            // cwd nothing matches; the order line then reports 0 with heat.
            let hot = heat.and_then(|heat| heat.counts.get(&name)).copied();
            (symbol_rank(path, symbol), hot.unwrap_or(0), name)
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    let total = ranked.len();
    let mut out = order_line(&ranked, symbol);
    let mut shown = 0_usize;
    let mut cap = format!("the {SKELETON_FILES}-file cap");
    for (_, _, name) in ranked.iter().take(SKELETON_FILES) {
        let mut block = format!("{name}\n");
        let text = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        for line in skeleton(&text, SKELETON_LINES).0 {
            block.push_str(&format!("  {line}\n"));
        }
        // Whole files only: the layer clamp cuts from the end, and this footer is what it cut.
        if out.len().saturating_add(block.len()) > LAYER_CAP.saturating_sub(FOOTER_ROOM) {
            cap = format!("the {LAYER_CAP}-byte layer budget");
            break;
        }
        out.push_str(&block);
        shown = shown.saturating_add(1);
    }
    if shown < total {
        out.push_str(&format!(
            "[skeletons truncated: {shown} of {total} files at {cap}; \
             read a directory for its skeletons, or pass symbol to rank its files first]\n"
        ));
    }
    for notice in walled.notices() {
        out.push_str(&notice);
    }
    Ok(out.trim_end().to_owned())
}

/// 2 defines the symbol, 1 only mentions it, 0 neither or no symbol given.
fn symbol_rank(path: &Path, symbol: Option<&str>) -> u8 {
    let Some(symbol) = symbol else { return 0 };
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    if text
        .lines()
        .any(|line| is_decl(line) && line.contains(symbol))
    {
        2
    } else {
        u8::from(text.contains(symbol))
    }
}

/// Invariant: printed first, because the layer clamp cuts from the end.
fn order_line(ranked: &[(u8, usize, String)], symbol: Option<&str>) -> String {
    let count =
        |keep: fn(&(u8, usize, String)) -> bool| ranked.iter().filter(|row| keep(row)).count();
    let hot = count(|row| row.1 > 0);
    match symbol {
        Some(symbol) => format!(
            "[order: {} defining `{symbol}`, {} mentioning it, {hot} with change heat, then name]\n",
            count(|row| row.0 == 2),
            count(|row| row.0 == 1),
        ),
        None => format!("[order: {hot} with change heat, then name]\n"),
    }
}

pub(crate) fn skeleton(text: &str, cap: usize) -> (Vec<String>, usize) {
    let mut heads: Vec<String> = text
        .lines()
        .filter(|line| DECL_HEADS.iter().any(|head| line.starts_with(head)))
        .map(decl_head)
        .collect();
    let total = heads.len();
    if total > cap {
        heads.truncate(cap);
        heads.push(format!(
            "[{cap} of {total} heads, cap {cap} per file — grep def=true for all]"
        ));
    }
    (heads, total)
}

/// Visibility, export and storage words that may open a declaration before its keyword.
const DECL_PREFIXES: &str =
    "pub crate super export default async unsafe extern static inline public private protected";
/// Words that introduce a named declaration; `const fn` and `static mut` chain two.
const DECL_KEYWORDS: &str = "fn struct enum union trait type const mut mod class def function \
    func interface typedef";
/// Words that open a statement, never a C `type name(` declaration.
const STATEMENT_HEADS: &str = "let var return await yield throw new if while for assert";

fn in_word_list(list: &str, word: &str) -> bool {
    list.split_whitespace().any(|entry| entry == word)
}

fn identifiers(text: &str) -> Vec<&str> {
    text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .collect()
}

/// The word after a declaration's keyword (a Go method's, past its receiver), past visibility
/// and storage words, or in C the word before the `(` of an unindented `type name(`.
pub(crate) fn defined_name(line: &str) -> Option<String> {
    let indented = line.starts_with(char::is_whitespace);
    let trimmed = line.trim_start();
    (trimmed.starts_with(|first: char| first.is_ascii_alphabetic())).then_some(())?;
    let method = (trimmed.strip_prefix("func (")).and_then(|rest| rest.split_once(')'));
    let trimmed = method.map_or_else(|| trimmed.to_owned(), |(_, name)| format!("func{name}"));
    let prefixed = |word: &&str| in_word_list(DECL_PREFIXES, word);
    let all = identifiers(&trimmed);
    let mut rest = all.iter().copied().skip_while(prefixed).peekable();
    let head = *rest.peek()?;
    if in_word_list(DECL_KEYWORDS, head) {
        return rest
            .find(|word| !in_word_list(DECL_KEYWORDS, word))
            .map(str::to_owned);
    }
    let (text, _) = trimmed.split_once('(')?;
    let before: Vec<&str> = identifiers(text).into_iter().skip_while(prefixed).collect();
    let typed = !indented
        && before.len() >= 2
        && !text.contains(['.', '='])
        && !in_word_list(STATEMENT_HEADS, head);
    typed
        .then(|| before.last().map(|word| (*word).to_owned()))
        .flatten()
}

pub(crate) fn decl_head(line: &str) -> String {
    line.trim_end().trim_end_matches('{').trim_end().to_owned()
}

/// A definition line at any indentation: a method inside an impl or a class counts.
pub(crate) fn is_decl(line: &str) -> bool {
    let trimmed = line.trim_start();
    DECL_HEADS.iter().any(|head| trimmed.starts_with(head))
}

struct Heat {
    counts: BTreeMap<String, usize>,
    truncated: bool,
}

fn change_heat(context: &ToolContext) -> Result<Heat, String> {
    let mut spawn = command("git");
    spawn
        .arg("-C")
        .arg(&context.cwd)
        .arg("log")
        .arg("--no-merges")
        .arg(format!("-{HEAT_COMMITS}"))
        .arg("--format=")
        .arg("--name-only");
    let capture = run_captured(spawn, None, &context.cancelled, OUTPUT_CAP)
        .map_err(|error| format!("git not runnable: {error}"))?;
    if capture.exit_code != Some(0) {
        return Err(format!(
            "git log exited {:?}: {}",
            capture.exit_code,
            capture.stderr.lines().next().unwrap_or_default()
        ));
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in capture.stdout.lines().filter(|line| !line.is_empty()) {
        let count = counts.entry(line.to_owned()).or_default();
        *count = count.saturating_add(1);
    }
    if counts.is_empty() {
        return Err("git log named no changed files".to_owned());
    }
    Ok(Heat {
        counts,
        truncated: capture.truncated,
    })
}

fn git_heat(heat: Result<Heat, String>) -> LayerBody {
    let heat = heat?;
    let total = heat.counts.len();
    // A tie on count would otherwise order by hash-map chance; the packet is
    // compared byte for byte across runs.
    let mut rows: Vec<(usize, &str)> = heat.counts.iter().map(|(p, c)| (*c, p.as_str())).collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let mut out = String::new();
    for (count, path) in rows.iter().take(HEAT_ROWS) {
        out.push_str(&format!("{count:>4}  {path}\n"));
    }
    if total > HEAT_ROWS {
        out.push_str(&format!("[heat truncated: {HEAT_ROWS} of {total} paths]\n"));
    }
    if heat.truncated {
        out.push_str("[git log output was capped before counting]\n");
    }
    Ok(out.trim_end().to_owned())
}

fn gates(root: &Path) -> LayerBody {
    let lines: Vec<String> = GATES
        .iter()
        .filter(|(file, _)| root.join(file).exists())
        .map(|(file, gate)| format!("{gate}  ({file})"))
        .collect();
    if lines.is_empty() {
        return Err("no gate manifest in the working directory".to_owned());
    }
    Ok(lines.join("\n"))
}

fn issues(root: &Path, context: &ToolContext) -> LayerBody {
    let path = root.join(ISSUES_PATH);
    if crate::builtins::walled(&context.deny_read, &path) {
        return Err(format!("{ISSUES_PATH} is denied to this agent"));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{ISSUES_PATH} unreadable: {error}"))?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let total = lines.len();
    let tail = lines.split_at(total.saturating_sub(ISSUE_ROWS)).1;
    let mut out = String::new();
    let mut unparsed = 0usize;
    for line in tail {
        match issue_row(line) {
            Some(row) => out.push_str(&format!("{row}\n")),
            None => unparsed = unparsed.saturating_add(1),
        }
    }
    if out.is_empty() {
        return Err(format!("{ISSUES_PATH} has no fingerprinted rows"));
    }
    if total > tail.len() {
        out.push_str(&format!(
            "[issues truncated: last {} of {total} rows]\n",
            tail.len()
        ));
    }
    if unparsed > 0 {
        out.push_str(&format!("[{unparsed} rows unparsed]\n"));
    }
    Ok(out.trim_end().to_owned())
}

fn issue_row(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let fingerprint = value.get("fingerprint")?.as_str()?;
    let text = ["title", "text", "summary"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or("(no text)");
    Some(format!("{fingerprint}  {text}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_clamped_layer_ends_on_a_whole_row() {
        let clamped = super::clamp("heat", "abcdef\n".repeat(1_000));
        let (kept, marker) = clamped.rsplit_once('\n').unwrap_or_default();
        assert_eq!(marker, "[heat truncated at 4000 bytes]");
        assert!(kept.lines().all(|row| row == "abcdef"), "{kept}");
    }
}

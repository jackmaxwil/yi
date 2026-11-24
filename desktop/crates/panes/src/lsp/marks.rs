//! Code marks from a language server: squiggles, hints, the hover card, the fix bar.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use rune::Tx;
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{LSP, Table};
use yd_core::ids::{CommandId, RowId};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::msg::Msg;
use yd_core::row::{Row, Rows, Tone, Value};
use yd_ui::code::doc::{Cols, Doc, LineNo};
use yd_ui::code::hover::Asked;
use yd_ui::code::marks::{Glyph, Marks};
use yd_ui::code::pointer::{CodeEvent, Token};
use yd_ui::theme::look::CODE_LINE;
use yd_ui::win::{Win, dispatch};

pub use surface::{bar, bar_line, card, server};
pub use yd_core::command::tools::lsp::{
    APPLY_FIX, DEFINITION, FIXES, HINTS, HOVER, OPEN, REFERENCES,
};

mod surface;
#[cfg(test)]
mod tests;

/// The file a code surface shows: where its keys start, its path under the working folder
/// and its text.
pub struct File<'a> {
    pub surface: &'a str,
    pub path: &'a str,
    pub doc: &'a Doc,
}

/// The token the pointer rests on, and how far along its line the card goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Resting {
    pub surface: String,
    pub path: String,
    pub line: LineNo,
    /// The token's first character, counted from 1 as the server counts.
    pub col: usize,
    pub shift: f32,
}

/// What this module remembers across frames.
#[derive(Debug, Default)]
pub struct Kept {
    pub resting: Option<Resting>,
    asked: Asked,
    /// The `def:` row of a go-to-definition still waited for.
    pub(super) pending: Option<RowId>,
}

/// A line's character column counted from 1, for the byte `at` of `line`.
fn char_col(line: &str, at: u32) -> usize {
    let before = usize::try_from(at).ok().and_then(|at| line.get(..at));
    before
        .map_or(0, |before| before.chars().count())
        .saturating_add(1)
}

/// The byte of `line` where its `n`th character (from 0) starts; its length past the end.
fn byte_of(line: &str, n: usize) -> usize {
    line.char_indices().nth(n).map_or(line.len(), |(at, _)| at)
}

fn number(row: &Row, field: &str) -> Option<usize> {
    row.words(field)?.parse().ok()
}

/// The path, line and column a `hint:` or `fix:` row's id carries: the last parts.
fn spot_of<'a>(id: &'a str, kind: &str) -> Option<(&'a str, usize, usize)> {
    let mut parts = id.strip_prefix(kind)?.rsplitn(3, ':');
    let last = parts.next()?.parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    Some((parts.next()?, line, last))
}

fn rows(table: &Table) -> impl Iterator<Item = &Row> {
    (0..table.len()).filter_map(|at| table.get(at))
}

fn lines_at(path: &str, line: usize, col: usize) -> Option<Address> {
    let address = Address::new(Kind::Lines, path).ok()?;
    Some(address.part(format!("L{line}:C{col}")))
}

/// A problem of `path`: its place as the server counts it, and what it says.
pub(super) struct Problem<'a> {
    pub row: &'a Row,
    pub line: usize,
    pub col: usize,
    pub end: usize,
}

impl<'a> Problem<'a> {
    pub fn words(&self) -> &'a str {
        self.row.words("severity").unwrap_or_default()
    }

    pub fn tone(&self) -> Tone {
        match self.row.get("severity") {
            Some(Value::Badge(_, tone)) => *tone,
            _ => Tone::Dim,
        }
    }
}

pub(super) fn problems<'a>(table: &'a Table, path: &str) -> impl Iterator<Item = Problem<'a>> {
    let wanted = path.to_owned();
    rows(table).filter_map(move |row| {
        let id = row.id.as_str().strip_prefix("problem:")?;
        let (found, _) = id.rsplit_once(':')?;
        let (found, line) = found.rsplit_once(':')?;
        let line = line.parse().ok()?;
        let col = number(row, "col")?;
        let end = number(row, "end_col").unwrap_or(col);
        (found == wanted).then_some(Problem {
            row,
            line,
            col,
            end,
        })
    })
}

/// The errors and warnings the server reports in `path`, as a header says them: `1 error`.
pub fn counted(model: &Model, path: &str) -> Vec<String> {
    let found = model
        .tables
        .get(LSP)
        .into_iter()
        .flat_map(|t| problems(t, path));
    let (mut errors, mut warnings) = (0usize, 0usize);
    for problem in found {
        match problem.tone() {
            Tone::Bad => errors = errors.saturating_add(1),
            Tone::Warn => warnings = warnings.saturating_add(1),
            _ => {}
        }
    }
    let said = [(errors, "error"), (warnings, "warning")].into_iter();
    let said = said.filter(|(n, _)| *n > 0);
    said.map(|(n, what)| format!("{n} {what}{}", if n == 1 { "" } else { "s" }))
        .collect()
}

/// Lines of `path` that have a fix row.
fn fixed(table: &Table, path: &str) -> BTreeSet<usize> {
    let spots = rows(table).filter_map(|row| spot_of(row.id.as_str(), "fix:"));
    spots
        .filter(|(found, _, _)| *found == path)
        .map(|(_, line, _)| line)
        .collect()
}

/// A problem's bytes in its line: a zero-width one takes the character it stands at, or the
/// last one when it stands at the end.
fn squiggle(text: &str, problem: &Problem<'_>) -> Option<Cols> {
    let start = byte_of(text, problem.col.saturating_sub(1));
    let end = byte_of(text, problem.end.saturating_sub(1));
    let (start, end) = if end > start {
        (start, end)
    } else if start < text.len() {
        (start, byte_of(text, problem.col))
    } else {
        (text.char_indices().next_back()?.0, text.len())
    };
    Some(Cols::new(
        u32::try_from(start).ok()?,
        u32::try_from(end).ok()?,
    ))
}

/// What the `lsp` feed puts on the file at `path`: a wavy line under each problem by its
/// severity, a fix glyph on lines with a fix, and the server's inline hints.
pub fn of(model: &Model, path: &str, doc: &Doc) -> Marks {
    onto(Marks::new(), model, path, doc)
}

/// [`of`], added to the marks a file already has.
// ponytail: reads the whole feed each view run; a per-file index when a file has thousands.
pub fn onto(mut marks: Marks, model: &Model, path: &str, doc: &Doc) -> Marks {
    let Some(table) = model.tables.get(LSP) else {
        return marks;
    };
    let fixes = fixed(table, path);
    let tone = |problem: &Problem<'_>| match problem.tone() {
        Tone::Bad => Tone::Bad,
        Tone::Warn => Tone::Need,
        _ => Tone::Dim,
    };
    for problem in problems(table, path) {
        let line = LineNo::from_index(problem.line.saturating_sub(1));
        let Some(cols) = doc.line(line).and_then(|text| squiggle(text, &problem)) else {
            continue;
        };
        marks = marks.squiggle(line, cols, tone(&problem));
        if fixes.contains(&problem.line) {
            marks = marks.glyph(line, Glyph::Fix);
        }
    }
    let hints = rows(table).filter_map(|row| Some((row, spot_of(row.id.as_str(), "hint:")?)));
    for (row, (_, line, col)) in hints.filter(|(_, (found, _, _))| *found == path) {
        let line = LineNo::from_index(line.saturating_sub(1));
        let (Some(text), Some(label)) = (doc.line(line), row.words("text")) else {
            continue;
        };
        let before = byte_of(text, col.saturating_sub(1));
        if let Ok(before) = u32::try_from(before) {
            marks = marks.hint(line, before, label);
        }
    }
    marks
}

/// Moves when a server starts, stops or fails: a hover asked before is asked again then.
fn epoch(model: &Model) -> u64 {
    let mut state = DefaultHasher::new();
    for row in model.tables.get(LSP).into_iter().flat_map(rows) {
        if row.id.as_str().starts_with("server:") {
            (&row.id, row.get("state").and_then(dot)).hash(&mut state);
        }
    }
    state.finish()
}

fn dot(value: &Value) -> Option<yd_core::row::Dot> {
    match value {
        Value::Dot(dot) => Some(*dot),
        _ => None,
    }
}

fn kept<R>(win: &Win, change: impl FnOnce(&mut Kept, u64) -> R) -> Option<R> {
    let mut hub = win.hub.try_borrow_mut().ok()?;
    let epoch = epoch(&hub.model);
    Some(change(hub.panes.get_mut::<Kept>(None)?, epoch))
}

pub(super) fn ask(win: &mut Win, command: CommandId, at: Address) {
    let run = Run::new(command).on(Target::Thing(at));
    dispatch(win, Msg::Command(Route::Source(LSP), run));
}

/// Runs `lsp.open` and `lsp.hints` for a file that has just opened, once for as long as the
/// servers and its text (`rev`) stay as they are.
pub fn open(win: &mut Win, path: &str, rev: u64) {
    let Some(at) = lines_at(path, 1, 1) else {
        return;
    };
    let first = kept(win, |kept, epoch| {
        kept.asked.once(&format!("open:{path}"), epoch ^ rev)
    });
    if first == Some(true) {
        ask(win, OPEN, at.clone());
        ask(win, HINTS, at);
    }
}

/// The token's first character as the server counts it.
fn spot(file: &File<'_>, token: &Token) -> Option<(usize, usize)> {
    let text = file.doc.line(token.line)?;
    Some((
        token.line.index().saturating_add(1),
        char_col(text, token.cols.start()),
    ))
}

/// The rest the pointer makes on `token`: where its card goes.
pub fn resting(file: &File<'_>, token: &Token) -> Option<Resting> {
    let (_, col) = spot(file, token)?;
    let text = file.doc.line(token.line)?;
    let before = text.get(..usize::try_from(token.cols.start()).ok()?)?;
    Some(Resting {
        surface: file.surface.to_owned(),
        path: file.path.to_owned(),
        line: token.line,
        col,
        shift: rune::text::advance(before, CODE_LINE),
    })
}

fn hover(win: &mut Win, file: &File<'_>, token: &Token) {
    let Some((line, col)) = spot(file, token) else {
        return;
    };
    let Some(at) = lines_at(file.path, line, col) else {
        return;
    };
    let id = format!("hover:{}:{line}:{col}", file.path);
    if kept(win, |kept, epoch| kept.asked.once(&id, epoch)) == Some(true) {
        ask(win, HOVER, at);
    }
    surface::fixes(win, file, line);
}

/// A rest on a token shows its card and asks the server once; Command-click on a name with no
/// references list goes to its definition. True when the event was ours.
pub fn heard(win: &mut Win, file: &File<'_>, event: CodeEvent, tx: &mut Tx) -> bool {
    match event {
        CodeEvent::HoverStart(token) => {
            let resting = resting(file, &token);
            kept(win, |kept, _| kept.resting = resting);
            hover(win, file, &token);
        }
        CodeEvent::HoverEnd(_) => {
            kept(win, |kept, _| kept.resting = None);
        }
        CodeEvent::CommandPress(token) if !token.follows => {
            if let Some((line, col)) = spot(file, &token) {
                surface::go_to(win, file.path, line, col);
            }
            tx.live_only();
        }
        CodeEvent::CommandPress(_)
        | CodeEvent::Press(_)
        | CodeEvent::GutterPress(_)
        | CodeEvent::SecondaryPress { .. }
        | CodeEvent::Selection(_) => return false,
    }
    true
}

/// Opens the answer to a go-to-definition once its row has come. Call it from the window's
/// poll; true when a file was opened.
pub fn follow(win: &mut Win, tx: &mut Tx) -> bool {
    surface::arrived(win, tx)
}

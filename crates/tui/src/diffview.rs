use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::colors::{DiffRowKind, DiffRowStyle, Theme};
use crate::highlight::{self, Lang};
use crate::transcript::show_controls;

/// Invariant: the gutter never narrows below three digits. Sized from the widest number,
/// crossing line 100 would re-pad rows already in native scrollback, which cannot be rewritten.
const GUTTER_MIN: usize = 3;
const INDENT: &str = "    ";
const SEPARATOR: &str = "⋮";
const SAFETY_ROWS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffBudget {
    pub hunks: usize,
    pub rows: usize,
}

impl DiffBudget {
    /// Collapsed limits: enough of a refactor to see its shape.
    pub const NORMAL: Self = Self { hunks: 8, rows: 40 };
    pub const FULL: Self = Self {
        hunks: usize::MAX,
        rows: SAFETY_ROWS,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    kind: DiffRowKind,
    number: u64,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Hunk {
    rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDiff {
    path: String,
    hunks: Vec<Hunk>,
}

/// A hunk header's `-old,n +new,m`. Absent counts default to one, which is what
/// a single-line range omits.
fn hunk_starts(header: &str) -> Option<(u64, u64)> {
    let body = header.strip_prefix("@@ ")?;
    let mut parts = body.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start =
        |span: &str| -> Option<u64> { span.split(',').next().and_then(|n| n.parse::<u64>().ok()) };
    Some((start(old)?, start(new)?))
}

fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old_no, mut new_no) = (0_u64, 0_u64);
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            files.push(FileDiff {
                path: show_controls(path.strip_prefix("b/").unwrap_or(path)),
                hunks: Vec::new(),
            });
            continue;
        }
        if line.starts_with("--- ") || line.starts_with('\\') {
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            if let Some((old, new)) = hunk_starts(line) {
                old_no = old;
                new_no = new;
                file.hunks.push(Hunk::default());
            }
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };
        // A deletion is numbered in the file it left, everything else in the
        // file that results — the only numbering under which both columns read.
        let (kind, number) = match line.as_bytes().first() {
            Some(b'+') => (DiffRowKind::Added, new_no),
            Some(b'-') => (DiffRowKind::Removed, old_no),
            _ => (DiffRowKind::Context, new_no),
        };
        match kind {
            DiffRowKind::Added => new_no = new_no.saturating_add(1),
            DiffRowKind::Removed => old_no = old_no.saturating_add(1),
            DiffRowKind::Context => {
                old_no = old_no.saturating_add(1);
                new_no = new_no.saturating_add(1);
            }
        }
        hunk.rows.push(Row {
            kind,
            number,
            text: show_controls(line.get(1..).unwrap_or_default()),
        });
    }
    files.retain(|file| !file.hunks.is_empty());
    files
}

struct Budgeted {
    files: Vec<FileDiff>,
    dropped_hunks: usize,
    dropped_rows: usize,
}

fn row_count(files: &[FileDiff]) -> usize {
    files
        .iter()
        .flat_map(|file| file.hunks.iter())
        .map(|hunk| hunk.rows.len())
        .sum()
}

fn hunk_count(files: &[FileDiff]) -> usize {
    files.iter().map(|file| file.hunks.len()).sum()
}

/// Whole hunks are kept or dropped, never half-formed; the tail goes first and the footer
/// names what went. The first hunk always stays, so an over-budget change shows its opening.
fn budgeted(files: Vec<FileDiff>, budget: DiffBudget) -> Budgeted {
    let (total_hunks, total_rows) = (hunk_count(&files), row_count(&files));
    let (mut hunks, mut rows) = (0_usize, 0_usize);
    let mut kept: Vec<FileDiff> = Vec::new();
    for file in files {
        let mut room = Vec::new();
        for hunk in file.hunks {
            let next = rows.saturating_add(hunk.rows.len());
            if hunks >= budget.hunks || (next > budget.rows && hunks > 0) {
                break;
            }
            hunks = hunks.saturating_add(1);
            rows = next;
            room.push(hunk);
        }
        if !room.is_empty() {
            kept.push(FileDiff {
                path: file.path,
                hunks: room,
            });
        }
    }
    Budgeted {
        dropped_hunks: total_hunks.saturating_sub(hunks),
        dropped_rows: total_rows.saturating_sub(rows),
        files: kept,
    }
}

/// Tokens keep their trailing spaces, so a rebuilt line is byte-identical.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        let boundary = bytes.get(index) == Some(&b' ');
        if boundary && bytes.get(index.saturating_add(1)) != Some(&b' ') {
            if let Some(token) = text.get(start..=index) {
                out.push(token);
            }
            start = index.saturating_add(1);
        }
        index = index.saturating_add(1);
    }
    if let Some(rest) = text.get(start..)
        && !rest.is_empty()
    {
        out.push(rest);
    }
    out
}

/// The span of `text` that differs from `other`, as a byte range. Indentation is excluded:
/// highlighting the leading whitespace of a changed line marks the one thing unchanged.
fn changed_span(text: &str, other: &str) -> Option<(usize, usize)> {
    let (mine, theirs) = (tokens(text), tokens(other));
    let head = mine
        .iter()
        .zip(theirs.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let room = mine.len().min(theirs.len()).saturating_sub(head);
    let tail = mine
        .iter()
        .rev()
        .zip(theirs.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(room);
    if head.saturating_add(tail) >= mine.len() {
        return None;
    }
    let start: usize = mine.iter().take(head).map(|token| token.len()).sum();
    let end: usize = mine
        .iter()
        .take(mine.len().saturating_sub(tail))
        .map(|token| token.len())
        .sum();
    let indent = text.len().saturating_sub(text.trim_start().len());
    Some((start.max(indent), end.max(start.max(indent))))
}

/// Split on display width, never on words: a diff row wraps by column, and a
/// word-wrapped one loses the alignment that makes two versions comparable.
fn split_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0_usize;
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if used.saturating_add(w) > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used = used.saturating_add(w);
    }
    if out.is_empty() || !current.is_empty() {
        out.push(current);
    }
    out
}

fn pad(spans: &mut Vec<Span<'static>>, width: usize, style: &DiffRowStyle) {
    let Some(fill) = style.fill else { return };
    let used: usize = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used)),
        Style::default().bg(fill),
    ));
}

fn sign_of(kind: DiffRowKind) -> char {
    match kind {
        DiffRowKind::Added => '+',
        DiffRowKind::Removed => '-',
        DiffRowKind::Context => ' ',
    }
}

/// The row less its chrome: indent + sign + number + `" │ "`.
fn content_width(gutter: usize, width: usize) -> usize {
    let chrome = INDENT
        .len()
        .saturating_add(1)
        .saturating_add(gutter)
        .saturating_add(3);
    width.saturating_sub(chrome).max(8)
}

/// `rows` and `index` rather than the two decisions they imply: both the blank
/// number and the word emphasis are read off the neighbouring rows.
fn row_lines(
    rows: &[Row],
    index: usize,
    gutter_width: usize,
    width: usize,
    theme: &Theme,
    mut lang: Option<&mut Lang>,
) -> Vec<Line<'static>> {
    let Some(row) = rows.get(index) else {
        return Vec::new();
    };
    let emphasis = emphasis_for(rows, index);
    let style = theme.diff_row(row.kind);
    let gutter = if repeats_previous(rows, index) {
        " ".repeat(gutter_width)
    } else {
        format!("{:>width$}", row.number, width = gutter_width)
    };
    let mut out = Vec::new();
    for (chunk_no, chunk) in split_width(&row.text, content_width(gutter_width, width))
        .into_iter()
        .enumerate()
    {
        let first = chunk_no == 0;
        let mut spans = vec![
            Span::raw(INDENT.to_owned()),
            Span::styled(
                if first {
                    sign_of(row.kind).to_string()
                } else {
                    " ".to_owned()
                },
                style.sign,
            ),
            Span::styled(
                if first {
                    gutter.clone()
                } else {
                    " ".repeat(gutter_width)
                },
                style.gutter,
            ),
            Span::styled(" │ ".to_owned(), style.gutter),
        ];
        match (emphasis.filter(|_| first), lang.as_deref_mut()) {
            (Some((start, end)), _) => {
                spans.extend(emphasized(&row.text, &chunk, start, end, &style));
            }
            // A deletion keeps its syntax colours dimmed, so the polarity still
            // reads when both sides are highlighted.
            (None, Some(lang)) => {
                let base = if row.kind == DiffRowKind::Removed {
                    style.content.add_modifier(Modifier::DIM)
                } else {
                    style.content
                };
                spans.extend(highlight::spans(&chunk, lang, theme, base));
            }
            (None, None) => spans.push(Span::styled(chunk, style.content)),
        }
        pad(&mut spans, width, &style);
        out.push(Line::from(spans));
    }
    out
}

/// The range is over the whole row, so a wrapped row carries it on the first
/// chunk — where a one-for-one replacement's difference is.
fn emphasized(
    text: &str,
    chunk: &str,
    start: usize,
    end: usize,
    style: &DiffRowStyle,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let limit = chunk.len();
    let (start, end) = (start.min(limit), end.min(limit));
    for (range, mark) in [(0..start, false), (start..end, true), (end..limit, false)] {
        let Some(part) = text.get(range) else {
            continue;
        };
        if part.is_empty() {
            continue;
        }
        let style = if mark {
            style.content.add_modifier(Modifier::REVERSED)
        } else {
            style.content
        };
        out.push(Span::styled(part.to_owned(), style));
    }
    out
}

/// A `-N` answered by `+N`, or an addition answered by the context line it
/// pushed down, repeats a number the eye already read.
fn repeats_previous(rows: &[Row], index: usize) -> bool {
    let Some(row) = rows.get(index) else {
        return false;
    };
    let Some(previous) = index.checked_sub(1).and_then(|i| rows.get(i)) else {
        return false;
    };
    if previous.number != row.number {
        return false;
    }
    matches!(
        (previous.kind, row.kind),
        (DiffRowKind::Removed, DiffRowKind::Added) | (DiffRowKind::Added, DiffRowKind::Context)
    )
}

/// A removal run and an addition run of one line each is a replacement, so the
/// changed tokens are marked; longer runs are a rewrite, where marking is noise.
fn emphasis_for(rows: &[Row], index: usize) -> Option<(usize, usize)> {
    let row = rows.get(index)?;
    if row.kind != DiffRowKind::Added {
        return None;
    }
    let previous = index.checked_sub(1).and_then(|i| rows.get(i))?;
    if previous.kind != DiffRowKind::Removed {
        return None;
    }
    let before_run = index
        .checked_sub(2)
        .and_then(|i| rows.get(i))
        .is_some_and(|r| r.kind == DiffRowKind::Removed);
    let after_run = rows
        .get(index.saturating_add(1))
        .is_some_and(|r| r.kind == DiffRowKind::Added);
    if before_run || after_run {
        return None;
    }
    changed_span(&row.text, &previous.text)
}

fn widest(files: &[FileDiff]) -> usize {
    let max = files
        .iter()
        .flat_map(|file| file.hunks.iter())
        .flat_map(|hunk| hunk.rows.iter())
        .map(|row| row.number)
        .max()
        .unwrap_or(0);
    max.to_string().len().max(GUTTER_MIN)
}

/// Renders a unified patch as transcript rows. `budget` decides how much of a
/// long change survives; the footer names what it dropped.
pub fn render(patch: &str, width: usize, theme: &Theme, budget: DiffBudget) -> Vec<Line<'static>> {
    let parsed = parse(patch);
    if parsed.is_empty() {
        return Vec::new();
    }
    let named = parsed.len() > 1;
    let gutter_width = widest(&parsed);
    let cut = budgeted(parsed, budget);
    let mut out = Vec::new();
    for file in &cut.files {
        let mut lang = highlight::lang_for(&file.path);
        if named {
            out.push(Line::from(Span::styled(
                format!("{INDENT}{}", file.path),
                theme.muted_style(),
            )));
        }
        for (index, hunk) in file.hunks.iter().enumerate() {
            if index > 0 {
                out.push(Line::from(Span::styled(
                    format!("{INDENT} {:>width$} {SEPARATOR}", "", width = gutter_width),
                    theme.dim_style(),
                )));
            }
            for position in 0..hunk.rows.len() {
                out.extend(row_lines(
                    &hunk.rows,
                    position,
                    gutter_width,
                    width,
                    theme,
                    lang.as_mut(),
                ));
            }
        }
    }
    if cut.dropped_rows > 0 {
        out.push(Line::from(Span::styled(
            format!(
                "{INDENT}… {} more hunk{}, {} more line{} · ctrl+o",
                cut.dropped_hunks,
                if cut.dropped_hunks == 1 { "" } else { "s" },
                cut.dropped_rows,
                if cut.dropped_rows == 1 { "" } else { "s" },
            ),
            theme.dim_style(),
        )));
    }
    out
}

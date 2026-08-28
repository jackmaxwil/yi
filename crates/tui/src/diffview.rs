use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::colors::{DiffRowKind, DiffRowStyle, Theme};
use crate::highlight::{self, Lang};

/// Invariant: the gutter never narrows below three digits. Derived from the
/// widest line number, a streaming diff crossing line 100 would re-pad rows
/// already committed to native scrollback, which cannot be rewritten.
const GUTTER_MIN: usize = 3;
const INDENT: &str = "    ";
const SEPARATOR: &str = "⋮";
/// A patch this long is a machine's mistake, not a change to read.
const SAFETY_ROWS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffBudget {
    pub hunks: usize,
    pub rows: usize,
}

impl DiffBudget {
    /// OMP's collapsed limits: enough of a refactor to see its shape.
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
                path: path.strip_prefix("b/").unwrap_or(path).to_owned(),
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
            text: line.get(1..).unwrap_or_default().to_owned(),
        });
    }
    files.retain(|file| !file.hunks.is_empty());
    files
}

fn is_change(kind: DiffRowKind) -> bool {
    kind != DiffRowKind::Context
}

/// Leading and trailing context are the cheapest rows to give up: the change
/// they frame is still readable with one line of ground on each side.
fn trim_edge_context(hunk: &mut Hunk) {
    let first_change = hunk.rows.iter().position(|row| is_change(row.kind));
    let Some(first) = first_change else { return };
    let last = hunk
        .rows
        .iter()
        .rposition(|row| is_change(row.kind))
        .unwrap_or(first);
    let head = first.saturating_sub(1);
    let tail = (last.saturating_add(2)).min(hunk.rows.len());
    hunk.rows = hunk.rows.drain(head..tail).collect();
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

/// Change rows outrank context, and whole hunks are dropped from the tail
/// rather than any hunk being shown half-formed.
fn budgeted(mut files: Vec<FileDiff>, budget: DiffBudget) -> Budgeted {
    let total_hunks = hunk_count(&files);
    let total_rows = row_count(&files);
    if total_hunks <= budget.hunks && total_rows <= budget.rows {
        return Budgeted {
            files,
            dropped_hunks: 0,
            dropped_rows: 0,
        };
    }
    let mut kept = 0_usize;
    for file in &mut files {
        let room = budget.hunks.saturating_sub(kept);
        file.hunks.truncate(room);
        kept = kept.saturating_add(file.hunks.len());
    }
    if row_count(&files) > budget.rows {
        for hunk in files.iter_mut().flat_map(|file| file.hunks.iter_mut()) {
            trim_edge_context(hunk);
        }
    }
    let mut used = 0_usize;
    for file in &mut files {
        let mut room = Vec::new();
        for hunk in file.hunks.drain(..) {
            let next = used.saturating_add(hunk.rows.len());
            if next > budget.rows && !room.is_empty() {
                break;
            }
            used = next;
            room.push(hunk);
        }
        file.hunks = room;
    }
    files.retain(|file| !file.hunks.is_empty());
    Budgeted {
        dropped_hunks: total_hunks.saturating_sub(hunk_count(&files)),
        dropped_rows: total_rows.saturating_sub(row_count(&files)),
        files,
    }
}

/// Tokens keep their trailing run of spaces so a rebuilt line is byte-identical
/// to the one that was split.
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

/// The span of `text` that differs from `other`, as a byte range. Indentation is
/// excluded: highlighting the leading whitespace of every changed line marks the
/// one thing that did not change.
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

struct Layout {
    gutter: usize,
    width: usize,
}

impl Layout {
    fn content_width(&self) -> usize {
        // indent + sign + number + " │ "
        let chrome = INDENT
            .len()
            .saturating_add(1)
            .saturating_add(self.gutter)
            .saturating_add(3);
        self.width.saturating_sub(chrome).max(8)
    }
}

fn row_lines(
    row: &Row,
    blank_number: bool,
    emphasis: Option<(usize, usize)>,
    layout: &Layout,
    theme: &Theme,
    lang: Option<&'static Lang>,
) -> Vec<Line<'static>> {
    let style = theme.diff_row(row.kind);
    let gutter = if blank_number {
        " ".repeat(layout.gutter)
    } else {
        format!("{:>width$}", row.number, width = layout.gutter)
    };
    let mut out = Vec::new();
    for (index, chunk) in split_width(&row.text, layout.content_width())
        .into_iter()
        .enumerate()
    {
        let first = index == 0;
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
                    " ".repeat(layout.gutter)
                },
                style.gutter,
            ),
            Span::styled(" │ ".to_owned(), style.gutter),
        ];
        match (emphasis.filter(|_| first), lang) {
            (Some((start, end)), _) => {
                spans.extend(emphasized(&row.text, &chunk, start, end, &style));
            }
            // A deletion keeps its syntax colours dimmed, so the polarity still
            // reads when both sides are highlighted (codex `:881-890`).
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
        pad(&mut spans, layout.width, &style);
        out.push(Line::from(spans));
    }
    out
}

/// The emphasis range is over the whole row, so a wrapped row only carries it on
/// the first chunk — which is where a one-for-one replacement's difference is.
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

/// OMP `diff.ts:55-95`: a removal run and an addition run of exactly one line
/// each is a replacement, and the tokens that actually changed are worth
/// marking. Longer runs are a rewrite, where per-token marking is noise.
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
    let layout = Layout {
        gutter: widest(&parsed),
        width,
    };
    let cut = budgeted(parsed, budget);
    let mut out = Vec::new();
    for file in &cut.files {
        // The patch names its own file, so the language needs no parameter.
        let lang = highlight::lang_for(&file.path);
        if named {
            out.push(Line::from(Span::styled(
                format!("{INDENT}{}", file.path),
                theme.muted_style(),
            )));
        }
        for (index, hunk) in file.hunks.iter().enumerate() {
            if index > 0 {
                out.push(Line::from(Span::styled(
                    format!("{INDENT} {:>width$} {SEPARATOR}", "", width = layout.gutter),
                    theme.dim_style(),
                )));
            }
            for (position, row) in hunk.rows.iter().enumerate() {
                out.extend(row_lines(
                    row,
                    repeats_previous(&hunk.rows, position),
                    emphasis_for(&hunk.rows, position),
                    &layout,
                    theme,
                    lang,
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

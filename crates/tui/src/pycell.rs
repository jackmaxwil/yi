use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::cell::{ToolCell, ToolStatus, TranscriptMode, spinner_frame};
use crate::colors::Theme;
use crate::highlight;
use crate::transcript::show_controls;
use crate::wrap::wrap_line;

const PREVIEW_CAP: usize = 64;
const PROMPT: &str = "› ";
const CONTINUATION: &str = "  ";
const BODY_INDENT: &str = "";
/// A base64 payload is never the line that says what a cell did.
const BLOB_RUN: usize = 32;

const EFFECTS: [&str; 10] = [
    "write_text",
    "mkdir",
    "execute",
    "subprocess",
    "rlm.",
    "to_csv",
    "savefig",
    "commit",
    "requests.",
    "open(",
];

const SECRETS: [&str; 5] = ["key", "secret", "token", "password", "passwd"];

fn is_skipped(line: &str) -> bool {
    let starts = [
        "import ", "from ", "export ", "source ", "print(", "len(", "%", "!",
    ];
    line.starts_with('#')
        || line.starts_with('@')
        || line == "set -e"
        || starts.iter().any(|prefix| line.starts_with(prefix))
}

/// Higher wins, first occurrence breaks a tie: an effect outranks a binding,
/// which outranks a bare call, which outranks anything else.
fn rank(line: &str) -> Option<u8> {
    if line.is_empty() || is_skipped(line) {
        return None;
    }
    if EFFECTS.iter().any(|effect| line.contains(effect)) {
        return Some(3);
    }
    if line.contains('(') {
        return Some(if line.contains('=') { 2 } else { 1 });
    }
    Some(0)
}

fn looks_like_blob(token: &str) -> bool {
    token.len() >= BLOB_RUN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// Matched anywhere in the token, not only at its start, since a key is usually an argument;
/// the run after the prefix must be key-shaped so `task-oriented` is not mistaken for one.
fn holds_api_key(token: &str) -> bool {
    let Some(at) = token.find("sk-") else {
        return false;
    };
    let rest = token.get(at.saturating_add(3)..).unwrap_or_default();
    rest.bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        .count()
        >= 16
}

/// A cell's source reaches the screen and every frame dump of it. Both halves are
/// load-bearing: the name catches `api_key = "…"`, the shape a bare literal naming nothing.
pub fn redact(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let named = SECRETS.iter().any(|needle| lower.contains(needle));
    let mut out: Vec<String> = Vec::new();
    for token in line.split(' ') {
        // Trailing punctuation is the caller's syntax, not the secret's: kept,
        // so a redacted argument still reads as an argument.
        let tail_at = token.trim_end_matches([')', ',', ';', ']', '}']).len();
        let (body, tail) = token.split_at(tail_at.min(token.len()));
        let quoted = body.starts_with('"') || body.starts_with('\'');
        let trimmed = body.trim_matches(['"', '\'']);
        if holds_api_key(trimmed) || (named && quoted) {
            out.push(format!("<redacted>{tail}"));
        } else if looks_like_blob(trimmed) {
            out.push(format!("<blob>{tail}"));
        } else {
            out.push(token.to_owned());
        }
    }
    out.join(" ")
}

/// The one line a reader would name if asked what the cell did.
pub fn preview(code: &str) -> String {
    let best = code
        .lines()
        .map(str::trim)
        .filter_map(|line| rank(line).map(|rank| (rank, line)))
        .max_by_key(|(rank, _)| *rank);
    let Some((_, line)) = best else {
        return String::new();
    };
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact(&collapsed);
    if redacted.chars().count() > PREVIEW_CAP {
        return redacted
            .chars()
            .take(PREVIEW_CAP.saturating_sub(1))
            .collect::<String>()
            + "…";
    }
    redacted
}

/// The kernel reports a structured error, so this is only for the mixed case: a
/// cell that printed before it raised, whose stdout would read as traceback.
pub fn split_traceback(text: &str) -> (&str, &str) {
    let marker = text
        .find("Traceback (most recent call last):")
        .or_else(|| text.find("\nTraceback ("));
    match marker {
        Some(at) => (
            text.get(..at).unwrap_or_default(),
            text.get(at..).unwrap_or_default(),
        ),
        None => (text, ""),
    }
}

fn string(details: &Value, key: &str) -> String {
    details
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.trim_end_matches('\n').split('\n').count()
}

/// `%%bash` is a shell cell whose output is a shell's; naming it python because
/// the kernel is a Python one tells the reader the wrong thing about the code.
pub fn chip(code: &str) -> &'static str {
    if code.trim_start().starts_with("%%bash") {
        "bash"
    } else {
        "python"
    }
}

/// Invariant: byte-identical in every transcript mode. A head that changes
/// width when the body opens moves every row under it.
pub fn head(cell: &ToolCell, spinner_phase: usize) -> String {
    let code = string(&cell.details, "code");
    let lead = match cell.status {
        ToolStatus::Running => format!("{} ", spinner_frame(spinner_phase)),
        ToolStatus::Awaiting => "△ ".to_owned(),
        _ => String::new(),
    };
    let mut parts = vec![chip(&code).to_owned()];
    let preview = preview(&code);
    if !preview.is_empty() {
        parts.push(preview);
    }
    format!("{lead}⊙ {}", parts.join(" · "))
}

fn chips(cell: &ToolCell, theme: &Theme) -> Vec<crate::card::Chip> {
    use crate::card::Chip;
    let code = string(&cell.details, "code");
    let mut chips = Vec::new();
    let inputs = line_count(&code);
    let outputs = line_count(&string(&cell.details, "stdout"))
        .saturating_add(line_count(&string(&cell.details, "stderr")))
        .saturating_add(line_count(&string(&cell.details, "result")))
        .saturating_add(line_count(&traceback_of(&cell.details)));
    if inputs > 0 {
        let value = if outputs > 0 {
            format!("↑ {inputs} ↓ {outputs}")
        } else {
            format!("↑ {inputs}")
        };
        chips.push(Chip::new(value, "lines", Style::default().fg(theme.text)));
    }
    let duration = cell
        .details
        .get("durationMs")
        .and_then(Value::as_u64)
        .unwrap_or(cell.elapsed_ms);
    chips.extend(Chip::elapsed(duration, theme));
    let ename = cell
        .details
        .pointer("/error/ename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !ename.is_empty() {
        chips.push(Chip::new(
            ename.to_owned(),
            "",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ));
    }
    chips
}

fn gutter_lines(code: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lang = highlight::lang_for(chip(code));
    let base = Style::default().fg(theme.text);
    code.lines()
        .enumerate()
        .flat_map(|(index, source)| {
            let marker = if index == 0 { PROMPT } else { CONTINUATION };
            // The body is redacted on the head's terms: the whole transcript gets shared,
            // and a key is no safer one mode deeper than on the row above.
            let source = redact(&show_controls(source));
            let mut spans = vec![Span::styled(
                format!("{BODY_INDENT}{marker}"),
                theme.dim_style(),
            )];
            match lang.as_mut() {
                Some(lang) => spans.extend(highlight::spans(&source, lang, theme, base)),
                None => spans.push(Span::styled(source, base)),
            }
            wrap_line(&Line::from(spans), width, "      ")
        })
        .collect()
}

fn stream_lines(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    text.trim_end_matches('\n')
        .lines()
        .flat_map(|row| {
            wrap_line(
                &Line::from(Span::styled(
                    format!("{BODY_INDENT}  {}", show_controls(row)),
                    style,
                )),
                width,
                "        ",
            )
        })
        .collect()
}

pub fn traceback_of(details: &Value) -> String {
    let rows: Vec<String> = details
        .pointer("/error/traceback")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if !rows.is_empty() {
        return rows.join("\n");
    }
    let ename = details
        .pointer("/error/ename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let evalue = details
        .pointer("/error/evalue")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if ename.is_empty() {
        return String::new();
    }
    format!("{ename}: {evalue}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRow {
    Frame,
    Source,
    Error,
    Raised,
}

fn separator(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '-')
}

fn frame(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("File ")
        || trimmed.starts_with("Cell In[")
        || trimmed.contains("Traceback (most recent call last)")
}

fn caret(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '^' || c == '~')
}

fn gutter(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut at = 0_usize;
    while bytes
        .get(at)
        .is_some_and(|b| matches!(b, b' ' | b'-' | b'>'))
    {
        at = at.saturating_add(1);
    }
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at = at.saturating_add(1);
    }
    while bytes.get(at) == Some(&b' ') {
        at = at.saturating_add(1);
    }
    at
}

/// A `----> 61` row, as ipython marks the line a frame ran: two dashes at least, then the arrow.
fn arrow(line: &str) -> bool {
    let trimmed = line.trim_start();
    let dashes = trimmed
        .len()
        .saturating_sub(trimmed.trim_start_matches('-').len());
    dashes >= 2
        && trimmed
            .get(dashes..)
            .is_some_and(|rest| rest.starts_with('>'))
}

/// `SyntaxError: invalid syntax`, unindented and named: the one row a failed block exists to say.
fn exception(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.len() < line.len() || caret(line) || separator(line) || frame(line) {
        return false;
    }
    let Some((name, _)) = trimmed.split_once(':') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.'))
}

fn trace_row(line: &str, last: TraceRow) -> TraceRow {
    if line.trim().is_empty() || separator(line) || frame(line) {
        return TraceRow::Frame;
    }
    let indent = line.len().saturating_sub(line.trim_start().len());
    let source = arrow(line)
        || (!caret(line) && indent >= 4 && matches!(last, TraceRow::Frame | TraceRow::Source));
    if source {
        TraceRow::Source
    } else if exception(line) {
        TraceRow::Raised
    } else {
        TraceRow::Error
    }
}

/// Invariant: a frame's source row keeps its own syntax colour, and the numbering the kernel
/// printed above it stays scaffolding, so a twenty-row traceback reads as code and not a wall.
pub fn traceback_rows(
    text: &str,
    code: &str,
    theme: &Theme,
) -> Vec<(TraceRow, Vec<Span<'static>>)> {
    let mut lang = highlight::lang_for(chip(code));
    let base = Style::default().fg(theme.text);
    let error = Style::default().fg(theme.error);
    let mut last = TraceRow::Frame;
    text.trim_end_matches('\n')
        .split('\n')
        .map(|raw| {
            let line = show_controls(raw);
            let row = trace_row(&line, last);
            if !line.trim().is_empty() {
                last = row;
            }
            let spans = match row {
                TraceRow::Frame => vec![Span::styled(line, theme.dim_style())],
                TraceRow::Error => vec![Span::styled(line, error)],
                TraceRow::Raised => vec![Span::styled(line, error.add_modifier(Modifier::BOLD))],
                TraceRow::Source => source_spans(&line, lang.as_mut(), theme, base),
            };
            (row, spans)
        })
        .collect()
}

fn source_spans(
    line: &str,
    lang: Option<&mut highlight::Lang>,
    theme: &Theme,
    base: Style,
) -> Vec<Span<'static>> {
    let at = gutter(line);
    let (lead, body) = if line.is_char_boundary(at) {
        line.split_at(at)
    } else {
        ("", line)
    };
    let mut spans = vec![Span::styled(lead.to_owned(), theme.dim_style())];
    match lang {
        Some(lang) => spans.extend(highlight::spans(body, lang, theme, base)),
        None => spans.push(Span::styled(body.to_owned(), base)),
    }
    spans
}

fn traceback_lines(text: &str, code: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    traceback_rows(text, code, theme)
        .into_iter()
        .flat_map(|(_, spans)| {
            let mut row = vec![Span::raw("  ")];
            row.extend(spans);
            wrap_line(&Line::from(row), width, "        ")
        })
        .collect()
}

/// A failed cell shows its source and its traceback in every mode: a reader who
/// cannot see why the kernel raised cannot act on it.
pub fn lines(
    cell: &ToolCell,
    width: usize,
    theme: &Theme,
    mode: TranscriptMode,
    spinner_phase: usize,
) -> Vec<Line<'static>> {
    let failed = matches!(cell.status, ToolStatus::Failed | ToolStatus::Denied);
    let expanded = mode == TranscriptMode::Verbose;
    let style = match cell.status {
        ToolStatus::Awaiting => Style::default().fg(theme.warning),
        ToolStatus::Failed => Style::default().fg(theme.error),
        ToolStatus::Denied => theme.muted_style().add_modifier(Modifier::CROSSED_OUT),
        ToolStatus::Running | ToolStatus::Done => Style::default().fg(theme.text),
    };
    let text = head(cell, spinner_phase);
    let (lead, rest) = text.split_at(text.find('⊙').unwrap_or(0));
    let (name, rest) = rest.split_once(" · ").unwrap_or((rest, ""));
    let mut spans = vec![
        Span::styled(lead.to_owned(), theme.accent_style()),
        Span::styled(
            name.to_owned(),
            Style::default()
                .fg(crate::card::tool_hue(theme, "ipython"))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if !rest.is_empty() {
        spans.push(Span::styled(format!("  {rest}"), style));
    }
    let inner = crate::card::body_width(width);
    let mut out = Vec::new();
    let card = |body| {
        crate::card::card(
            Line::from(spans.clone()),
            &chips(cell, theme),
            body,
            width,
            theme,
            cell.status,
        )
    };
    if !expanded && !failed {
        return card(out);
    }
    let code = string(&cell.details, "code");
    if !code.is_empty() {
        out.extend(gutter_lines(&code, inner, theme));
    }
    let raw_stdout = string(&cell.details, "stdout");
    let (stdout, trailing) = split_traceback(&raw_stdout);
    let plain = Style::default().fg(theme.text);
    for (text, style) in [
        (stdout.to_owned(), plain),
        (string(&cell.details, "result"), plain),
        (string(&cell.details, "stderr"), theme.muted_style()),
    ] {
        let bounded = (!expanded).then(|| crate::transcript::preview_lines(&text, 5, 5).join("\n"));
        let shown = bounded.as_deref().unwrap_or(&text);
        if !text.trim().is_empty() {
            out.extend(stream_lines(shown, style, inner));
        }
    }
    for text in [trailing.to_owned(), traceback_of(&cell.details)] {
        if text.trim().is_empty() {
            continue;
        }
        let bounded = (!expanded).then(|| crate::transcript::preview_lines(&text, 5, 5).join("\n"));
        let shown = bounded.as_deref().unwrap_or(&text);
        out.extend(traceback_lines(shown, &code, inner, theme));
    }
    card(out)
}

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::cell::{ToolCell, ToolStatus, TranscriptMode, spinner_frame};
use crate::colors::Theme;
use crate::diffview::{self, DiffBudget};
use crate::wrap::wrap_line;

const PREVIEW_CAP: usize = 64;
const PROMPT: &str = "› ";
const CONTINUATION: &str = "  ";
const BODY_INDENT: &str = "    ";
/// A base64 payload pasted into a cell is never the line that says what the
/// cell did, and printing it costs the whole preview.
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

/// Matched anywhere in the token, not only at its start: a key is usually an
/// argument (`Anthropic(api_key="sk-…")`), and the run after the prefix has to
/// be key-shaped so `task-oriented` is not mistaken for one.
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

/// A cell's source reaches the screen and every frame dump taken of it. Both
/// halves are load-bearing: the name catches `api_key = "…"`, the shape catches
/// a bare literal that names nothing.
fn redact(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let named = SECRETS.iter().any(|needle| lower.contains(needle));
    let mut out: Vec<String> = Vec::new();
    for token in line.split(' ') {
        let quoted = token.starts_with('"') || token.starts_with('\'');
        let trimmed = token.trim_matches(['"', '\'']);
        if holds_api_key(trimmed) || (named && quoted) {
            out.push("<redacted>".to_owned());
        } else if looks_like_blob(trimmed) {
            out.push("<blob>".to_owned());
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

/// The kernel reports a structured error, so the heuristic exists only for the
/// mixed case: a cell that printed before it raised, whose stdout would
/// otherwise be read as part of the traceback.
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

fn elapsed_label(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 1000 {
        format!("{}.{}s", ms / 1000, (ms % 1000) / 100)
    } else {
        format!("{ms}ms")
    }
}

/// `%%bash` is a shell cell whose output is a shell's; naming it python because
/// the kernel is a Python one tells the reader the wrong thing about the code.
fn chip(code: &str) -> &'static str {
    if code.trim_start().starts_with("%%bash") {
        "bash"
    } else {
        "python"
    }
}

/// Invariant: this line is byte-identical in every transcript mode. prime-agent
/// `ipython-cell.ts:368-373` — a head that changes width when the body opens
/// moves every row under it, and the reader loses their place.
pub fn head(cell: &ToolCell, spinner_phase: usize) -> String {
    let code = string(&cell.details, "code");
    let glyph = match cell.status {
        ToolStatus::Running => spinner_frame(spinner_phase),
        ToolStatus::Done => '✓',
        ToolStatus::Failed | ToolStatus::Denied => '✗',
    };
    let mut parts = vec![chip(&code).to_owned()];
    let preview = preview(&code);
    if !preview.is_empty() {
        parts.push(preview);
    }
    let inputs = line_count(&code);
    let outputs = line_count(&string(&cell.details, "stdout"))
        .saturating_add(line_count(&string(&cell.details, "stderr")))
        .saturating_add(line_count(&string(&cell.details, "result")));
    if inputs > 0 {
        parts.push(if outputs > 0 {
            format!("↑ {inputs} ↓ {outputs} lines")
        } else {
            format!("↑ {inputs} lines")
        });
    }
    let duration = cell
        .details
        .get("durationMs")
        .and_then(Value::as_u64)
        .unwrap_or(cell.elapsed_ms);
    if duration > 0 {
        parts.push(elapsed_label(duration));
    }
    let ename = cell
        .details
        .pointer("/error/ename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !ename.is_empty() {
        parts.push(ename.to_owned());
    }
    format!("  {glyph} ⊙ {}", parts.join(" · "))
}

fn gutter_lines(code: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    code.lines()
        .enumerate()
        .flat_map(|(index, source)| {
            let marker = if index == 0 { PROMPT } else { CONTINUATION };
            wrap_line(
                &Line::from(vec![
                    Span::styled(format!("{BODY_INDENT}{marker}"), theme.dim_style()),
                    Span::styled(source.to_owned(), Style::default().fg(theme.text)),
                ]),
                width,
                "      ",
            )
        })
        .collect()
}

fn stream_lines(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    text.trim_end_matches('\n')
        .lines()
        .flat_map(|row| {
            wrap_line(
                &Line::from(Span::styled(format!("{BODY_INDENT}  {row}"), style)),
                width,
                "        ",
            )
        })
        .collect()
}

fn traceback_of(details: &Value) -> String {
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

/// The kernel's own file edits: a cell that wrote three files has three diffs
/// worth the same rows an `edit` call would earn.
fn diff_lines(
    details: &Value,
    width: usize,
    theme: &Theme,
    budget: DiffBudget,
) -> Vec<Line<'static>> {
    let Some(diffs) = details.get("diffs").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for diff in diffs {
        let path = diff.get("path").and_then(Value::as_str).unwrap_or("(cell)");
        let old = diff.get("old_str").and_then(Value::as_str).unwrap_or("");
        let new = diff.get("new_str").and_then(Value::as_str).unwrap_or("");
        let patch = format!(
            "--- a/{path}\n+++ b/{path}\n@@ -1,{} +1,{} @@\n{}{}",
            line_count(old).max(1),
            line_count(new).max(1),
            old.lines().map(|l| format!("-{l}\n")).collect::<String>(),
            new.lines().map(|l| format!("+{l}\n")).collect::<String>(),
        );
        out.push(Line::from(Span::styled(
            format!("{BODY_INDENT}╰─ {path}"),
            theme.muted_style(),
        )));
        out.extend(diffview::render(&patch, width, theme, budget));
    }
    out
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
    let mut out = wrap_line(
        &Line::from(Span::styled(
            head(cell, spinner_phase),
            match cell.status {
                ToolStatus::Running => Style::default().fg(theme.text),
                ToolStatus::Done => theme.muted_style(),
                ToolStatus::Failed => Style::default().fg(theme.error),
                ToolStatus::Denied => theme.muted_style().add_modifier(Modifier::CROSSED_OUT),
            },
        )),
        width,
        "    ",
    );
    let budget = if expanded {
        DiffBudget::FULL
    } else {
        DiffBudget::NORMAL
    };
    out.extend(diff_lines(&cell.details, width, theme, budget));
    if !expanded && !failed {
        return out;
    }
    let code = string(&cell.details, "code");
    if !code.is_empty() {
        out.push(Line::default());
        out.extend(gutter_lines(&code, width, theme));
    }
    let raw_stdout = string(&cell.details, "stdout");
    let (stdout, trailing) = split_traceback(&raw_stdout);
    for (text, style) in [
        (stdout.to_owned(), Style::default().fg(theme.text)),
        (
            string(&cell.details, "result"),
            Style::default().fg(theme.text),
        ),
        (string(&cell.details, "stderr"), theme.muted_style()),
        (trailing.to_owned(), Style::default().fg(theme.error)),
        (
            traceback_of(&cell.details),
            Style::default().fg(theme.error),
        ),
    ] {
        if !text.trim().is_empty() {
            out.extend(stream_lines(&text, style, width));
        }
    }
    out
}

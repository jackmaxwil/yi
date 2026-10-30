//! The sources a run of read-only calls touched: one dim row in every mode but verbose, which
//! opens it into the per-call block.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cell::{ToolCell, ToolStatus, TranscriptMode, explore_verb, glyph, subject_spans};
use crate::colors::Theme;
use crate::wrap::wrap_line;

const EXPLORED_CAP: usize = 32;
/// Fixed, not measured: a width read off one run would leave two adjacent
/// Explored blocks with their subjects in different columns.
const VERB_WIDTH: usize = 6;

pub(crate) fn lines(
    rows: &[ToolCell],
    width: usize,
    theme: &Theme,
    mode: TranscriptMode,
) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    if mode != TranscriptMode::Verbose {
        return vec![citation_row(rows, width, theme)];
    }
    let head = Line::from(vec![
        Span::styled(
            "✱ Explored",
            Style::default()
                .fg(crate::card::tool_hue(theme, "read"))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ×{}", rows.len()), theme.muted_style()),
    ]);
    let inner = crate::card::body_width(width);
    let mut out = Vec::new();
    for row in rows.iter().take(EXPLORED_CAP) {
        let verb = explore_verb(&row.name).unwrap_or("Ran");
        let mut spans = vec![Span::styled(
            format!("{verb:<VERB_WIDTH$} "),
            Style::default().fg(theme.accent),
        )];
        spans.extend(subject_spans(
            &row.name,
            subject(row),
            theme,
            Style::default().fg(theme.text),
        ));
        if let Some(digest) = &row.digest {
            spans.push(Span::styled(format!("  {digest}"), theme.dim_style()));
        }
        out.extend(wrap_line(&Line::from(spans), inner, "        "));
    }
    if let Some(extra) = rows.len().checked_sub(EXPLORED_CAP).filter(|n| *n > 0) {
        out.push(Line::from(Span::styled(
            format!("… {extra} more"),
            theme.dim_style(),
        )));
    }
    crate::card::card(head, &[], out, width, theme, ToolStatus::Done)
}

/// The summary opens with the tool's own glyph and label, which the row already carries; what is
/// left is the source.
fn subject(row: &ToolCell) -> &str {
    row.summary
        .split_once(&crate::cell::label(&row.name))
        .map(|(_, rest)| rest.trim())
        .unwrap_or(row.summary.as_str())
}
fn citation_row(rows: &[ToolCell], width: usize, theme: &Theme) -> Line<'static> {
    let mut text = String::new();
    let mut shown = 0_usize;
    for (index, row) in rows.iter().enumerate() {
        let piece = format!(
            "{}{} {}",
            if index == 0 { "" } else { "  " },
            glyph(&row.name),
            subject(row)
        );
        let hidden = rows.len().saturating_sub(index).saturating_sub(1);
        let tail = tail_of(hidden);
        let fits = width
            >= [text.as_str(), piece.as_str(), tail.as_str()]
                .into_iter()
                .map(UnicodeWidthStr::width)
                .fold(0, usize::saturating_add);
        if fits {
            text.push_str(&piece);
        } else if index == 0 {
            let room = width.saturating_sub(UnicodeWidthStr::width(tail.as_str()));
            text.push_str(&clip(&piece, room));
        } else {
            break;
        }
        shown = index.saturating_add(1);
    }
    text.push_str(&tail_of(rows.len().saturating_sub(shown)));
    Line::from(Span::styled(text, theme.dim_style()))
}

/// A cut names what it hid, at the cut: `+N` is the calls the row could not carry.
fn tail_of(hidden: usize) -> String {
    if hidden == 0 {
        return String::new();
    }
    format!(" +{hidden}")
}

fn clip(text: &str, width: usize) -> String {
    let mut used = 0_usize;
    let mut out = String::new();
    for ch in text.chars() {
        used = used.saturating_add(UnicodeWidthChar::width(ch).unwrap_or(0));
        if used > width {
            break;
        }
        out.push(ch);
    }
    out
}

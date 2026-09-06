//! The call card: a rail in the outcome's colour, the head, chips right-aligned to the
//! measure, and body rows on a tinted ground.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::cell::ToolStatus;
use crate::colors::Theme;
use crate::wrap::wrap_line;

pub const RAIL: &str = "│ ";
const INSET: &str = "  ";
const GAP: usize = 2;

/// A number that carries the colour and a unit that does not: `412` bright, `lines` dim.
pub struct Chip {
    pub value: String,
    pub unit: String,
    pub style: Style,
}

impl Chip {
    pub fn new(value: impl Into<String>, unit: &str, style: Style) -> Self {
        Self {
            value: value.into(),
            unit: unit.to_owned(),
            style,
        }
    }

    pub fn count(count: usize, noun: &str, theme: &Theme) -> Self {
        let unit = if count == 1 {
            noun.to_owned()
        } else {
            format!("{noun}s")
        };
        Self::new(count.to_string(), &unit, Style::default().fg(theme.text))
    }

    /// Dim under a second, amber past one, red past ten: the wait is the story.
    pub fn elapsed(ms: u64, theme: &Theme) -> Option<Self> {
        if ms == 0 {
            return None;
        }
        let style = if ms >= 10_000 {
            Style::default().fg(theme.error)
        } else if ms >= 1_000 {
            Style::default().fg(theme.warning)
        } else {
            theme.dim_style()
        };
        Some(Self::new(crate::cell::elapsed_label(ms), "", style))
    }

    pub fn exit(code: i64, theme: &Theme) -> Self {
        let style = if code == 0 {
            Style::default().fg(theme.success)
        } else {
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD)
        };
        Self::new(format!("⏎ {code}"), "", style)
    }
}

pub fn rail_style(theme: &Theme, status: ToolStatus) -> Style {
    let colour = match status {
        ToolStatus::Running => theme.accent,
        ToolStatus::Awaiting => theme.warning,
        ToolStatus::Done => theme.success,
        ToolStatus::Failed | ToolStatus::Denied => theme.error,
    };
    Style::default().fg(colour)
}

/// One hue per family of tool, so a column of calls reads by colour before by name.
pub fn tool_hue(theme: &Theme, name: &str) -> Color {
    match name {
        "bash" => theme.orange,
        "read" | "grep" | "glob" | "find" => theme.cyan,
        "edit" | "write" => theme.magenta,
        "ipython" => theme.teal,
        "plan" => theme.purple,
        _ => theme.warning,
    }
}

/// The columns a body row may use once the rail and the inset have theirs.
pub fn body_width(width: usize) -> usize {
    width
        .saturating_sub(RAIL.width())
        .saturating_sub(INSET.len())
        .max(1)
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| span.content.as_ref().width())
        .sum()
}

fn chip_spans(chips: &[Chip], theme: &Theme) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut used = 0;
    for (index, chip) in chips.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" ".repeat(GAP)));
            used += GAP;
        }
        spans.push(Span::styled(chip.value.clone(), chip.style));
        used += chip.value.width();
        if !chip.unit.is_empty() {
            spans.push(Span::styled(format!(" {}", chip.unit), theme.dim_style()));
            used += chip.unit.width() + 1;
        }
    }
    (spans, used)
}

/// Chips ride the head's row when they fit after it, else their own row, always flush right.
pub fn card(
    head: Line<'static>,
    chips: &[Chip],
    body: Vec<Line<'static>>,
    width: usize,
    theme: &Theme,
    status: ToolStatus,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(RAIL.width()).max(1);
    let mut rows = wrap_line(&head, inner, INSET);
    let (chip_row, chip_width) = chip_spans(chips, theme);
    if chip_width > 0 {
        let head_width = rows.last().map_or(0, line_width);
        let fits = rows.len() == 1 && head_width + GAP + chip_width <= inner;
        if fits && let Some(last) = rows.last_mut() {
            last.spans
                .push(Span::raw(" ".repeat(inner - head_width - chip_width)));
            last.spans.extend(chip_row);
        } else {
            let mut spans = vec![Span::raw(" ".repeat(inner.saturating_sub(chip_width)))];
            spans.extend(chip_row);
            rows.push(Line::from(spans));
        }
    }
    let ground = theme.user_style();
    for row in body {
        let used = line_width(&row) + INSET.len();
        let mut spans = vec![Span::styled(INSET, ground)];
        spans.extend(
            row.spans
                .into_iter()
                .map(|span| Span::styled(span.content, ground.patch(span.style))),
        );
        spans.push(Span::styled(" ".repeat(inner.saturating_sub(used)), ground));
        rows.push(Line::from(spans));
    }
    let rail = rail_style(theme, status);
    rows.into_iter()
        .map(|row| {
            let mut spans = vec![Span::styled(RAIL, rail)];
            spans.extend(row.spans);
            Line::from(spans)
        })
        .collect()
}

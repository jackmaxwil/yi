//! The header row: workspace tabs at the left, the four pane buttons at the right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::Theme;

use crate::app::App;
use crate::keys;

/// Invariant: hit widths come from the label builder the bar draws with.
pub(crate) fn tab_hits(app: &App, bar: Rect) -> Vec<(Rect, usize)> {
    let mut x = bar.x;
    let mut hits = Vec::new();
    for (index, tab) in app.state.tabs.iter().enumerate() {
        let width = label_width(&tab_label(index, tab.zoomed));
        hits.push((Rect::new(x, bar.y, width, 1), index));
        x = x.saturating_add(width);
    }
    hits
}

fn tab_label(index: usize, zoomed: bool) -> String {
    let zoom = if zoomed { "Z" } else { "" };
    format!(" {}{zoom} ", index.saturating_add(1))
}

const HEADER_BUTTONS: [(&str, &str, keys::Action); 4] = [
    (">_", "Terminal", keys::Action::OpenTerminal),
    ("±", "Diff", keys::Action::ToggleDiff),
    ("◉", "Browser", keys::Action::OpenBrowser),
    ("▤", "Files", keys::Action::OpenEditor),
];

fn button_label(glyph: &str, word: &str, worded: bool) -> String {
    if worded {
        format!(" {glyph} {word} ")
    } else {
        format!(" {glyph:^2} ")
    }
}

fn label_width(label: &str) -> u16 {
    u16::try_from(label.chars().count()).unwrap_or(u16::MAX)
}

/// The buttons flush right with a one-cell gap, worded when they fit beside the tabs and
/// glyph-only when they do not.
pub(crate) fn header_buttons(header: Rect, taken: u16) -> Vec<(Rect, keys::Action)> {
    let room = header.width.saturating_sub(taken);
    let need = |worded: bool| -> u16 {
        HEADER_BUTTONS
            .iter()
            .map(|(glyph, word, _)| {
                label_width(&button_label(glyph, word, worded)).saturating_add(1)
            })
            .fold(0, u16::saturating_add)
    };
    let worded = need(true) <= room;
    let mut right = header.right().saturating_sub(1);
    let mut out = Vec::new();
    for (glyph, word, action) in HEADER_BUTTONS.iter().rev() {
        let width = label_width(&button_label(glyph, word, worded));
        let Some(left) = right.checked_sub(width) else {
            break;
        };
        if left < header.x.saturating_add(taken) {
            break;
        }
        out.push((Rect::new(left, header.y, width, 1), *action));
        right = left.saturating_sub(1);
    }
    out.reverse();
    out
}

pub(crate) fn render_header(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let buttons = app.hits.iter().flat_map(|hits| &hits.header_buttons);
    for (rect, action) in buttons {
        let Some((glyph, word, _)) = HEADER_BUTTONS.iter().find(|(.., a)| a == action) else {
            continue;
        };
        let worded = rect.width > label_width(&button_label(glyph, word, false));
        let chip = Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(button_label(glyph, word, worded), chip)),
            *rect,
        );
    }
    if app.state.tabs.len() > 1 {
        render_tabs(app, frame, area, theme);
    }
}

fn render_tabs(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut spans = Vec::new();
    for (index, tab) in app.state.tabs.iter().enumerate() {
        let label = tab_label(index, tab.zoomed);
        let style = if index == app.state.active_tab {
            Style::default()
                .fg(theme.text)
                .bg(theme.selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            theme.dim_style()
        };
        spans.push(Span::styled(label, style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

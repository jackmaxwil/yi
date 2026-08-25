use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};
use crate::wrap::wrap_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskChoice {
    AllowOnce,
    AllowAlways,
    Reject,
}

const OPTIONS: [(AskChoice, &str); 3] = [
    (AskChoice::AllowOnce, "Allow once"),
    (AskChoice::AllowAlways, "Allow always"),
    (AskChoice::Reject, "Reject"),
];

/// U12: fixed-height approval view — height is set at spawn so the live
/// region never jitters while the user decides.
pub struct ApprovalView {
    pub title: String,
    pub description: String,
    pub selected: usize,
    pub outcome: Option<AskChoice>,
}

impl ApprovalView {
    pub fn new(title: String, description: String) -> Self {
        Self {
            title,
            description,
            selected: 0,
            outcome: None,
        }
    }
}

impl BottomView for ApprovalView {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(Span::styled(
            format!(" △ {}", self.title),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ))];
        let mut description = self.description.clone();
        if description.chars().count() > 200 {
            description = description.chars().take(200).collect::<String>() + "…";
        }
        out.extend(
            wrap_line(
                &Line::from(Span::styled(
                    format!("   {description}"),
                    theme.muted_style(),
                )),
                width,
                "   ",
            )
            .into_iter()
            .take(3),
        );
        let mut options = Vec::new();
        for (i, (_, label)) in OPTIONS.iter().enumerate() {
            let style = if i == self.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.muted_style()
            };
            let marker = if i == self.selected { "›" } else { " " };
            options.push(Span::styled(format!(" {marker} {label}  "), style));
        }
        out.push(Line::from(options));
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        match key.code {
            KeyCodeValue::Left | KeyCodeValue::Up => {
                self.selected = self.selected.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Right | KeyCodeValue::Down | KeyCodeValue::Tab => {
                if self.selected + 1 < OPTIONS.len() {
                    self.selected += 1;
                }
                PopupResult::Open
            }
            KeyCodeValue::Enter => {
                self.outcome = OPTIONS.get(self.selected).map(|(choice, _)| *choice);
                PopupResult::Close
            }
            KeyCodeValue::Esc => {
                self.outcome = Some(AskChoice::Reject);
                PopupResult::Close
            }
            KeyCodeValue::Char('y') => {
                self.outcome = Some(AskChoice::AllowOnce);
                PopupResult::Close
            }
            KeyCodeValue::Char('a') => {
                self.outcome = Some(AskChoice::AllowAlways);
                PopupResult::Close
            }
            KeyCodeValue::Char('n') => {
                self.outcome = Some(AskChoice::Reject);
                PopupResult::Close
            }
            _ => PopupResult::Open,
        }
    }
}

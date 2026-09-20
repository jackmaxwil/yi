use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};
use crate::wrap::wrap_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskChoice {
    AllowOnce,
    AllowAlways(usize),
    Reject,
}

/// Diff rows the prompt shows. The permission layer already cuts the patch at 40
/// (`cut_preview`); this second cut sizes the view to a 24-row screen beside the transcript.
const BODY_LINES: usize = 10;

/// U12: fixed-height approval view — height is set at spawn so the live
/// region never jitters while the user decides.
pub struct ApprovalView {
    pub title: String,
    pub description: String,
    pub selected: usize,
    pub outcome: Option<AskChoice>,
    options: Vec<(AskChoice, String)>,
}

impl ApprovalView {
    pub fn new(title: String, description: String, grants: Vec<String>) -> Self {
        let always: Vec<(AskChoice, String)> = match grants.is_empty() {
            true => vec![(AskChoice::AllowAlways(0), "Always allow".to_owned())],
            false => grants
                .iter()
                .enumerate()
                .map(|(index, grant)| {
                    (
                        AskChoice::AllowAlways(index),
                        format!("Always allow {grant}"),
                    )
                })
                .collect(),
        };
        let mut options = vec![(AskChoice::AllowOnce, "Allow once".to_owned())];
        options.extend(always);
        options.push((AskChoice::Reject, "Reject".to_owned()));
        Self {
            title,
            description,
            selected: 0,
            outcome: None,
            options,
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
        // The first line is prose, the rest the tool's diff. `wrap_line` has no newline
        // handling, so it all flattened into one span and the diff was thrown away here.
        let mut prose = self
            .description
            .split('\n')
            .next()
            .unwrap_or_default()
            .to_owned();
        if prose.chars().count() > 200 {
            prose = prose.chars().take(200).collect::<String>() + "…";
        }
        out.extend(
            wrap_line(
                &Line::from(Span::styled(format!("   {prose}"), theme.muted_style())),
                width,
                "   ",
            )
            .into_iter()
            .take(2),
        );
        let body: Vec<&str> = self
            .description
            .split('\n')
            .skip(1)
            // The `--- a/x` / `+++ b/x` pair repeats the path the title already
            // carries, at two rows out of a ten-row budget.
            .filter(|line| !line.starts_with("--- ") && !line.starts_with("+++ "))
            .collect();
        for raw in body.iter().take(BODY_LINES) {
            let style = match raw.as_bytes().first() {
                Some(b'+') => Style::default().fg(theme.success),
                Some(b'-') => Style::default().fg(theme.error),
                Some(b'@') => Style::default().fg(theme.accent),
                _ => theme.dim_style(),
            };
            // Truncated, never wrapped: a wrapped diff line loses the column saying whether
            // it is an addition, and one long line could spend the whole budget.
            let text: String = raw.chars().take(width.saturating_sub(4)).collect();
            out.push(Line::from(Span::styled(format!("   {text}"), style)));
        }
        if body.len() > BODY_LINES {
            out.push(Line::from(Span::styled(
                format!("   … {} more lines", body.len().saturating_sub(BODY_LINES)),
                theme.dim_style(),
            )));
        }
        let mut options = Vec::new();
        for (i, (_, label)) in self.options.iter().enumerate() {
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
        let wide = options
            .iter()
            .map(|span| span.content.chars().count())
            .sum::<usize>()
            > width;
        match wide {
            true => out.extend(options.into_iter().map(Line::from)),
            false => out.push(Line::from(options)),
        }
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        match key.code {
            KeyCodeValue::Left | KeyCodeValue::Up => {
                self.selected = self.selected.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Right | KeyCodeValue::Down | KeyCodeValue::Tab => {
                if self.selected + 1 < self.options.len() {
                    self.selected += 1;
                }
                PopupResult::Open
            }
            KeyCodeValue::Enter => {
                self.outcome = self.options.get(self.selected).map(|(choice, _)| *choice);
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
                self.outcome = Some(AskChoice::AllowAlways(0));
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

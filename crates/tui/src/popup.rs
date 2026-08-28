use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};

#[derive(Debug)]
pub enum PopupResult {
    Open,
    Close,
    Insert(String),
}

/// U11: a bottom view renders in place of the composer and owns keys while open.
pub trait BottomView {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>>;
    fn handle_key(&mut self, key: &SingleKey) -> PopupResult;
}

pub struct ListPopup {
    pub prefix: char,
    pub query: String,
    pub items: Vec<String>,
    pub selected: usize,
}

const MAX_VISIBLE: usize = 8;

impl ListPopup {
    pub fn new(prefix: char, items: Vec<String>) -> Self {
        Self {
            prefix,
            query: String::new(),
            items,
            selected: 0,
        }
    }

    pub fn filtered(&self) -> Vec<&String> {
        self.items
            .iter()
            .filter(|item| item.to_lowercase().contains(&self.query.to_lowercase()))
            .take(100)
            .collect()
    }
}

impl BottomView for ListPopup {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let filtered = self.filtered();
        let mut out = vec![Line::from(vec![
            Span::styled(
                format!(" {}{}", self.prefix, self.query),
                Style::default().fg(theme.text),
            ),
            Span::styled("█", Style::default().fg(theme.accent)),
        ])];
        let start = self.selected.saturating_sub(MAX_VISIBLE - 1);
        for (i, item) in filtered.iter().enumerate().skip(start).take(MAX_VISIBLE) {
            let selected = i == self.selected;
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.muted_style()
            };
            let marker = if selected { "›" } else { " " };
            let mut text = format!(" {marker} {item}");
            text.truncate(width.saturating_sub(1));
            out.push(Line::from(Span::styled(text, style)));
        }
        if filtered.len() > MAX_VISIBLE {
            out.push(Line::from(Span::styled(
                format!("   … {} more", filtered.len() - MAX_VISIBLE),
                theme.dim_style(),
            )));
        }
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        let count = self.filtered().len();
        match key.code {
            KeyCodeValue::Esc => PopupResult::Close,
            KeyCodeValue::Up => {
                self.selected = self.selected.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Down => {
                if self.selected + 1 < count {
                    self.selected += 1;
                }
                PopupResult::Open
            }
            KeyCodeValue::Enter | KeyCodeValue::Tab => {
                // Completing to the selected item would drop the arguments.
                if self.prefix == '/' && self.query.contains(' ') {
                    return PopupResult::Insert(format!("{}{}", self.prefix, self.query));
                }
                match self.filtered().get(self.selected) {
                    Some(item) => PopupResult::Insert(format!("{}{}", self.prefix, item)),
                    None => PopupResult::Close,
                }
            }
            KeyCodeValue::Backspace => {
                if self.query.pop().is_none() {
                    return PopupResult::Close;
                }
                self.selected = 0;
                PopupResult::Open
            }
            KeyCodeValue::Char(c) if !key.ctrl && !key.alt => {
                self.query.push(c);
                self.selected = 0;
                PopupResult::Open
            }
            KeyCodeValue::Space => {
                self.query.push(' ');
                PopupResult::Open
            }
            _ => PopupResult::Open,
        }
    }
}

/// One walk, shared with the `glob` and `grep` tools.
pub fn walk_files(root: &std::path::Path, cap: usize) -> Vec<String> {
    yi_runtime::list_files(root, cap)
}

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

/// A bottom view renders in place of the composer and owns keys while open.
pub trait BottomView {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>>;
    fn handle_key(&mut self, key: &SingleKey) -> PopupResult;
    /// A view with a text field takes a paste there; one without swallows it, so a paste never
    /// lands in the composer hidden behind the view.
    fn paste(&mut self, _text: &str) {}
}

pub(crate) fn one_line(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub struct ListPopup {
    pub prefix: char,
    pub query: String,
    pub items: Vec<String>,
    pub selected: usize,
    pub note: Option<String>,
    /// The composer draws the text. A command list then shows only the matches.
    pub show_query: bool,
}

const MAX_VISIBLE: usize = 8;

impl ListPopup {
    pub fn new(prefix: char, items: Vec<String>) -> Self {
        Self {
            prefix,
            query: String::new(),
            items,
            selected: 0,
            note: None,
            show_query: true,
        }
    }

    /// Invariant: a query that is exactly one item's name selects that item, not a longer one
    /// it prefixes — `/plan` must not run `/plantree`, whatever order the table is in.
    pub fn filtered(&self) -> Vec<&String> {
        let query = self.query.to_lowercase();
        let mut found: Vec<&String> = self
            .items
            .iter()
            .filter(|item| item.to_lowercase().contains(&query))
            .collect();
        if let Some(exact) = found.iter().position(|item| item.to_lowercase() == query) {
            let hit = found.remove(exact);
            found.insert(0, hit);
        }
        found
    }

    /// Tab fills the highlighted command and leaves a space for arguments.
    /// A line that already has arguments stays as typed, and a file list does not use this.
    pub fn complete_tab(&mut self) -> bool {
        if self.prefix != '/' || self.query.contains(' ') {
            return false;
        }
        let Some(item) = self
            .filtered()
            .get(self.selected)
            .map(|item| (*item).clone())
        else {
            return false;
        };
        self.query = format!("{item} ");
        self.selected = 0;
        true
    }
}

impl BottomView for ListPopup {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let filtered = self.filtered();
        let mut out = Vec::new();
        if self.show_query {
            out.push(Line::from(vec![
                Span::styled(
                    format!(" {}{}", self.prefix, self.query),
                    Style::default().fg(theme.text),
                ),
                Span::styled("█", Style::default().fg(theme.accent)),
            ]));
        }
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
            let text = format!(" {marker} {item}");
            let text: String = text.chars().take(width.saturating_sub(1)).collect();
            out.push(Line::from(Span::styled(text, style)));
        }
        if filtered.len() > MAX_VISIBLE {
            out.push(Line::from(Span::styled(
                format!("   … {} more", filtered.len() - MAX_VISIBLE),
                theme.dim_style(),
            )));
        }
        if let Some(note) = &self.note {
            let note: String = format!("   {note}").chars().take(width).collect();
            out.push(Line::from(Span::styled(note, theme.dim_style())));
        }
        out
    }

    fn paste(&mut self, text: &str) {
        self.query.push_str(&one_line(text));
        self.selected = 0;
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
            KeyCodeValue::Tab if self.prefix == '/' => {
                let _filled = self.complete_tab();
                PopupResult::Open
            }
            KeyCodeValue::Enter | KeyCodeValue::Tab => {
                // Completing to the selected item would drop the arguments.
                let typo = key.code == KeyCodeValue::Enter && count == 0;
                if self.prefix == '/' && (self.query.contains(' ') || typo) {
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

/// One ignore engine, shared with the `glob` and `grep` tools.
pub use yi_runtime::Ignore;

#[cfg(test)]
mod tests {
    use super::{BottomView, ListPopup, PopupResult};
    use crate::keymap::{KeyCodeValue, SingleKey};

    fn key(code: KeyCodeValue) -> SingleKey {
        SingleKey {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn type_query(popup: &mut ListPopup, text: &str) {
        for character in text.chars() {
            popup.handle_key(&key(KeyCodeValue::Char(character)));
        }
    }

    #[test]
    fn tab_fills_the_command_and_leaves_room_for_arguments() {
        let mut popup = ListPopup::new('/', vec!["queue".to_owned(), "quit".to_owned()]);
        type_query(&mut popup, "que");
        assert!(matches!(
            popup.handle_key(&key(KeyCodeValue::Tab)),
            PopupResult::Open
        ));
        assert_eq!(popup.query, "queue ");
        type_query(&mut popup, "one more");
        match popup.handle_key(&key(KeyCodeValue::Enter)) {
            PopupResult::Insert(line) => assert_eq!(line, "/queue one more"),
            other => panic!("expected the typed command, got {other:?}"),
        }
    }

    #[test]
    fn tab_on_a_command_that_already_has_arguments_does_not_run_it() {
        let mut popup = ListPopup::new('/', vec!["queue".to_owned()]);
        type_query(&mut popup, "queue later");
        assert!(matches!(
            popup.handle_key(&key(KeyCodeValue::Tab)),
            PopupResult::Open
        ));
        assert_eq!(popup.query, "queue later");
    }
}

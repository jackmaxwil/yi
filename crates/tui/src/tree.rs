use std::collections::{HashMap, HashSet};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};

use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFilter {
    Default,
    UserOnly,
    All,
}

pub enum TreeResult {
    Open,
    Close,
    Rewind(String),
}

#[derive(Debug, Clone)]
struct Row {
    id: String,
    label: String,
    role: &'static str,
    depth: usize,
    is_last: bool,
    on_path: bool,
}

pub struct TreeView {
    rows: Vec<Row>,
    pub selected: usize,
    pub query: String,
    pub filter: TreeFilter,
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn entry_label(entry: &Entry) -> Option<(&'static str, String)> {
    match entry {
        Entry::Message { message, .. } => match message {
            AgentMessage::User { content, .. } => Some(("user", user_text(content))),
            AgentMessage::Assistant { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                Some(("assistant", text))
            }
            AgentMessage::ToolResult { tool_name, .. } => Some(("tool", format!("[{tool_name}]"))),
            AgentMessage::Custom { custom_type, .. } => {
                Some(("custom", format!("[{custom_type}]")))
            }
            _ => None,
        },
        Entry::Compaction { .. } => Some(("compaction", "[compaction]".to_owned())),
        _ => None,
    }
}

fn keep(role: &str, filter: TreeFilter) -> bool {
    match filter {
        TreeFilter::All => true,
        TreeFilter::UserOnly => role == "user",
        TreeFilter::Default => role == "user" || role == "assistant",
    }
}

impl TreeView {
    /// U31: flatten the Pi entry tree depth-first with connector metadata;
    /// the branch containing `leaf` is marked so the active path renders lit.
    pub fn new(entries: &[Entry], leaf: Option<&str>, filter: TreeFilter) -> Self {
        let mut children: HashMap<Option<&str>, Vec<&Entry>> = HashMap::new();
        for entry in entries {
            children.entry(entry.parent_id()).or_default().push(entry);
        }
        let mut on_path: HashSet<&str> = HashSet::new();
        let by_id: HashMap<&str, &Entry> =
            entries.iter().map(|entry| (entry.id(), entry)).collect();
        let mut cursor = leaf;
        while let Some(id) = cursor {
            on_path.insert(id);
            cursor = by_id.get(id).and_then(|entry| entry.parent_id());
        }
        let mut rows = Vec::new();
        let mut stack: Vec<(&Entry, usize, bool)> = children
            .get(&None)
            .map(|roots| {
                roots
                    .iter()
                    .rev()
                    .enumerate()
                    .map(|(i, entry)| (*entry, 0, i == 0))
                    .collect()
            })
            .unwrap_or_default();
        while let Some((entry, depth, is_last)) = stack.pop() {
            let id = entry.id();
            if let Some((role, text)) = entry_label(entry) {
                let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let mut label = compact;
                if label.chars().count() > 70 {
                    label = label.chars().take(70).collect::<String>() + "…";
                }
                rows.push(Row {
                    id: id.to_owned(),
                    label,
                    role,
                    depth,
                    is_last,
                    on_path: on_path.contains(id),
                });
            }
            if let Some(kids) = children.get(&Some(id)) {
                let forked = kids.len() > 1;
                for (i, kid) in kids.iter().rev().enumerate() {
                    let child_depth = if forked { depth + 1 } else { depth };
                    stack.push((kid, child_depth, i == 0));
                }
            }
        }
        let selected = rows
            .iter()
            .rposition(|row| row.on_path)
            .unwrap_or(rows.len().saturating_sub(1));
        Self {
            rows,
            selected,
            query: String::new(),
            filter,
        }
    }

    fn visible(&self) -> Vec<(usize, &Row)> {
        let needle = self.query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| keep(row.role, self.filter))
            .filter(|(_, row)| needle.is_empty() || row.label.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn lines(&self, width: usize, theme: &Theme, max_rows: usize) -> Vec<Line<'static>> {
        let visible = self.visible();
        let mut out = vec![Line::from(vec![
            Span::styled(
                " Session tree ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "· {} · tab filter · enter rewind · esc close  {}",
                    match self.filter {
                        TreeFilter::Default => "messages",
                        TreeFilter::UserOnly => "user only",
                        TreeFilter::All => "all",
                    },
                    self.query
                ),
                theme.muted_style(),
            ),
        ])];
        let position = visible
            .iter()
            .position(|(i, _)| *i == self.selected)
            .unwrap_or(0);
        let start = position.saturating_sub(max_rows.saturating_sub(1));
        for (i, row) in visible.iter().skip(start).take(max_rows) {
            let selected = *i == self.selected;
            let connector = if row.is_last { "└─ " } else { "├─ " };
            let indent = "│  ".repeat(row.depth);
            let glyph = match row.role {
                "user" => "› ",
                "assistant" => "",
                _ => "",
            };
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else if row.on_path {
                Style::default().fg(theme.text)
            } else {
                theme.muted_style()
            };
            let mut text = format!(" {indent}{connector}{glyph}{}", row.label);
            text.truncate(width.saturating_sub(1));
            out.push(Line::from(Span::styled(text, style)));
        }
        if visible.len() > max_rows {
            out.push(Line::from(Span::styled(
                format!("   … {} more", visible.len() - max_rows),
                theme.dim_style(),
            )));
        }
        out
    }

    pub fn handle_key(&mut self, key: &SingleKey) -> TreeResult {
        let visible: Vec<usize> = self.visible().iter().map(|(i, _)| *i).collect();
        let position = visible
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        match key.code {
            KeyCodeValue::Esc => TreeResult::Close,
            KeyCodeValue::Up => {
                if let Some(&index) = visible.get(position.saturating_sub(1)) {
                    self.selected = index;
                }
                TreeResult::Open
            }
            KeyCodeValue::Down => {
                if let Some(&index) = visible.get(position + 1) {
                    self.selected = index;
                }
                TreeResult::Open
            }
            KeyCodeValue::Tab => {
                self.filter = match self.filter {
                    TreeFilter::Default => TreeFilter::UserOnly,
                    TreeFilter::UserOnly => TreeFilter::All,
                    TreeFilter::All => TreeFilter::Default,
                };
                TreeResult::Open
            }
            KeyCodeValue::Enter => match self.rows.get(self.selected) {
                Some(row) => TreeResult::Rewind(row.id.clone()),
                None => TreeResult::Close,
            },
            KeyCodeValue::Backspace => {
                self.query.pop();
                TreeResult::Open
            }
            KeyCodeValue::Char(c) if !key.ctrl && !key.alt => {
                self.query.push(c);
                TreeResult::Open
            }
            _ => TreeResult::Open,
        }
    }
}

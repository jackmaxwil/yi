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

const PAGE: usize = 8;

const HELP: &str = "Enter: rewind. ↑/↓: move. Alt+↑/↓: previous/next turn. PgUp/PgDn: page. Home/End: first/last. Tab: filter. Type to search. Esc: close";

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
                let text = if text.trim().is_empty() {
                    "(no content)".to_owned()
                } else {
                    text
                };
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

    /// OMP `TreeSelectorComponent`: a titled panel of spacer / help / search /
    /// divider / spacer / rows / filter, each row `cursor + gutter + active-path
    /// bullet + role: text`, selection carried by a full-width background.
    pub fn lines(&self, width: usize, theme: &Theme, max_rows: usize) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(4);
        let mut out = vec![top_border(width, "Session Tree", theme)];
        out.push(row(Vec::new(), inner, theme, None));
        out.push(row(
            vec![Span::styled(HELP.to_owned(), theme.muted_style())],
            inner,
            theme,
            None,
        ));
        let mut search = vec![Span::styled("Search:".to_owned(), theme.muted_style())];
        if !self.query.is_empty() {
            search.push(Span::styled(
                format!(" {}", self.query),
                theme.accent_style(),
            ));
        }
        out.push(row(search, inner, theme, None));
        out.push(divider(width, theme));
        out.push(row(Vec::new(), inner, theme, None));
        for line in self.rows_lines(inner, theme, max_rows.max(1)) {
            out.push(row(line, inner, theme, None));
        }
        if let Some(label) = self.filter_label() {
            out.push(row(
                vec![Span::styled(label.to_owned(), theme.muted_style())],
                inner,
                theme,
                None,
            ));
        }
        out.push(row(Vec::new(), inner, theme, None));
        out.push(bottom_border(width, theme));
        out
    }

    fn filter_label(&self) -> Option<&'static str> {
        match self.filter {
            TreeFilter::Default => None,
            TreeFilter::UserOnly => Some("[user]"),
            TreeFilter::All => Some("[all]"),
        }
    }

    fn rows_lines(&self, inner: usize, theme: &Theme, max_rows: usize) -> Vec<Vec<Span<'static>>> {
        let visible = self.visible();
        if visible.is_empty() {
            return vec![vec![Span::styled(
                "No entries match".to_owned(),
                theme.muted_style(),
            )]];
        }
        let position = visible
            .iter()
            .position(|(i, _)| *i == self.selected)
            .unwrap_or(0);
        let (start, end) = centered_window(position, visible.len(), max_rows);
        let overflow = visible.len() > max_rows;
        let content_width = inner.saturating_sub(usize::from(overflow));
        let thumb = thumb_range(start, visible.len(), max_rows);
        let mut lines = Vec::new();
        for (offset, (index, entry)) in visible
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let selected = *index == self.selected;
            let background = selected.then(|| theme.selection_bg());
            let mut spans = vec![Span::styled(
                if selected { "› " } else { "  " }.to_owned(),
                paint(theme.accent_style(), background),
            )];
            let prefix = gutter_prefix(entry);
            if !prefix.is_empty() {
                spans.push(Span::styled(prefix, paint(theme.dim_style(), background)));
            }
            if entry.on_path {
                spans.push(Span::styled(
                    "● ".to_owned(),
                    paint(theme.accent_style(), background),
                ));
            }
            spans.push(Span::styled(
                role_prefix(entry.role),
                paint(role_style(entry.role, theme), background),
            ));
            spans.push(Span::styled(
                entry.label.clone(),
                paint(Style::default().fg(theme.text), background),
            ));
            let mut spans = fit_spans(spans, content_width, background);
            if overflow {
                let row_index = offset.saturating_sub(start);
                let in_thumb = row_index >= thumb.0 && row_index < thumb.1;
                let (glyph, style) = if in_thumb {
                    ("█", theme.accent_style())
                } else {
                    ("│", theme.muted_style())
                };
                spans.push(Span::styled(glyph.to_owned(), style));
            }
            lines.push(spans);
        }
        lines
    }

    /// Alt+↑/↓ steps whole turns: from anywhere in a turn to the user message
    /// that started the previous or next one (OMP `previous/next turn`).
    fn step_turn(&mut self, visible: &[usize], position: usize, forward: bool) {
        let mut cursor = position;
        loop {
            cursor = if forward {
                cursor.saturating_add(1)
            } else {
                match cursor.checked_sub(1) {
                    Some(next) => next,
                    None => return,
                }
            };
            let Some(&index) = visible.get(cursor) else {
                return;
            };
            if self.rows.get(index).is_some_and(|row| row.role == "user") {
                self.selected = index;
                return;
            }
        }
    }

    pub fn handle_key(&mut self, key: &SingleKey) -> TreeResult {
        let visible: Vec<usize> = self.visible().iter().map(|(i, _)| *i).collect();
        let position = visible
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        match key.code {
            KeyCodeValue::Esc => TreeResult::Close,
            KeyCodeValue::Up if key.alt => {
                self.step_turn(&visible, position, false);
                TreeResult::Open
            }
            KeyCodeValue::Down if key.alt => {
                self.step_turn(&visible, position, true);
                TreeResult::Open
            }
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
            KeyCodeValue::PageUp => {
                if let Some(&index) = visible.get(position.saturating_sub(PAGE)) {
                    self.selected = index;
                } else if let Some(&index) = visible.first() {
                    self.selected = index;
                }
                TreeResult::Open
            }
            KeyCodeValue::PageDown => {
                match visible.get(position.saturating_add(PAGE)) {
                    Some(&index) => self.selected = index,
                    None => {
                        if let Some(&index) = visible.last() {
                            self.selected = index;
                        }
                    }
                }
                TreeResult::Open
            }
            KeyCodeValue::Home => {
                if let Some(&index) = visible.first() {
                    self.selected = index;
                }
                TreeResult::Open
            }
            KeyCodeValue::End => {
                if let Some(&index) = visible.last() {
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

fn paint(style: Style, background: Option<ratatui::style::Color>) -> Style {
    match background {
        Some(color) => style.bg(color),
        None => style,
    }
}

fn role_prefix(role: &str) -> String {
    match role {
        "user" => "user: ".to_owned(),
        "assistant" => "assistant: ".to_owned(),
        _ => String::new(),
    }
}

fn role_style(role: &str, theme: &Theme) -> Style {
    match role {
        "user" => theme.accent_style(),
        "assistant" => Style::default().fg(theme.success),
        _ => theme.dim_style(),
    }
}

fn gutter_prefix(row: &Row) -> String {
    if row.depth == 0 {
        return String::new();
    }
    let connector = if row.is_last { "└─ " } else { "├─ " };
    format!("{}{connector}", "│  ".repeat(row.depth.saturating_sub(1)))
}

/// So a selection background covers the row end to end (OMP `fit`).
fn fit_spans(
    spans: Vec<Span<'static>>,
    width: usize,
    background: Option<ratatui::style::Color>,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0_usize;
    for span in spans {
        let span_width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        if used.saturating_add(span_width) <= width {
            used = used.saturating_add(span_width);
            out.push(span);
            continue;
        }
        let room = width.saturating_sub(used);
        if room > 0 {
            let text: String = span.content.chars().take(room).collect();
            used = used.saturating_add(unicode_width::UnicodeWidthStr::width(text.as_str()));
            out.push(Span::styled(text, span.style));
        }
        break;
    }
    if used < width {
        out.push(Span::styled(
            " ".repeat(width.saturating_sub(used)),
            paint(Style::default(), background),
        ));
    }
    out
}

fn row(
    spans: Vec<Span<'static>>,
    inner: usize,
    theme: &Theme,
    background: Option<ratatui::style::Color>,
) -> Line<'static> {
    let bar = || Span::styled("│".to_owned(), theme.dim_style());
    let mut out = vec![bar(), Span::raw(" ")];
    out.extend(fit_spans(spans, inner, background));
    out.push(Span::raw(" "));
    out.push(bar());
    Line::from(out)
}

fn top_border(width: usize, title: &str, theme: &Theme) -> Line<'static> {
    let inner = width.saturating_sub(2);
    let shown = format!(" {title} ");
    let fill = inner
        .saturating_sub(1)
        .saturating_sub(unicode_width::UnicodeWidthStr::width(shown.as_str()));
    Line::from(vec![
        Span::styled("╭─".to_owned(), theme.dim_style()),
        Span::styled(shown, theme.accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}╮", "─".repeat(fill)), theme.dim_style()),
    ])
}

fn divider(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("├{}┤", "─".repeat(width.saturating_sub(2))),
        theme.dim_style(),
    ))
}

fn bottom_border(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        theme.dim_style(),
    ))
}

/// OMP `centeredWindow`: keep the selection in the middle of the window
/// instead of only scrolling when it leaves the edge.
fn centered_window(selected: usize, total: usize, max_visible: usize) -> (usize, usize) {
    let start = selected
        .saturating_sub(max_visible / 2)
        .min(total.saturating_sub(max_visible));
    (start, start.saturating_add(max_visible).min(total))
}

fn thumb_range(scroll: usize, total: usize, height: usize) -> (usize, usize) {
    if height == 0 || total <= height {
        return (0, height);
    }
    let size = (height.saturating_mul(height) / total).clamp(1, height);
    let travel = height.saturating_sub(size);
    let max_offset = total.saturating_sub(height);
    let start = if max_offset == 0 {
        0
    } else {
        scroll.saturating_mul(travel) / max_offset
    };
    (start, start.saturating_add(size))
}

use std::collections::{BTreeMap, HashSet};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::TextArea;

struct HistorySearch {
    original: String,
    query: String,
    match_cursor: Option<usize>,
}

pub struct Composer {
    pub textarea: TextArea<'static>,
    pastes: BTreeMap<u32, String>,
    paste_counter: u32,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: Option<String>,
    search: Option<HistorySearch>,
}

impl Default for Composer {
    fn default() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        Self {
            textarea,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            history: Vec::new(),
            history_index: None,
            draft: None,
            search: None,
        }
    }
}

/// Paste sanitization: CRLF and CR become LF, tabs expand to three spaces, other controls
/// drop. NFC normalization is dropped — it needs a Unicode tables dep.
pub fn sanitize_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' => out.push_str("   "),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

const MARKER_LINES: usize = 10;
const MARKER_CHARS: usize = 1000;

impl Composer {
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.textarea.lines().iter().all(|l| l.trim().is_empty())
    }

    pub fn set_text(&mut self, text: &str) {
        self.textarea = TextArea::from(text.lines().map(str::to_owned).collect::<Vec<_>>());
        self.textarea
            .set_cursor_line_style(ratatui::style::Style::default());
        self.textarea.move_cursor(tui_textarea::CursorMove::Bottom);
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }

    /// Re-styled per draw so theme and running state stay current.
    pub fn set_frame(
        &mut self,
        border: ratatui::style::Style,
        placeholder: ratatui::style::Style,
        title: Option<String>,
    ) {
        use ratatui::widgets::{Block, BorderType, Borders};
        self.textarea.set_placeholder_style(placeholder);
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border);
        if let Some(title) = self.search_title().or(title) {
            block = block.title(title);
        }
        self.textarea.set_block(block);
    }

    pub fn search_active(&self) -> bool {
        self.search.is_some()
    }

    pub fn search_title(&self) -> Option<String> {
        let search = self.search.as_ref()?;
        Some(if search.query.is_empty() {
            "reverse-i-search".to_owned()
        } else if search.match_cursor.is_none() {
            format!("failing reverse-i-search: {}", search.query)
        } else {
            format!("reverse-i-search: {}", search.query)
        })
    }

    pub fn begin_search(&mut self) {
        if self.search.is_some() {
            self.search_older();
            return;
        }
        // Invariant: empty query never previews; match_cursor stays None until a hit.
        self.search = Some(HistorySearch {
            original: self.text(),
            query: String::new(),
            match_cursor: None,
        });
    }

    pub fn cancel_search(&mut self) -> bool {
        let Some(search) = self.search.take() else {
            return false;
        };
        self.set_text(&search.original);
        true
    }

    pub fn accept_search(&mut self) -> bool {
        if self
            .search
            .as_ref()
            .is_none_or(|s| s.match_cursor.is_none())
        {
            return false;
        }
        self.search = None;
        true
    }

    pub fn search_older(&mut self) {
        self.step_search(true);
    }

    pub fn search_newer(&mut self) {
        self.step_search(false);
    }

    pub fn handle_search_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Up => self.search_older(),
            KeyCode::Down => self.step_search(false),
            KeyCode::Char('s') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.step_search(false);
            }
            KeyCode::Backspace => self.edit_query(None),
            KeyCode::Char('h') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.edit_query(None);
            }
            KeyCode::Char(ch)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::ALT)
                    && !ch.is_control() =>
            {
                self.edit_query(Some(ch));
            }
            _ => {}
        }
    }

    fn edit_query(&mut self, push: Option<char>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        match push {
            Some(ch) => search.query.push(ch),
            None => {
                let _ = search.query.pop();
            }
        }
        self.refresh_search();
    }

    fn step_search(&mut self, older: bool) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        let hits = self.unique_matches(&search.query);
        let Some(last) = hits.len().checked_sub(1) else {
            return;
        };
        let next = match search.match_cursor {
            None => 0,
            Some(i) if older => i.saturating_add(1).min(last),
            Some(i) => i.saturating_sub(1),
        };
        if let Some(search) = self.search.as_mut() {
            search.match_cursor = Some(next);
        }
        self.apply_preview();
    }

    fn refresh_search(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let hits = self.unique_matches(&search.query);
        let cursor = (!search.query.is_empty() && !hits.is_empty()).then_some(0);
        if let Some(search) = self.search.as_mut() {
            search.match_cursor = cursor;
        }
        self.apply_preview();
    }

    fn unique_matches(&self, query: &str) -> Vec<usize> {
        let needle = query.to_lowercase();
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for (index, entry) in self.history.iter().enumerate().rev() {
            if entry.to_lowercase().contains(&needle) && seen.insert(entry.as_str()) {
                hits.push(index);
            }
        }
        hits
    }

    fn apply_preview(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let original = search.original.clone();
        let preview = search.match_cursor.and_then(|cursor| {
            let hits = self.unique_matches(&search.query);
            hits.get(cursor).and_then(|i| self.history.get(*i)).cloned()
        });
        self.set_text(preview.as_deref().unwrap_or(&original));
    }

    pub fn input(&mut self, event: KeyEvent) {
        self.history_index = None;
        self.textarea.input(event);
    }

    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
    }

    pub fn handle_paste(&mut self, raw: &str) {
        let text = sanitize_paste(raw);
        let lines = text.split('\n').count();
        if lines > MARKER_LINES || text.chars().count() > MARKER_CHARS {
            self.paste_counter += 1;
            let id = self.paste_counter;
            let marker = if lines > MARKER_LINES {
                format!("[Paste #{id}, +{lines} lines]")
            } else {
                format!("[Paste #{id}, {} chars]", text.chars().count())
            };
            self.pastes.insert(id, text);
            self.textarea.insert_str(&marker);
        } else {
            self.textarea.insert_str(&text);
        }
    }

    /// One pass, so replaced content is never rescanned and a pasted body
    /// keeps a marker label. Longer labels first so `#1` never shadows `#10`.
    pub fn expand_markers(&self, text: &str) -> String {
        let mut labels: Vec<(String, &str)> = Vec::new();
        for (id, content) in &self.pastes {
            let lines = content.split('\n').count();
            labels.push((format!("[Paste #{id}, +{lines} lines]"), content));
            labels.push((
                format!("[Paste #{id}, {} chars]", content.chars().count()),
                content,
            ));
            labels.push((format!("[Paste #{id}]"), content));
        }
        labels.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        if labels.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'outer: while !rest.is_empty() {
            for (label, content) in &labels {
                if let Some(stripped) = rest.strip_prefix(label.as_str()) {
                    out.push_str(content);
                    rest = stripped;
                    continue 'outer;
                }
            }
            let mut chars = rest.chars();
            if let Some(ch) = chars.next() {
                out.push(ch);
            }
            rest = chars.as_str();
        }
        out
    }

    /// Backspace deletes a whole paste marker when the cursor sits at its
    /// end (the atom rule: a marker never decays into stray text).
    pub fn backspace(&mut self) {
        let (row, col) = self.textarea.cursor();
        let line = self.textarea.lines().get(row).cloned().unwrap_or_default();
        let prefix: String = line.chars().take(col).collect();
        if prefix.ends_with(']')
            && let Some(start) = prefix.rfind("[Paste #")
        {
            let head = prefix.get(..start).unwrap_or_default();
            let token_len = prefix.chars().count().saturating_sub(head.chars().count());
            for _ in 0..token_len {
                self.textarea.delete_char();
            }
            return;
        }
        self.textarea.delete_char();
    }

    pub fn take_submission(&mut self) -> Option<String> {
        let text = self.text();
        if text.trim().is_empty() {
            return None;
        }
        let expanded = self.expand_markers(&text);
        self.history.push(text);
        self.history_index = None;
        self.search = None;
        self.pastes.clear();
        self.set_text("");
        Some(expanded)
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_index = match self.history_index {
            None => {
                self.draft = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_index = Some(next_index);
        if let Some(entry) = self.history.get(next_index).cloned() {
            self.set_text(&entry);
        }
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_index = None;
            let draft = self.draft.take().unwrap_or_default();
            self.set_text(&draft);
        } else {
            self.history_index = Some(index + 1);
            if let Some(entry) = self.history.get(index + 1).cloned() {
                self.set_text(&entry);
            }
        }
    }

    pub fn desired_height(&self) -> u16 {
        let lines = self.textarea.lines().len().clamp(1, 8);
        u16::try_from(lines).unwrap_or(1).saturating_add(2)
    }
}

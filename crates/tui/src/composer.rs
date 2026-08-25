use std::collections::BTreeMap;

use ratatui::crossterm::event::KeyEvent;
use tui_textarea::TextArea;

pub struct Composer {
    pub textarea: TextArea<'static>,
    pastes: BTreeMap<u32, String>,
    paste_counter: u32,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: Option<String>,
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
        }
    }
}

/// omp `#sanitizePastedText` (verbatim semantics): CRLF -> LF, tabs expanded
/// (omp's code expands to three spaces despite its four-space comment — the
/// code's behavior is the ported contract), control characters stripped
/// except newline. NFC normalization is dropped: it needs a Unicode tables
/// dep and macOS NFD drag-drops are the only known producer.
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

    /// omp `#expandPasteMarkers` (verbatim semantics): one pass, so replaced
    /// content is never rescanned — a pasted body containing another marker's
    /// label survives verbatim. Longer labels first so `#1` never shadows `#10`.
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
            let token_len = prefix.chars().count() - prefix[..start].chars().count();
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
        u16::try_from(lines).unwrap_or(1)
    }
}

use std::collections::{BTreeMap, HashSet};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use tui_textarea::{CursorMove, TextArea};
use unicode_width::UnicodeWidthChar;

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
    wrap_width: usize,
    top: usize,
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
            wrap_width: 80,
            top: 0,
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
        let up = match event.code {
            KeyCode::Up => true,
            KeyCode::Down => false,
            _ => {
                edit_key(&mut self.textarea, event);
                return;
            }
        };
        if event.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            if !event.modifiers.contains(KeyModifiers::SHIFT) {
                self.textarea.cancel_selection();
            } else if !self.textarea.is_selecting() {
                self.textarea.start_selection();
            }
            self.move_visual(up);
        } else {
            edit_key(&mut self.textarea, event);
        }
    }

    /// Up and Down walk wrapped rows at the same display column, not logical lines.
    fn move_visual(&mut self, up: bool) {
        let rows = wrap(self.textarea.lines(), self.wrap_width);
        let (line, col) = self.textarea.cursor();
        let at = cursor_row(&rows, line, col);
        let target = if up {
            at.checked_sub(1)
        } else {
            at.checked_add(1)
        };
        let (Some(&(from_line, from, _)), Some(&(to_line, start, end))) =
            (rows.get(at), target.and_then(|t| rows.get(t)))
        else {
            return;
        };
        let lines = self.textarea.lines();
        let x = width_of(lines.get(from_line), from, col);
        let text = lines.get(to_line).map(String::as_str).unwrap_or_default();
        let last = rows
            .get(target.unwrap_or_default().saturating_add(1))
            .is_none_or(|next| next.0 != to_line);
        // Invariant: only a line's last row owns the column at its end; on any other row
        // that column is the next row's first, so the walk stops one short.
        let limit = if last {
            end
        } else {
            end.saturating_sub(1).max(start)
        };
        let mut used = 0;
        let mut to = start;
        for ch in text.chars().skip(start).take(limit.saturating_sub(start)) {
            used += ch.width().unwrap_or(0);
            if used > x {
                break;
            }
            to += 1;
        }
        if let (Ok(row), Ok(col)) = (u16::try_from(to_line), u16::try_from(to)) {
            self.textarea.move_cursor(CursorMove::Jump(row, col));
        }
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

    /// Box height for a composer `width` cells wide in a pane `rows` tall, borders included.
    pub fn layout(&mut self, width: u16, rows: u16) -> u16 {
        // One cell of margin a side, a border a side, and one column kept for the cursor.
        self.wrap_width = usize::from(width.saturating_sub(5)).max(1);
        let wrapped = wrap(self.textarea.lines(), self.wrap_width);
        let rows = usize::from(rows);
        // Invariant: the box never takes more than 38.2% of the pane, the golden split.
        let cap = (rows.saturating_mul(382) / 1000).saturating_sub(2).max(1);
        let rest = if rows >= 24 { 3 } else { 1 };
        let want = wrapped.len().max(rest);
        let step = GROWTH
            .iter()
            .copied()
            .find(|step| *step >= want)
            .unwrap_or(want);
        let height = step.min(cap);
        let (line, col) = self.textarea.cursor();
        let cursor = cursor_row(&wrapped, line, col);
        self.top = self
            .top
            .min(cursor)
            .max(cursor.saturating_add(1).saturating_sub(height));
        u16::try_from(height).unwrap_or(1).saturating_add(2)
    }

    /// Paints the wrapped text with its cursor and selection; [`Composer::layout`] runs first.
    pub fn render(&self, area: Rect, buffer: &mut Buffer) {
        let inner = match self.textarea.block() {
            Some(block) => {
                let inner = block.inner(area);
                block.render(area, buffer);
                inner
            }
            None => area,
        };
        let lines = self.textarea.lines();
        let wrapped = wrap(lines, self.wrap_width);
        let (cursor_line, cursor_col) = self.textarea.cursor();
        let cursor = cursor_row(&wrapped, cursor_line, cursor_col);
        let selection = self.textarea.selection_range();
        let style = self.textarea.style();
        let cursor_style = self.textarea.cursor_style();
        let rows = wrapped.iter().enumerate().skip(self.top);
        for (y, (index, &(line, start, end))) in (inner.top()..inner.bottom()).zip(rows) {
            let text = lines.get(line).map(String::as_str).unwrap_or_default();
            let mut x = inner.left();
            let mut col = start;
            for ch in text.chars().skip(start).take(end.saturating_sub(start)) {
                let mut cell_style = style;
                if selection.is_some_and(|(from, to)| from <= (line, col) && (line, col) < to) {
                    cell_style = cell_style.add_modifier(Modifier::REVERSED);
                }
                if index == cursor && col == cursor_col {
                    cell_style = cell_style.patch(cursor_style);
                }
                let (next, _) = buffer.set_stringn(
                    x,
                    y,
                    ch.encode_utf8(&mut [0; 4]),
                    usize::from(inner.right().saturating_sub(x)),
                    cell_style,
                );
                x = next;
                col += 1;
            }
            if index == cursor
                && cursor_col == end
                && let Some(cell) = buffer.cell_mut((x, y))
            {
                cell.set_symbol(" ").set_style(style.patch(cursor_style));
            }
        }
    }
}

/// Invariant: each step is about the golden ratio past the last, so a long paste moves the
/// layout a handful of times rather than once per row.
const GROWTH: [usize; 10] = [1, 2, 3, 5, 8, 13, 21, 34, 55, 89];

/// Incident: the textarea counted logical lines, so a long paragraph sat on one row and
/// scrolled sideways. Rows here are `(line, start char, end char)`, word-wrapped by display width.
fn wrap(lines: &[String], width: usize) -> Vec<(usize, usize, usize)> {
    let mut rows = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let (mut start, mut used, mut space) = (0, 0, None);
        for (at, ch) in text.chars().enumerate() {
            let cells = ch.width().unwrap_or(0);
            if used + cells > width && at > start {
                let cut = space.filter(|&cut| cut > start).unwrap_or(at);
                rows.push((line, start, cut));
                used = text
                    .chars()
                    .skip(cut)
                    .take(at - cut)
                    .filter_map(|c| c.width())
                    .sum();
                start = cut;
                space = None;
            }
            used += cells;
            if ch == ' ' {
                space = Some(at + 1);
            }
        }
        rows.push((line, start, text.chars().count()));
    }
    rows
}

fn cursor_row(rows: &[(usize, usize, usize)], line: usize, col: usize) -> usize {
    rows.iter()
        .rposition(|&(l, start, _)| l == line && start <= col)
        .unwrap_or(0)
}

fn width_of(text: Option<&String>, from: usize, to: usize) -> usize {
    text.map_or(0, |text| {
        text.chars()
            .skip(from)
            .take(to.saturating_sub(from))
            .filter_map(|c| c.width())
            .sum()
    })
}

/// Incident: Ghostty sends Cmd+Delete as ctrl+u, which the textarea took for undo. Shell and
/// macOS editing keys are bound here before the textarea's own emacs table; true when handled.
pub fn edit_key(textarea: &mut TextArea<'_>, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let cmd = key.modifiers.contains(KeyModifiers::SUPER);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let jump = |textarea: &mut TextArea<'_>, to| {
        textarea.move_cursor(to);
        true
    };
    match key.code {
        KeyCode::Char('u') if ctrl => clear_line(textarea),
        KeyCode::Backspace if cmd => clear_line(textarea),
        KeyCode::Backspace if ctrl => textarea.delete_word(),
        KeyCode::Delete if ctrl => textarea.delete_next_word(),
        KeyCode::Char('z' | 'Z') if ctrl && (shift || key.code == KeyCode::Char('Z')) => {
            textarea.redo()
        }
        KeyCode::Char('u' | 'U') if cmd && (shift || key.code == KeyCode::Char('U')) => {
            textarea.redo()
        }
        KeyCode::Char('z') if ctrl => textarea.undo(),
        KeyCode::Char('u') if cmd => textarea.undo(),
        KeyCode::Char('j') if ctrl => {
            textarea.insert_newline();
            true
        }
        KeyCode::Enter if alt => {
            textarea.insert_newline();
            true
        }
        KeyCode::Left if alt => jump(textarea, CursorMove::WordBack),
        KeyCode::Right if alt => jump(textarea, CursorMove::WordForward),
        KeyCode::Left if cmd => jump(textarea, CursorMove::Head),
        KeyCode::Right if cmd => jump(textarea, CursorMove::End),
        KeyCode::Up if cmd => jump(textarea, CursorMove::Top),
        KeyCode::Down if cmd => jump(textarea, CursorMove::Bottom),
        // The textarea pages on these emacs keys, a jump nobody expects from a prompt.
        KeyCode::Char('v') if ctrl || alt => false,
        _ => textarea.input(key),
    }
}

/// Clears the cursor's line, as zsh's ctrl+u does; an empty line is left, never joined.
fn clear_line(textarea: &mut TextArea<'_>) -> bool {
    textarea.cancel_selection();
    textarea.move_cursor(CursorMove::Head);
    let (line, _) = textarea.cursor();
    textarea
        .lines()
        .get(line)
        .is_some_and(|text| !text.is_empty())
        && textarea.delete_line_by_end()
}

impl crate::app::App {
    /// Invariant: a quote lands whole on its own lines; a long one never folds into a paste marker.
    pub fn quote(&mut self, text: &str) {
        let quoted: String = sanitize_paste(text)
            .lines()
            .map(|line| format!("> {line}\n"))
            .collect();
        if self.composer.textarea.cursor().1 != 0 {
            self.composer.textarea.insert_newline();
        }
        self.composer.textarea.insert_str(quoted);
    }
}

use std::io::Write;
use std::path::Path;
use std::time::{Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use tui_textarea::{CursorMove, TextArea};

use crate::keys::Action;
use crate::model::{Editor, PaneContent};

use super::App;

const MAX_EDITOR_BYTES: u64 = 8 * 1024 * 1024;
/// Invariant: the text area's cursor jump takes a u16 row, so a longer file is refused.
const MAX_EDITOR_LINES: usize = 65_535;
const DISK_POLL: std::time::Duration = std::time::Duration::from_secs(1);

fn mtime_of(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(super) fn load(path: &str) -> Result<Editor, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{path}: {error}"))?
        .len();
    if size > MAX_EDITOR_BYTES {
        return Err(format!(
            "{path}: {} MB is over the 8 MB editor limit",
            size / 1_048_576
        ));
    }
    let source = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let lines = source.lines().count();
    if lines > MAX_EDITOR_LINES {
        return Err(format!(
            "{path}: {lines} lines is over the 65,535-line editor limit"
        ));
    }
    let mut text = TextArea::from(source.lines().map(str::to_owned).collect::<Vec<_>>());
    text.set_cursor_line_style(ratatui::style::Style::default());
    Ok(Editor {
        path: path.to_owned(),
        text: Box::new(text),
        dirty: false,
        mtime: mtime_of(path),
        stale: false,
        scroll_top: 0,
        primed: None,
        last_cursor: (0, 0),
        lang: Path::new(path)
            .extension()
            .map(|ext| ext.to_string_lossy().into_owned()),
        // Invariant: a file mixing endings saves with the one most of its lines use.
        crlf: source.matches("\r\n").count().saturating_mul(2) > source.matches('\n').count(),
        final_newline: source.ends_with('\n'),
    })
}

impl Editor {
    fn contents(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self.text.lines().join(eol);
        if self.final_newline {
            out.push_str(eol);
        }
        out
    }

    fn save(&mut self) -> Result<String, String> {
        const STALE: &str = "file changed on disk · r reloads · k keeps yours";
        if self.stale {
            return Err(STALE.to_owned());
        }
        if mtime_of(&self.path) != self.mtime {
            self.stale = true;
            return Err(STALE.to_owned());
        }
        let fail = |error: std::io::Error| format!("save {}: {error}", short(&self.path));
        let target = std::fs::canonicalize(&self.path).map_err(fail)?;
        let mut tmp = target.clone().into_os_string();
        tmp.push(".yi-tmp");
        let meta = std::fs::metadata(&target).map_err(fail)?;
        // An empty file has no ending to keep; once typed into, it ends on a newline.
        self.final_newline |= meta.len() == 0 && self.text.lines() != [""];
        let body = self.contents();
        let written = std::fs::File::create(&tmp).and_then(|mut file| {
            file.write_all(body.as_bytes())?;
            file.set_permissions(meta.permissions())?;
            file.sync_all()?;
            std::fs::rename(&tmp, &target)
        });
        if let Err(error) = written {
            let _leftover = std::fs::remove_file(&tmp);
            return Err(fail(error));
        }
        self.dirty = false;
        self.mtime = mtime_of(&self.path);
        Ok(format!("saved {}", short(&self.path)))
    }

    fn reload(&mut self) -> Result<(), String> {
        let fresh = load(&self.path)?;
        let (row, col) = self.text.cursor();
        self.text = fresh.text;
        self.crlf = fresh.crlf;
        self.final_newline = fresh.final_newline;
        self.text.move_cursor(CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        ));
        self.mtime = fresh.mtime;
        self.dirty = false;
        self.stale = false;
        self.primed = None;
        Ok(())
    }

    fn check_disk(&mut self) -> Result<(), String> {
        if mtime_of(&self.path) == self.mtime {
            return Ok(());
        }
        if self.dirty {
            self.stale = true;
            return Ok(());
        }
        self.reload()
    }

    fn key(&mut self, key: KeyEvent) -> Option<String> {
        if self.stale {
            match key.code {
                KeyCode::Char('r') => return self.reload().err().or(Some("reloaded".to_owned())),
                KeyCode::Char('k') => {
                    self.stale = false;
                    self.mtime = mtime_of(&self.path);
                }
                _ => {}
            }
            return None;
        }
        let before = self.text.cursor().0;
        if yi_tui::composer::edit_key(&mut self.text, key) {
            self.dirty = true;
            // Only an edit above the viewport can change what the visible rows sit inside.
            let touched = before.min(self.text.cursor().0);
            if touched < self.scroll_top {
                self.primed = None;
            }
        }
        None
    }

    pub(super) fn jump(&mut self, row: usize, col: usize) {
        self.text.move_cursor(CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        ));
    }

    pub(super) fn click(&mut self, row: usize, col: usize) {
        if self.text.is_selecting() {
            self.text.cancel_selection();
        }
        self.jump(row, col);
        self.text.start_selection();
    }

    pub(super) fn drag_to(&mut self, row: usize, col: usize) {
        self.jump(row, col);
    }

    /// The drag ends: an empty one leaves no selection, a real one is the copy, and the
    /// selection stays live so a following keystroke still replaces it.
    pub(super) fn release(&mut self) -> Option<String> {
        let ((start_row, start_col), (end_row, end_col)) = self.text.selection_range()?;
        if (start_row, start_col) == (end_row, end_col) {
            self.text.cancel_selection();
            return None;
        }
        let lines = self.text.lines();
        let width = end_col.saturating_sub(start_col);
        if start_row == end_row {
            let line = lines.get(start_row)?;
            return Some(line.chars().skip(start_col).take(width).collect());
        }
        let mut text: String = lines.get(start_row)?.chars().skip(start_col).collect();
        for row in start_row.saturating_add(1)..end_row {
            text.push('\n');
            text.push_str(lines.get(row)?);
        }
        text.push('\n');
        text.extend(lines.get(end_row)?.chars().take(end_col));
        Some(text)
    }
}

impl App {
    pub(super) fn open_editor(&mut self, path: &str) {
        match load(path) {
            Ok(editor) => {
                if let Some(pane) = self.state.focused_pane_mut() {
                    pane.content = PaneContent::Editor(editor);
                    pane.scroll_from_bottom = 0;
                }
            }
            Err(error) => self.note(&error),
        }
        self.dirty = true;
    }

    pub(super) fn editor_mut(&mut self) -> Option<&mut Editor> {
        match &mut self.state.focused_pane_mut()?.content {
            PaneContent::Editor(editor) => Some(editor),
            _ => None,
        }
    }

    pub(super) fn on_editor(&self) -> bool {
        self.state
            .focused_pane_id()
            .and_then(|id| self.state.panes.get(&id))
            .is_some_and(|pane| matches!(pane.content, PaneContent::Editor(_)))
    }

    pub(super) fn editor_key(&mut self, key: KeyEvent) {
        if let Some(note) = self.editor_mut().and_then(|editor| editor.key(key)) {
            self.note(&note);
        }
        self.dirty = true;
    }

    pub(super) fn editor_action(&mut self, action: Action) -> bool {
        let Some(editor) = self.editor_mut() else {
            return false;
        };
        let note = match action {
            Action::Save => Some(editor.save().unwrap_or_else(|error| error)),
            Action::Undo => (!editor.text.undo()).then(|| "nothing to undo".to_owned()),
            Action::Redo => (!editor.text.redo()).then(|| "nothing to redo".to_owned()),
            _ => return false,
        };
        if matches!(action, Action::Undo | Action::Redo) && note.is_none() {
            editor.dirty = true;
        }
        if let Some(note) = note {
            self.note(&note);
        }
        self.dirty = true;
        true
    }

    pub(super) fn find_in_editor(&mut self, pattern: &str) {
        let Some(editor) = self.editor_mut() else {
            return self.note("no editor pane is focused");
        };
        let (row, col) = editor.text.cursor();
        let lines = editor.text.lines();
        let rows = lines.iter().enumerate();
        let after = rows.clone().skip(row).map(|(index, line)| {
            let from = if index == row {
                col.saturating_add(1)
            } else {
                0
            };
            (index, line, from)
        });
        let wrapped = rows
            .take(row.saturating_add(1))
            .map(|(i, line)| (i, line, 0));
        let hit = after.chain(wrapped).find_map(|(index, line, from)| {
            let skip: usize = line.chars().take(from).map(char::len_utf8).sum();
            let at = skip.checked_add(line.get(skip..)?.find(pattern)?)?;
            Some((index, line.get(..at)?.chars().count()))
        });
        let outcome = match hit {
            Some((line, column)) if !pattern.is_empty() => {
                editor.jump(line, column);
                None
            }
            _ => Some(format!("{pattern}: no match")),
        };
        if let Some(note) = outcome {
            self.note(&note);
        }
        self.dirty = true;
    }

    pub(super) fn check_editors(&mut self, now: Instant) {
        if now < self.next_disk_check {
            return;
        }
        self.next_disk_check = now + DISK_POLL;
        self.editors_touched(None);
    }

    pub(super) fn editors_touched(&mut self, path: Option<&str>) {
        let mut notes = Vec::new();
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Editor(editor) = &mut pane.content
                && path.is_none_or(|path| path == editor.path)
                && let Err(error) = editor.check_disk()
            {
                notes.push(error);
            }
        }
        for note in notes {
            self.note(&note);
        }
        self.dirty = true;
    }
}

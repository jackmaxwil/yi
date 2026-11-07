use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};

use crate::files::{Files, tree_rows};

use super::App;
use super::MouseKind;

impl App {
    /// Closed opens and focuses the tree, open and unfocused takes its keys, open and focused closes.
    pub(super) fn toggle_files(&mut self) {
        match &mut self.state.files {
            Some(files) if files.focused => self.state.files = None,
            Some(files) => files.focused = true,
            None => {
                let fits = self.hits.as_ref().is_some_and(|hits| {
                    let open = hits.area.width.saturating_sub(hits.sidebar_width) >= 60;
                    crate::files::side(hits.area, open).width > 0
                });
                if fits {
                    let root = self.session_dir();
                    self.state.files = Some(Files::open(&root));
                }
            }
        }
        self.dirty = true;
    }

    pub(crate) fn files_focused(&self) -> bool {
        self.hits.as_ref().is_some_and(|hits| hits.files.is_some())
            && self.state.files.as_ref().is_some_and(|files| files.focused)
    }

    fn files_rect(&self) -> Rect {
        self.hits
            .as_ref()
            .and_then(|hits| hits.files)
            .unwrap_or(Rect::new(0, 0, 0, 20))
    }

    /// A folder row folds; a file opens in a panel beside the panes and the tree stays up.
    fn files_activate(&mut self) {
        let side = self.files_rect();
        let mut opening: Option<(PathBuf, String)> = None;
        if let Some(files) = &mut self.state.files {
            if files.toggle() {
                let rows = tree_rows(files, side);
                files.scrolled(rows);
                self.dirty = true;
                return;
            }
            opening = files
                .current()
                .map(|node| (node.path().to_path_buf(), files.root().to_owned()));
        }
        let Some((path, workdir)) = opening else {
            return;
        };
        let Some(path) = path.to_str().map(str::to_owned) else {
            self.note("[cannot open this file: its name is not valid UTF-8]");
            return;
        };
        if let Some(files) = &mut self.state.files {
            files.focused = false;
        }
        self.state.focus_panes();
        self.open_editor(&path, workdir, true);
    }

    pub(crate) fn files_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            self.files_activate();
            return;
        }
        let side = self.files_rect();
        let Some(files) = &mut self.state.files else {
            return;
        };
        let rows = tree_rows(files, side);
        match key.code {
            KeyCode::Esc => {
                self.state.files = None;
                self.dirty = true;
                return;
            }
            KeyCode::Up => files.step(false),
            KeyCode::Down => files.step(true),
            KeyCode::Left => files.collapse(),
            KeyCode::Right => files.expand(),
            KeyCode::PageUp => files.page(rows, false),
            KeyCode::PageDown => files.page(rows, true),
            KeyCode::Home => files.edge(false),
            KeyCode::End => files.edge(true),
            KeyCode::Char('r') => files.refresh(),
            _ => {}
        }
        files.scrolled(rows);
        self.dirty = true;
    }

    /// True when the pointer is over the sidebar and it took the event; a press anywhere else
    /// only hands the keys back to the panes and goes on to whatever it landed on.
    pub(super) fn files_mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> bool {
        let Some(side) = self.hits.as_ref().and_then(|hits| hits.files) else {
            return false;
        };
        let Some(files) = &mut self.state.files else {
            return false;
        };
        if !side.contains(Position::new(x, y)) {
            let press = matches!(kind, MouseKind::Down | MouseKind::RightDown);
            self.dirty |= files.hover(None) || (press && std::mem::take(&mut files.focused));
            return false;
        }
        match kind {
            MouseKind::Down => {
                if y == side.y {
                    return false;
                }
                files.focused = true;
                if let Some(index) = files.pointed(side, x, y) {
                    files.select(index);
                    self.files_activate();
                }
            }
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let rows = tree_rows(files, side);
                files.wheel(kind == MouseKind::ScrollDown, rows);
            }
            MouseKind::Moved => {
                let over = files.pointed(side, x, y);
                self.dirty |= files.hover(over);
                return true;
            }
            MouseKind::Up | MouseKind::Drag | MouseKind::RightDown => {}
        }
        self.dirty = true;
        true
    }
}

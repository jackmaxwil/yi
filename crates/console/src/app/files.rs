use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::files::{Files, tree_rows};
use crate::model::Mode;

use super::App;
use super::MouseKind;

impl App {
    pub(super) fn open_files(&mut self) {
        let root = self.session_dir();
        self.state.mode = Mode::Files(Files::open(&root));
        self.dirty = true;
    }

    fn overlay_height(&self) -> u16 {
        self.hits
            .as_ref()
            .and_then(|hits| hits.overlays.first())
            .map_or(20, |rect| rect.height)
    }

    fn close_files(&mut self) {
        self.state.mode = Mode::Normal;
        self.dirty = true;
    }

    /// A folder row folds; a file hands its path to the editor and closes the tree.
    fn files_activate(&mut self) {
        let height = self.overlay_height();
        let mut opening: Option<(PathBuf, String)> = None;
        if let Mode::Files(files) = &mut self.state.mode {
            if files.toggle() {
                let rows = tree_rows(files, height);
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
        self.state.mode = Mode::Normal;
        self.state.focus_panes();
        self.open_editor(&path, workdir);
    }

    pub(super) fn files_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            self.files_activate();
            return;
        }
        let height = self.overlay_height();
        let Mode::Files(files) = &mut self.state.mode else {
            return;
        };
        let rows = tree_rows(files, height);
        match key.code {
            KeyCode::Esc => return self.close_files(),
            KeyCode::Up => files.step(false),
            KeyCode::Down => files.step(true),
            KeyCode::Left => files.collapse(),
            KeyCode::Right => files.expand(),
            KeyCode::PageUp => files.page(rows, false),
            KeyCode::PageDown => files.page(rows, true),
            KeyCode::Home => files.edge(false),
            KeyCode::End => files.edge(true),
            KeyCode::Char('r') => {
                let root = files.root().to_owned();
                *files = Files::open(&root);
            }
            _ => {}
        }
        files.scrolled(rows);
        self.dirty = true;
    }

    pub(super) fn files_mouse(&mut self, kind: MouseKind, x: u16, y: u16) {
        if self.overlay_press(kind, x, y) {
            return;
        }
        let Some(popup) = self
            .hits
            .as_ref()
            .and_then(|hits| hits.overlays.first().copied())
        else {
            return;
        };
        let height = self.overlay_height();
        let clicked = match &self.state.mode {
            Mode::Files(files) if kind == MouseKind::Down => files.pointed(popup, x, y, height),
            Mode::Normal
            | Mode::Prefix
            | Mode::Navigator { .. }
            | Mode::Files(_)
            | Mode::Menu(_) => None,
        };
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let down = kind == MouseKind::ScrollDown;
                if let Mode::Files(files) = &mut self.state.mode {
                    let rows = tree_rows(files, height);
                    files.wheel(down, rows);
                }
                self.dirty = true;
            }
            MouseKind::Down => {
                let Some(index) = clicked else { return };
                if let Mode::Files(files) = &mut self.state.mode {
                    files.select(index);
                }
                self.dirty = true;
                self.files_activate();
            }
            MouseKind::Moved | MouseKind::Up | MouseKind::Drag | MouseKind::RightDown => {}
        }
    }
}

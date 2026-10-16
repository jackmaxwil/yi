use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;

use crate::model::{PaneContent, Zone};
use crate::pty::Pty;

use super::App;

fn shell() -> String {
    let fallback = if Path::new("/bin/zsh").is_file() {
        "/bin/zsh"
    } else {
        "/bin/sh"
    };
    std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_file())
        .unwrap_or_else(|| fallback.to_owned())
}

impl App {
    fn focused_pty(&mut self) -> Option<&mut Pty> {
        match &mut self.state.focused_pane_mut()?.content {
            PaneContent::Terminal(pty) if !pty.exited() => Some(pty),
            _ => None,
        }
    }

    fn terminal_cwd(&self) -> String {
        let session = self.state.focused_session();
        let workdir = session
            .as_ref()
            .and_then(|id| self.state.chat(id))
            .map(|chat| chat.app.cwd().to_owned());
        let root = session
            .as_ref()
            .and_then(|id| self.state.sessions.get(id))
            .map(|row| row.root.clone());
        workdir
            .into_iter()
            .chain(root)
            .find(|dir| Path::new(dir).is_dir())
            .unwrap_or_else(|| self.state.root.clone())
    }

    pub(super) fn open_terminal(&mut self) {
        let cwd = self.terminal_cwd();
        let spawned = std::env::current_exe()
            .and_then(|exe| Pty::spawn(&exe, &shell(), &cwd, self.wake.clone()));
        let pty = match spawned {
            Ok(pty) => pty,
            Err(error) => return self.note(&format!("terminal: {error}")),
        };
        let home = self.state.focused_pane_id();
        self.split_with_anim(Direction::Horizontal);
        let opened = self.state.focused_pane_id().filter(|id| Some(*id) != home);
        if let Some(pane) = opened.and_then(|id| self.state.panes.get_mut(&id)) {
            pane.content = PaneContent::Terminal(Box::new(pty));
            self.state.zone = Zone::Panes;
        }
    }

    /// A live terminal takes every key but ⌘ chords and ⌥ arrows, which stay the console's
    /// (ctrl+b's table is matched before this).
    pub(super) fn terminal_key(&mut self, key: &KeyEvent) -> bool {
        let arrow = matches!(
            key.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        );
        if key.modifiers.contains(KeyModifiers::SUPER)
            || (arrow && key.modifiers.contains(KeyModifiers::ALT))
        {
            return false;
        }
        let Some(pty) = self.focused_pty() else {
            return false;
        };
        let bytes = crate::pty::encode(key, pty.application_cursor());
        let sent = pty.send(&bytes);
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.scroll_from_bottom = 0;
        }
        if let Err(error) = sent {
            self.note(&format!("terminal: {error}"));
        }
        self.dirty = true;
        true
    }

    pub(super) fn terminal_paste(&mut self, text: &str) -> bool {
        let Some(pty) = self.focused_pty() else {
            return false;
        };
        if let Err(error) = pty.paste(text) {
            self.note(&format!("terminal: {error}"));
        }
        true
    }

    pub(super) fn poll_terminals(&mut self) {
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Terminal(pty) = &mut pane.content {
                self.dirty |= pty.poll();
            }
        }
    }
}

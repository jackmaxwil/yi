use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;

use crate::model::PaneContent;
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

    pub(super) fn session_dir(&self) -> String {
        let session = match self.state.focused_pane().map(|pane| &pane.content) {
            Some(
                PaneContent::Editor(crate::model::Editor { workdir, .. })
                | PaneContent::Markdown { workdir, .. }
                | PaneContent::Diff { workdir, .. },
            ) => return workdir.clone(),
            Some(PaneContent::SessionDiff { session, .. }) => Some(session.clone()),
            _ => self.state.focused_session(),
        };
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
        let cwd = self.session_dir();
        let spawned = std::env::current_exe()
            .and_then(|exe| Pty::spawn(&exe, &shell(), &cwd, self.wake.clone()));
        let pty = match spawned {
            Ok(pty) => pty,
            Err(error) => return self.note(&format!("terminal: {error}")),
        };
        let home = self.state.focused_pane_id();
        self.split_share(Direction::Horizontal, 0.5);
        let opened = self.state.focused_pane_id().filter(|id| Some(*id) != home);
        if let Some(pane) = opened.and_then(|id| self.state.panes.get_mut(&id)) {
            pane.content = PaneContent::Terminal(Box::new(pty));
            self.state.focus_panes();
        }
    }

    /// A shell block the person pressed Run on goes to a live terminal pane of this tab, or to a
    /// new one, as typed text they can watch run; the agent never sees it.
    pub(super) fn run_in_terminal(&mut self, code: &str) {
        let live = |app: &Self, id: &crate::layout::PaneId| {
            matches!(
                app.state.panes.get(id).map(|pane| &pane.content),
                Some(PaneContent::Terminal(pty)) if !pty.exited()
            )
        };
        let mut open = self
            .state
            .tab()
            .and_then(|tab| tab.layout.pane_ids().into_iter().find(|id| live(self, id)));
        if open.is_none() {
            self.open_terminal();
            open = self.state.focused_pane_id().filter(|id| live(self, id));
        }
        let Some(id) = open else { return };
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(id);
        }
        self.state.focus_panes();
        if let Some(PaneContent::Terminal(pty)) =
            self.state.panes.get_mut(&id).map(|p| &mut p.content)
        {
            let typed = pty
                .paste(code.trim_end_matches('\n'))
                .and_then(|()| pty.send(b"\r"));
            if let Err(error) = typed {
                self.note(&format!("terminal: {error}"));
            }
        }
        self.dirty = true;
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

#[cfg(test)]
mod tests {
    use super::App;
    use crate::model::PaneContent;

    /// Dies with a Run that lands in the chat's composer instead of the shell, that types a
    /// command and never presses enter, or that leaves the terminal unfocused so the person
    /// cannot answer a prompt the command raises.
    #[test]
    fn a_shell_block_is_typed_into_the_live_terminal_and_entered()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        let chat = app.state.focused_pane_id().ok_or("a pane")?;
        let shell = app
            .state
            .split_focused(ratatui::layout::Direction::Horizontal)
            .ok_or("no split")?;
        let (pty, written) = crate::pty::tests::queued_terminal()?;
        if let Some(pane) = app.state.panes.get_mut(&shell) {
            pane.content = PaneContent::Terminal(Box::new(pty));
        }
        if let Some(tab) = app.state.tab_mut() {
            tab.layout.focus_pane(chat);
        }
        let code = "cd ~/Development/yi && git pull --ff-only origin main && \\\n  scripts/build_dist.sh\n";
        app.run_in_terminal(code);
        let typed: Vec<u8> = written.try_iter().flatten().collect();
        assert_eq!(
            String::from_utf8_lossy(&typed),
            "cd ~/Development/yi && git pull --ff-only origin main && \\\n  scripts/build_dist.sh\r"
        );
        assert_eq!(app.state.focused_pane_id(), Some(shell));
        Ok(())
    }
}

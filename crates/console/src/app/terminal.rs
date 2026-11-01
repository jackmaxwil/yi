use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;

use crate::layout::PaneId;
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

    fn live_terminal(&self, pane: PaneId) -> bool {
        self.state.tab().is_some_and(|tab| {
            tab.layout.pane_ids().contains(&pane)
                && matches!(
                    self.state.panes.get(&pane).map(|pane| &pane.content),
                    Some(PaneContent::Terminal(pty)) if !pty.exited()
                )
        })
    }

    fn chip_terminal(&self) -> Option<PaneId> {
        self.state.tab()?.chip_terminal
    }

    pub(super) fn dir_of(&self, pane: PaneId) -> String {
        let session = match self.state.panes.get(&pane).map(|pane| &pane.content) {
            Some(
                PaneContent::Editor(crate::model::Editor { workdir, .. })
                | PaneContent::Markdown { workdir, .. }
                | PaneContent::Diff { workdir, .. },
            ) => return workdir.clone(),
            Some(PaneContent::SessionDiff { session, .. }) => Some(session.clone()),
            _ => self
                .state
                .panes
                .get(&pane)
                .and_then(|pane| pane.session())
                .cloned(),
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

    pub(super) fn session_dir(&self) -> String {
        let Some(pane) = self.state.focused_pane_id() else {
            return self.state.root.clone();
        };
        self.dir_of(pane)
    }

    pub(super) fn open_terminal(&mut self) {
        if let Some(pane) = self.state.focused_pane_id() {
            self.open_terminal_beside(pane);
        }
    }

    fn open_terminal_beside(&mut self, from: PaneId) -> Option<PaneId> {
        let cwd = self.dir_of(from);
        let spawned = std::env::current_exe()
            .and_then(|exe| Pty::spawn(&exe, &shell(), &cwd, self.wake.clone()));
        let pty = match spawned {
            Ok(pty) => pty,
            Err(error) => {
                self.note(&format!("terminal: {error}"));
                return None;
            }
        };
        self.focus_pane(from);
        let home = self.state.focused_pane_id();
        self.split_share(Direction::Horizontal, 0.5);
        let opened = self.state.focused_pane_id().filter(|id| Some(*id) != home);
        if let Some(pane) = opened.and_then(|id| self.state.panes.get_mut(&id)) {
            pane.content = PaneContent::Terminal(Box::new(pty));
            self.state.focus_panes();
        }
        opened
    }

    /// Incident: the tab's first live pane took the block, so a chip typed into an ssh session;
    /// now the terminal the person looks at, else the one a chip of this tab fed last.
    pub(super) fn run_in_terminal(&mut self, looking: Option<PaneId>, from: PaneId, code: &str) {
        let target = looking
            .filter(|pane| self.live_terminal(*pane))
            .or_else(|| {
                self.chip_terminal()
                    .filter(|pane| self.live_terminal(*pane))
            });
        let fed = match target {
            Some(pane) => {
                self.focus_pane(pane);
                Some(pane)
            }
            None => self.open_terminal_beside(from),
        };
        if let Some(tab) = self.state.tab_mut() {
            tab.chip_terminal = fed;
        }
        self.dirty = true;
        let Some(pty) = self.focused_pty() else {
            return;
        };
        let typed = pty
            .paste(code.trim_end_matches('\n'))
            .and_then(|()| pty.send(b"\r"));
        if let Err(error) = typed {
            self.note(&format!("terminal: {error}"));
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use super::App;
    use crate::layout::PaneId;
    use crate::model::PaneContent;

    type Fixture = (App, PaneId, PaneId, Receiver<Vec<u8>>);

    fn app_with_terminal(root: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new(root.to_owned(), theme);
        let chat = app.state.focused_pane_id().ok_or("a pane")?;
        let shell = app
            .state
            .split_focused(ratatui::layout::Direction::Horizontal)
            .ok_or("no split")?;
        let (pty, written) = crate::pty::tests::queued_terminal()?;
        if let Some(pane) = app.state.panes.get_mut(&shell) {
            pane.content = PaneContent::Terminal(Box::new(pty));
        }
        Ok((app, chat, shell, written))
    }

    fn typed(written: &Receiver<Vec<u8>>) -> String {
        String::from_utf8_lossy(&written.try_iter().flatten().collect::<Vec<u8>>()).into_owned()
    }

    /// Dies with a Run that lands in the chat's composer instead of the shell, that types a
    /// command and never presses enter, or that leaves the terminal unfocused so the person
    /// cannot answer a prompt the command raises.
    #[test]
    fn a_shell_block_is_typed_into_the_live_terminal_and_entered()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, chat, shell, written) = app_with_terminal("/r")?;
        let code = "cd ~/Development/yi && git pull --ff-only origin main && \\\n  scripts/build_dist.sh\n";
        app.run_in_terminal(Some(shell), chat, code);
        assert_eq!(
            typed(&written),
            "cd ~/Development/yi && git pull --ff-only origin main && \\\n  scripts/build_dist.sh\r"
        );
        assert_eq!(app.state.focused_pane_id(), Some(shell));
        Ok(())
    }

    /// Dies with a chip pressed in a chat pane typing the model's command into a live terminal of
    /// another pane, where an ssh session or a root shell takes it for input the person typed.
    #[test]
    fn a_shell_block_is_not_typed_into_a_terminal_the_person_is_not_looking_at()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, chat, _shell, written) = app_with_terminal("/nonexistent-yi-terminal-root")?;
        app.focus_pane(chat);
        let panes = app.state.panes.len();
        app.run_in_terminal(Some(chat), chat, "rm -rf ~");
        assert_eq!(typed(&written), "", "the unfocused terminal took the block");
        assert_eq!(app.state.panes.len(), panes, "a pane was opened anyway");
        Ok(())
    }

    /// Dies with a second Run press splitting another pane and spawning another shell, until the
    /// chat is a sliver and three commands have three terminal panes.
    #[test]
    fn a_second_press_feeds_the_terminal_the_first_one_fed()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, chat, shell, written) = app_with_terminal("/nonexistent-yi-terminal-root")?;
        let panes = app.state.panes.len();
        app.run_in_terminal(Some(shell), chat, "cargo build");
        app.focus_pane(chat);
        app.run_in_terminal(Some(chat), chat, "cargo test");
        assert_eq!(typed(&written), "cargo build\rcargo test\r");
        assert_eq!(
            app.state.panes.len(),
            panes,
            "a second terminal pane was opened"
        );
        assert_eq!(app.state.focused_pane_id(), Some(shell));
        Ok(())
    }

    /// Dies with a chip's terminal opening in the session of whichever pane held focus when the
    /// press landed, instead of the session whose answer carries the block.
    #[test]
    fn a_panes_directory_is_resolved_from_that_pane_not_from_the_focus()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut app, chat, _shell, _written) = app_with_terminal("/r")?;
        let notes = app
            .state
            .split_focused(ratatui::layout::Direction::Horizontal)
            .ok_or("no split")?;
        if let Some(pane) = app.state.panes.get_mut(&notes) {
            pane.content = PaneContent::Markdown {
                path: "/tmp/NOTES.md".to_owned(),
                source: String::new(),
                workdir: "/tmp".to_owned(),
                drawn: None,
            };
        }
        app.focus_pane(notes);
        assert_eq!(app.session_dir(), "/tmp");
        assert_eq!(app.dir_of(chat), "/r");
        Ok(())
    }
}

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::client::Outbound;
use crate::keys::CHORDS;
use crate::model::{Mode, PaneContent, SessionId};

use super::App;

pub enum PaletteEntry {
    Action { index: usize },
    Session(SessionId),
}

impl App {
    pub fn palette_entries(&self, query: &str) -> Vec<PaletteEntry> {
        let needle = query.trim().to_lowercase();
        let mut entries: Vec<PaletteEntry> = CHORDS
            .iter()
            .enumerate()
            .filter(|(_, chord)| chord.action.is_some())
            .filter(|(_, chord)| needle.is_empty() || chord.what.contains(&needle))
            .map(|(index, _)| PaletteEntry::Action { index })
            .collect();
        entries.extend(
            self.navigator_matches(query)
                .into_iter()
                .map(PaletteEntry::Session),
        );
        entries
    }

    /// Sessions matching the navigator query, across every root.
    pub fn navigator_matches(&self, query: &str) -> Vec<SessionId> {
        let needle = query.to_lowercase();
        self.state
            .order
            .iter()
            .filter(|id| {
                if needle.is_empty() {
                    return true;
                }
                let Some(row) = self.state.sessions.get(*id) else {
                    return false;
                };
                id.0.to_lowercase().contains(&needle)
                    || row.label().to_lowercase().contains(&needle)
                    || row.root.to_lowercase().contains(&needle)
            })
            .cloned()
            .collect()
    }

    pub(super) fn handle_navigator_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        let Mode::Navigator { query, selected } = &mut self.state.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.state.mode = Mode::Normal;
            }
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => *selected = selected.saturating_add(1),
            KeyCode::Backspace => {
                query.pop();
                *selected = 0;
            }
            KeyCode::Enter => {
                let query = query.clone();
                let index = *selected;
                self.state.mode = Mode::Normal;
                if self.run_navigator_command(&query) {
                    self.dirty = true;
                    return;
                }
                let entries = self.palette_entries(&query);
                match entries.get(index.min(entries.len().saturating_sub(1))) {
                    Some(PaletteEntry::Action { index }) => {
                        if let Some(action) = CHORDS.get(*index).and_then(|chord| chord.action) {
                            self.apply_action(outbound, action);
                        }
                    }
                    Some(PaletteEntry::Session(session)) => {
                        let session = session.clone();
                        if let Some(pane_id) = self.state.focused_pane_id() {
                            self.resume_into(outbound, pane_id, &session);
                            self.state.focus_panes();
                        }
                    }
                    None => {}
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(c);
                *selected = 0;
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// `md <path>` / `diff <path>` open viewer panes, `nb` swaps the pane
    /// to the notebook view of its session. True when handled.
    pub(super) fn run_navigator_command(&mut self, query: &str) -> bool {
        let (dir, home) = (self.session_dir(), self.state.home.clone());
        let resolve = |path: &str| match (path.strip_prefix("~/"), home.as_deref()) {
            (Some(rest), Some(home)) => format!("{home}/{rest}"),
            _ if path.starts_with('/') => path.to_owned(),
            _ => format!("{dir}/{path}"),
        };
        if query.trim() == "nb" {
            if let Some(pane) = self.state.focused_pane_mut() {
                let session = pane.session().cloned();
                pane.content = PaneContent::Notebook {
                    session,
                    full: false,
                    drawn: None,
                };
                pane.scroll_from_bottom = 0;
            }
            return true;
        }
        if let Some(pattern) = query.strip_prefix('/') {
            self.find_in_editor(pattern.trim());
            return true;
        }
        if let Some(("e", path)) = query.split_once(' ') {
            self.open_editor(&resolve(path.trim()), dir);
            return true;
        }
        let (kind, path) = match query.split_once(' ') {
            Some(("md", path)) => (true, resolve(path.trim())),
            Some(("diff", path)) => (false, resolve(path.trim())),
            _ => return false,
        };
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                self.show_viewer(if kind {
                    PaneContent::Markdown {
                        path,
                        source,
                        workdir: dir,
                        drawn: None,
                    }
                } else {
                    PaneContent::Diff {
                        path,
                        source,
                        workdir: dir,
                        drawn: None,
                    }
                });
            }
            Err(error) => self.note(&format!("{path}: {error}")),
        }
        true
    }

    /// A viewer joins the tab's first panel of its kind, else a new split beside the focused pane,
    /// so `md` and `diff` never take the screen from a lone chat.
    fn show_viewer(&mut self, content: PaneContent) {
        let kind = std::mem::discriminant(&content);
        let found = {
            let state = &self.state;
            state.tabs.get(state.active_tab).and_then(|tab| {
                tab.layout.pane_ids().into_iter().find(|id| {
                    state
                        .panes
                        .get(id)
                        .is_some_and(|pane| std::mem::discriminant(&pane.content) == kind)
                })
            })
        };
        let target = match found {
            Some(id) => {
                if let Some(tab) = self.state.tabs.get_mut(self.state.active_tab) {
                    tab.layout.focus_pane(id);
                    tab.zoomed = false;
                }
                Some(id)
            }
            None => {
                self.state
                    .split_focused(ratatui::layout::Direction::Horizontal);
                self.state.focused_pane_id()
            }
        };
        if let Some(pane) = target.and_then(|id| self.state.panes.get_mut(&id)) {
            pane.content = content;
            pane.scroll_from_bottom = crate::panes::TOP;
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    /// Dies with `e <path>` taking the whole screen: a file opens in a panel beside the chat.
    #[test]
    fn e_opens_a_file_in_a_panel_beside_the_chat() -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = crate::app::App::new("/r".to_owned(), theme);
        let chat = app.state.focused_pane_id().ok_or("a pane")?;
        let path = std::env::temp_dir().join(format!("yi-navigator-{}.txt", std::process::id()));
        std::fs::write(&path, "tide\n")?;
        app.run_navigator_command(&format!("e {}", path.display()));
        let kept =
            app.state.panes.get(&chat).is_some_and(|pane| {
                matches!(pane.content, crate::model::PaneContent::Session { .. })
            });
        assert!(kept, "the chat kept the screen");
        assert_eq!(app.state.panes.len(), 2, "the file opened in its own panel");
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    /// Dies with `md`/`diff` taking the whole screen: a viewer opens in a panel beside the chat.
    #[test]
    fn md_reuses_a_viewer_panel_instead_of_the_chat() -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = crate::app::App::new("/r".to_owned(), theme);
        let chat = app.state.focused_pane_id().ok_or("a pane")?;
        let first = std::env::temp_dir().join(format!("yi-md-a-{}.md", std::process::id()));
        let second = std::env::temp_dir().join(format!("yi-md-b-{}.md", std::process::id()));
        std::fs::write(&first, "# tide\n")?;
        std::fs::write(&second, "# ebb\n")?;
        assert!(app.run_navigator_command(&format!("md {}", first.display())));
        let kept =
            app.state.panes.get(&chat).is_some_and(|pane| {
                matches!(pane.content, crate::model::PaneContent::Session { .. })
            });
        assert!(kept, "the chat kept the screen");
        assert_eq!(
            app.state.panes.len(),
            2,
            "the viewer opened in its own panel"
        );
        assert!(app.run_navigator_command(&format!("md {}", second.display())));
        assert_eq!(
            app.state.panes.len(),
            2,
            "a second markdown joins the viewer panel"
        );
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
        Ok(())
    }
}

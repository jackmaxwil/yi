use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::client::Outbound;
use crate::keys::CHORDS;
use crate::model::{Mode, PaneContent, SessionId, Zone};

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
                            self.state.zone = Zone::Panes;
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
        let root = self.state.root.clone();
        let resolve = |path: &str| {
            if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("{root}/{path}")
            }
        };
        if query.trim() == "nb" {
            if let Some(pane) = self.state.focused_pane_mut() {
                let session = pane.session().cloned();
                pane.content = PaneContent::Notebook {
                    session,
                    cells: Vec::new(),
                    input: crate::model::notebook_input(),
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
            self.open_editor(&resolve(path.trim()));
            return true;
        }
        let (kind, path) = match query.split_once(' ') {
            Some(("md", path)) => (true, resolve(path.trim())),
            Some(("diff", path)) => (false, resolve(path.trim())),
            _ => return false,
        };
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                if let Some(pane) = self.state.focused_pane_mut() {
                    pane.content = if kind {
                        PaneContent::Markdown { path, source }
                    } else {
                        PaneContent::Diff { path, source }
                    };
                    pane.scroll_from_bottom = 0;
                }
            }
            Err(error) => self.note(&format!("{path}: {error}")),
        }
        true
    }
}

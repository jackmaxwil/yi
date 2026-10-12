use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use serde_json::{Value, json};
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

use crate::client::Outbound;
use crate::keys::Action;
use crate::layout::PaneId;
use crate::menu::{Facts, Menu, Target, Verb};
use crate::model::{Mode, PaneContent, SessionId, SessionRow, SessionStatus, Zone, now_ms};

use super::port::PortRequest;
use super::{App, MouseKind, RequestKind};

const UNWRAPPED: usize = 10_000;

impl App {
    fn target_at(&self, x: u16, y: u16) -> Option<Target> {
        let hits = self.hits.as_ref()?;
        if x < hits.sidebar_width {
            let (_, index) = hits.sidebar_rows.iter().find(|(row, _)| *row == y)?;
            return Some(Target::Session(*index));
        }
        let (pane, _) = hits
            .panes
            .iter()
            .find(|(_, rect)| rect.contains(Position::new(x, y)))?;
        let message = match &self.state.panes.get(pane)?.content {
            PaneContent::Session {
                chat: Some(chat), ..
            } => chat.app.pane_row(y).flatten().and_then(|(cell, _)| {
                let (said, source) = chat.app.said(cell)?;
                let entry = chat.app.locate(cell, &chat.port.branch()).cloned();
                Some((said, source.to_owned(), entry.map(Box::new)))
            }),
            _ => None,
        };
        Some(match message {
            Some((said, source, entry)) => Target::Message {
                pane: *pane,
                source,
                said,
                entry,
            },
            None => Target::Pane(*pane),
        })
    }

    fn open_menu(&mut self, target: Target, at: (u16, u16)) {
        let running = match &target {
            Target::Message { pane, .. } => self.state.panes.get(pane).is_some_and(|pane| {
                matches!(&pane.content, PaneContent::Session { chat: Some(chat), .. } if chat.app.is_running())
            }),
            Target::Pane(_) | Target::Session(_) => false,
        };
        let tab = self.state.tab();
        let facts = Facts {
            cmd: self.cmd_hints,
            running,
            last_pane: tab.is_none_or(|tab| tab.layout.pane_ids().len() < 2),
            zoomed: tab.is_some_and(|tab| tab.zoomed),
        };
        self.state.mode = Mode::Menu(Menu::new(target, at, &facts));
        self.dirty = true;
    }

    pub(super) fn right_click(&mut self, x: u16, y: u16) {
        if let Some(target) = self.target_at(x, y) {
            self.open_menu(target, (x, y));
        }
    }

    pub(super) fn open_menu_for_focus(&mut self) {
        let Some(hits) = &self.hits else { return };
        let opened = match self.state.zone {
            Zone::Sidebar => hits
                .sidebar_rows
                .iter()
                .find(|(_, index)| *index == self.state.selected)
                .map(|(row, index)| (Target::Session(*index), (1, *row))),
            Zone::Panes => self.state.focused_pane_id().and_then(|id| {
                let (_, rect) = hits.panes.iter().find(|(pane, _)| *pane == id)?;
                let at = (rect.x.saturating_add(2), rect.y.saturating_add(1));
                Some((Target::Pane(id), at))
            }),
        };
        if let Some((target, at)) = opened {
            self.open_menu(target, at);
        }
    }

    fn close_menu(&mut self) {
        self.state.mode = Mode::Normal;
        self.dirty = true;
    }

    pub(super) fn menu_key(&mut self, outbound: &Outbound, key: KeyEvent) {
        let Mode::Menu(menu) = &mut self.state.mode else {
            return;
        };
        self.dirty = true;
        match key.code {
            KeyCode::Up => menu.step(false),
            KeyCode::Down => menu.step(true),
            KeyCode::Right => {
                menu.descend();
            }
            KeyCode::Left | KeyCode::Esc if menu.levels.len() > 1 => {
                menu.levels.pop();
            }
            KeyCode::Left | KeyCode::Esc => self.close_menu(),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(outbound),
            _ => {}
        }
    }

    /// A press inside the menu picks the row under it; outside, it closes the menu, and a
    /// right press there opens the next one where it landed.
    pub(super) fn menu_mouse(&mut self, outbound: &Outbound, kind: MouseKind, x: u16, y: u16) {
        let Mode::Menu(menu) = &mut self.state.mode else {
            return;
        };
        let rects = self
            .hits
            .as_ref()
            .map(|hits| hits.overlays.clone())
            .unwrap_or_default();
        let hit = crate::menu::hit(menu, &rects, x, y);
        match (kind, hit) {
            (MouseKind::Moved, Some((depth, Some(row)))) => {
                if menu.point(depth, row) {
                    menu.descend();
                    self.dirty = true;
                }
            }
            (MouseKind::Down | MouseKind::RightDown, Some((depth, Some(row)))) => {
                menu.point(depth, row);
                self.activate(outbound);
            }
            (MouseKind::Down, None) => self.close_menu(),
            (MouseKind::RightDown, None) => {
                self.close_menu();
                self.right_click(x, y);
            }
            _ => {}
        }
    }

    fn activate(&mut self, outbound: &Outbound) {
        let Mode::Menu(menu) = &mut self.state.mode else {
            return;
        };
        if menu.descend() {
            self.dirty = true;
            return;
        }
        let Some(item) = menu.current() else { return };
        if let Some(reason) = item.blocked {
            return self.note(reason);
        }
        let Some(verb) = item.verb else { return };
        let Mode::Menu(menu) = std::mem::replace(&mut self.state.mode, Mode::Normal) else {
            return;
        };
        self.dirty = true;
        self.run_verb(outbound, menu.target, verb);
    }

    fn focus_pane(&mut self, pane: PaneId) {
        if let Some(tab) = self.state.tab_mut() {
            tab.layout.focus_pane(pane);
        }
        self.state.zone = Zone::Panes;
    }

    fn run_verb(&mut self, outbound: &Outbound, target: Target, verb: Verb) {
        let pane = match &target {
            Target::Message { pane, .. } | Target::Pane(pane) => Some(*pane),
            Target::Session(_) => None,
        };
        let here = |pane| {
            self.state
                .tab()
                .is_some_and(|tab| tab.layout.pane_ids().contains(&pane))
        };
        // Incident: an idle notebook closed itself under an open menu, and Close then closed the
        // chat that had focus; a verb whose pane is gone does nothing.
        if pane.is_some_and(|pane| !here(pane)) {
            return self.note("that pane has closed");
        }
        match (target, verb) {
            (Target::Message { source, .. }, Verb::Copy) => self.copy_out(&source),
            (Target::Message { source, .. }, Verb::CopyPlain) => {
                let plain: Vec<String> = yi_tui::markdown::render(&source, UNWRAPPED, &self.theme)
                    .iter()
                    .map(|line| {
                        let row: String = line
                            .spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect();
                        crate::select::strip_chrome(row.trim_end())
                    })
                    .collect();
                self.copy_out(plain.join("\n").trim_end());
            }
            (
                Target::Message {
                    pane,
                    entry: Some(entry),
                    ..
                },
                verb @ (Verb::CopyJson | Verb::Inspect | Verb::Rewind { .. } | Verb::Fork { .. }),
            ) => {
                self.ledger_verb(outbound, pane, &entry, verb);
            }
            (Target::Message { pane, source, .. }, Verb::Quote) => {
                self.focus_pane(pane);
                if let Some(chat) = self.focused_chat() {
                    chat.app.quote(&source);
                }
            }
            (Target::Pane(pane), Verb::Act(action)) => {
                self.focus_pane(pane);
                self.apply_action(outbound, action);
            }
            (Target::Session(index), Verb::Open) => {
                self.state.selected = index;
                self.state.cursor_moved = true;
                self.open_selected(outbound);
            }
            (Target::Session(_), Verb::OpenBeside) if !self.connected() => {
                self.note("not connected")
            }
            (Target::Session(index), Verb::OpenBeside) => {
                self.state.selected = index;
                self.state.cursor_moved = true;
                self.apply_action(outbound, Action::SplitRight);
                self.open_selected(outbound);
            }
            _ => {}
        }
    }
}

impl App {
    fn ledger_verb(&mut self, outbound: &Outbound, pane: PaneId, entry: &Entry, verb: Verb) {
        let session = self
            .state
            .panes
            .get(&pane)
            .and_then(|pane| pane.session().cloned());
        let id = entry.id().to_owned();
        match (verb, session) {
            (Verb::CopyJson, _) => {
                let json = serde_json::to_string_pretty(entry).unwrap_or_default();
                self.copy_out(&json);
            }
            (Verb::Inspect, _) => {
                let source = inspect(entry);
                let opened = self.beside(
                    pane,
                    PaneContent::Markdown {
                        path: format!("entry {id}"),
                        source,
                    },
                );
                // The pane clamps the offset, so the largest one opens it at its top.
                if let Some(opened) = opened.and_then(|id| self.state.panes.get_mut(&id)) {
                    opened.scroll_from_bottom = usize::MAX;
                }
            }
            (Verb::Rewind { files }, Some(session)) => {
                let request = if files {
                    PortRequest::RewindFiles(id)
                } else {
                    PortRequest::Rewind(id)
                };
                self.send_port_request(outbound, pane, &session, request);
            }
            (Verb::Fork { at }, Some(session)) => {
                let Some(new) = self.beside(
                    pane,
                    PaneContent::Session {
                        session: None,
                        chat: None,
                    },
                ) else {
                    return;
                };
                let position = if at { "at" } else { "before" };
                let params = json!({"sessionId": session.0, "entryId": id, "position": position});
                self.send_request(outbound, RequestKind::Fork(new), "_yi/fork", params);
            }
            _ => self.note("this pane has no session"),
        }
    }

    /// A pane opened to the right of `pane` and focused; `None` when the split made none.
    fn beside(&mut self, pane: PaneId, content: PaneContent) -> Option<PaneId> {
        self.focus_pane(pane);
        self.split_with_anim(ratatui::layout::Direction::Horizontal);
        let new = self.state.focused_pane_id().filter(|id| *id != pane)?;
        if let Some(slot) = self.state.panes.get_mut(&new) {
            slot.content = content;
        }
        Some(new)
    }

    /// The fork's session opens in the pane made for it; a re-ask puts the prompt back to edit.
    pub(super) fn absorb_fork(&mut self, outbound: &Outbound, pane: PaneId, reply: &Value) {
        let Some(id) = reply.pointer("/result/sessionId").and_then(Value::as_str) else {
            let why = reply.pointer("/error/message").and_then(Value::as_str);
            return self.note(&format!("fork: {}", why.unwrap_or("no session came back")));
        };
        let session = SessionId(id.to_owned());
        self.state.upsert_row(SessionRow {
            id: session.clone(),
            root: self.state.root.clone(),
            status: SessionStatus::Idle,
            attached: true,
            name: None,
            created_ms: now_ms(),
            last_ms: 0,
        });
        self.resume_into(outbound, pane, &session);
        let prefill = reply
            .pointer("/result/prefill")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(PaneContent::Session {
            chat: Some(chat), ..
        }) = self
            .state
            .panes
            .get_mut(&pane)
            .map(|pane| &mut pane.content)
            && !prefill.is_empty()
        {
            chat.app.set_draft_if_empty(prefill);
        }
    }
}

/// The entry with what its message cost on top; a duration is the gap to the entry before it,
/// which the ledger does not store, so the pane says it is derived.
fn inspect(entry: &Entry) -> String {
    let json = serde_json::to_string_pretty(entry).unwrap_or_default();
    let mut head = format!("**entry** `{}`", entry.id());
    if let Entry::Message {
        message:
            AgentMessage::Assistant {
                usage,
                provider,
                model,
                stop_reason,
                ..
            },
        ..
    } = entry
    {
        head.push_str(&format!(
            "\n\n**model** {provider}/{model} · **stop** {stop_reason:?}\n\n"
        ));
        if usage.unknown {
            head.push_str("**usage** not reported");
        } else {
            head.push_str(&format!(
                "**tokens** in {} · out {} · cache read {} · cache write {} · **cost** ${}",
                usage.input, usage.output, usage.cache_read, usage.cache_write, usage.cost.total
            ));
        }
    }
    format!("{head}\n\n```json\n{json}\n```\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_tui::colors::{ColorTier, Theme};

    #[test]
    fn a_verb_whose_pane_closed_under_the_menu_does_nothing() -> Result<(), String> {
        let mut app = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
        let (events, _received) = crate::client::events();
        let socket = std::env::temp_dir().join("yi-menu-stale-pane-no-daemon.sock");
        let (outbound, _threads) = crate::client::spawn(socket, events);
        app.apply_action(&outbound, Action::SplitRight);
        let open = |app: &App| app.state.tab().map_or(0, |tab| tab.layout.pane_ids().len());
        let mut ids = crate::layout::PaneIds::default();
        let gone = std::iter::repeat_with(|| ids.allocate())
            .take(64)
            .find(|id| {
                app.state
                    .tab()
                    .is_some_and(|tab| !tab.layout.pane_ids().contains(id))
            })
            .ok_or("no free pane id")?;
        let facts = Facts {
            cmd: false,
            running: false,
            last_pane: false,
            zoomed: false,
        };
        app.state.mode = Mode::Menu(Menu::new(Target::Pane(gone), (0, 0), &facts));
        app.activate(&outbound);
        outbound.shutdown();
        assert_eq!(open(&app), 2, "Close reached the pane that had focus");
        Ok(())
    }
}

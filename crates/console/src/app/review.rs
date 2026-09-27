use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use yi_tui::port::SessionPort;
use yi_types::acp::AcpSessionUpdate;
use yi_types::tape::MarkKind;

use crate::model::{PaneContent, SessionId, SessionStatus};

use super::App;

impl App {
    pub(super) fn on_review(&self) -> Option<SessionId> {
        match &self.state.focused_pane()?.content {
            PaneContent::SessionDiff { session, .. } => Some(session.clone()),
            _ => None,
        }
    }

    pub(super) fn review_key(&mut self, key: KeyEvent) {
        let Some(session) = self.on_review() else {
            return;
        };
        let armed = self
            .state
            .diffs
            .get_mut(&session)
            .and_then(|diff| diff.land_armed.take());
        match key.code {
            KeyCode::Char('s') => {
                if let Some(PaneContent::SessionDiff { scope, .. }) =
                    self.state.focused_pane_mut().map(|pane| &mut pane.content)
                {
                    *scope = scope.next();
                }
                self.refresh_branch(&session);
            }
            KeyCode::Char('l') => match armed {
                Some(title) => {
                    let chat = self.state.chats_mut(&session).into_iter().next();
                    let sent = chat.map(|chat| {
                        chat.port
                            .queue
                            .push(super::port::PortRequest::Slash(format!("land {title}")));
                    });
                    self.note(if sent.is_some() {
                        "landing: the gate's jobs show in the status row as they run"
                    } else {
                        "no chat holds this session, so nothing was sent to land"
                    });
                }
                None => match self
                    .state
                    .sessions
                    .get(&session)
                    .and_then(|row| row.name.clone())
                {
                    Some(title) => {
                        self.state
                            .diffs
                            .entry(session.clone())
                            .or_default()
                            .land_armed = Some(title);
                    }
                    None => self.note("this session has no title yet: /land <title> in its chat"),
                },
            },
            _ => {}
        }
        self.dirty = true;
    }

    pub(super) fn refresh_branch(&mut self, session: &SessionId) {
        self.state
            .diffs
            .entry(session.clone())
            .or_default()
            .branch_due = true;
    }

    pub(super) fn refresh_tape(&mut self, session: &SessionId) {
        self.state
            .diffs
            .entry(session.clone())
            .or_default()
            .tape_due = true;
    }

    pub(super) fn on_tape(&self) -> bool {
        self.state
            .focused_pane()
            .is_some_and(|pane| matches!(pane.content, PaneContent::Tape { .. }))
    }

    pub(super) fn tape_key(&mut self, key: KeyEvent) {
        let Some(PaneContent::Tape {
            session,
            tape,
            cursor,
        }) = self.state.focused_pane_mut().map(|pane| &mut pane.content)
        else {
            return;
        };
        let marks = tape.as_ref().map_or(0, |tape| tape.marks.len());
        match key.code {
            KeyCode::Left => *cursor = cursor.saturating_sub(1),
            KeyCode::Right => *cursor = (*cursor + 1).min(marks.saturating_sub(1)),
            KeyCode::Enter => {
                let chosen = tape
                    .as_ref()
                    .and_then(|tape| tape.marks.get(*cursor))
                    .cloned();
                let session = session.clone();
                let busy = self.state.sessions.get(&session).is_some_and(|row| {
                    matches!(row.status, SessionStatus::Working | SessionStatus::Blocked)
                });
                match chosen {
                    // Invariant: a rewind re-attaches the history a running turn is writing.
                    Some(mark) if mark.kind == MarkKind::User && busy => {
                        self.note("a turn is running: rewind once it ends");
                    }
                    Some(mark) if mark.kind == MarkKind::User => {
                        if let Some(chat) = self.state.chats_mut(&session).into_iter().next() {
                            chat.port
                                .queue
                                .push(super::port::PortRequest::Rewind(mark.entry.clone()));
                        }
                        self.note(
                            "rewound to before that turn: its prompt is back in the composer, the old branch kept",
                        );
                    }
                    Some(_) => self.note("only a turn you typed is a place to rewind to"),
                    None => {}
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub(super) fn absorb_tape(&mut self, session: &SessionId, result: &Value) {
        let fresh: Option<yi_types::tape::Tape> = serde_json::from_value(result.clone()).ok();
        for pane in self.state.panes.values_mut() {
            if let PaneContent::Tape {
                session: bound,
                tape,
                cursor,
            } = &mut pane.content
                && bound == session
            {
                let marks = tape.as_ref().map_or(&[][..], |tape| tape.marks.as_slice());
                let chosen = marks
                    .get(*cursor)
                    .filter(|_| cursor.saturating_add(1) < marks.len());
                let last = fresh
                    .as_ref()
                    .map_or(0, |tape| tape.marks.len().saturating_sub(1));
                *cursor = chosen
                    .and_then(|mark| {
                        fresh
                            .as_ref()?
                            .marks
                            .iter()
                            .position(|kept| kept.entry == mark.entry)
                    })
                    .unwrap_or(last);
                tape.clone_from(&fresh);
            }
        }
        self.dirty = true;
    }

    pub(super) fn ask_branches(&mut self, outbound: &crate::client::Outbound) {
        let tapes: Vec<SessionId> = self
            .state
            .diffs
            .iter_mut()
            .filter_map(|(id, diff)| std::mem::take(&mut diff.tape_due).then(|| id.clone()))
            .collect();
        for session in tapes {
            self.send_request(
                outbound,
                super::RequestKind::Tape(session.clone()),
                "_yi/tape",
                serde_json::json!({"sessionId": session.0}),
            );
        }
        let due: Vec<SessionId> = self
            .state
            .diffs
            .iter_mut()
            .filter_map(|(id, diff)| std::mem::take(&mut diff.branch_due).then(|| id.clone()))
            .collect();
        for session in due {
            self.send_request(
                outbound,
                super::RequestKind::BranchDiff(session.clone()),
                "_yi/branch_diff",
                serde_json::json!({"sessionId": session.0}),
            );
        }
    }

    pub(super) fn absorb_branch(&mut self, session: &SessionId, result: &Value) {
        self.state.diffs.entry(session.clone()).or_default().branch =
            serde_json::from_value(result.clone()).ok();
        self.dirty = true;
    }

    pub(super) fn absorb_review(&mut self, session: &SessionId, update: &AcpSessionUpdate) {
        match update {
            AcpSessionUpdate::StateUpdate(yi_types::acp::AcpState::Running) => {
                let diff = self.state.diffs.entry(session.clone()).or_default();
                diff.turn = diff.turn.saturating_add(1);
            }
            AcpSessionUpdate::StateUpdate(yi_types::acp::AcpState::Idle { .. }) => {
                let reviewing = self.state.panes.values().any(|pane| {
                    matches!(&pane.content, PaneContent::SessionDiff { session: bound, .. } if bound == session)
                });
                if reviewing {
                    self.refresh_branch(session);
                }
                let taping = self.state.panes.values().any(|pane| {
                    matches!(&pane.content, PaneContent::Tape { session: bound, .. } if bound == session)
                });
                if taping {
                    self.refresh_tape(session);
                }
            }
            AcpSessionUpdate::ToolCallUpdate {
                title: Some(title),
                raw_input: Some(input),
                ..
            } if title == "read" => {
                if let Some(path) = input.get("path").and_then(Value::as_str) {
                    let reads = &mut self.state.diffs.entry(session.clone()).or_default().reads;
                    let count = reads.entry(path.to_owned()).or_default();
                    *count = count.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn serving(&self, session: &SessionId) -> Option<String> {
        let list = self.state.chat(session)?.port.todo_list()?;
        list.items()
            .find(|item| item.state == yi_types::plan::doc::TodoStateName::Running)
            .map(|item| item.label.as_str().to_owned())
    }
}

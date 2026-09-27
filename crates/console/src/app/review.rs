use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use yi_tui::port::SessionPort;
use yi_types::acp::AcpSessionUpdate;

use crate::model::{PaneContent, SessionId};

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
        match key.code {
            KeyCode::Char('s') => {
                if let Some(PaneContent::SessionDiff { scope, .. }) =
                    self.state.focused_pane_mut().map(|pane| &mut pane.content)
                {
                    *scope = scope.next();
                }
                self.refresh_branch(&session);
            }
            KeyCode::Up | KeyCode::Down => {
                let diff = self.state.diffs.entry(session.clone()).or_default();
                diff.selected = match key.code {
                    KeyCode::Up => diff.selected.saturating_sub(1),
                    _ => diff.selected.saturating_add(1),
                };
            }
            KeyCode::Char('w') => {
                let scope = match &self.state.focused_pane().map(|pane| &pane.content) {
                    Some(PaneContent::SessionDiff { scope, .. }) => *scope,
                    _ => return,
                };
                let chosen = self.state.diffs.get(&session).and_then(|diff| {
                    crate::render::review_files(diff, scope)
                        .into_iter()
                        .nth(diff.selected)
                });
                if let Some((path, patch)) = chosen {
                    let lines = crate::render::hunk_lines(&patch);
                    self.state.diffs.entry(session.clone()).or_default().why_due =
                        Some((path, lines));
                }
            }
            KeyCode::Char('l') => {
                let title = self
                    .state
                    .sessions
                    .get(&session)
                    .map(crate::model::SessionRow::label)
                    .unwrap_or_default();
                if let Some(chat) = self.state.chats_mut(&session).into_iter().next() {
                    chat.port
                        .queue
                        .push(super::port::PortRequest::Slash(format!("land {title}")));
                }
                self.note("landing: the gate's jobs show in the status row as they run");
            }
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
            armed,
        }) = self.state.focused_pane_mut().map(|pane| &mut pane.content)
        else {
            return;
        };
        let marks = tape.as_ref().map_or(0, |tape| tape.marks.len());
        let chosen = tape
            .as_ref()
            .and_then(|tape| tape.marks.get(*cursor))
            .filter(|mark| mark.kind == yi_types::tape::MarkKind::User)
            .map(|mark| mark.entry.clone());
        let session = session.clone();
        let confirmed = std::mem::take(armed) && key.code == KeyCode::Char('u');
        let (request, note) = match (key.code, chosen) {
            (KeyCode::Left, _) => {
                *cursor = cursor.saturating_sub(1);
                (None, None)
            }
            (KeyCode::Right, _) => {
                *cursor = (*cursor + 1).min(marks.saturating_sub(1));
                (None, None)
            }
            (KeyCode::Enter, Some(entry)) => (
                Some(super::port::PortRequest::Rewind(entry)),
                Some(
                    "rewound to before that turn: its prompt is back in the composer, the old branch kept",
                ),
            ),
            (KeyCode::Char('u'), Some(entry)) if confirmed => (
                Some(super::port::PortRequest::RewindFiles(entry)),
                Some("restoring files to before that turn, then rewinding"),
            ),
            (KeyCode::Char('u'), Some(_)) => {
                *armed = true;
                (
                    None,
                    Some(
                        "u again restores files to before that turn; paths you changed since the last turn are kept",
                    ),
                )
            }
            (KeyCode::Enter | KeyCode::Char('u'), None) => {
                (None, Some("only a turn you typed is a place to rewind to"))
            }
            _ => (None, None),
        };
        if let Some(request) = request
            && let Some(chat) = self.state.chats_mut(&session).into_iter().next()
        {
            chat.port.queue.push(request);
        }
        if let Some(note) = note {
            self.note(note);
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
                ..
            } = &mut pane.content
                && bound == session
            {
                *cursor = fresh
                    .as_ref()
                    .map_or(0, |tape| tape.marks.len().saturating_sub(1));
                tape.clone_from(&fresh);
            }
        }
        self.dirty = true;
    }

    pub(super) fn ask_branches(&mut self, outbound: &crate::client::Outbound) {
        let whys: Vec<(SessionId, (String, Vec<u32>))> = self
            .state
            .diffs
            .iter_mut()
            .filter_map(|(id, diff)| diff.why_due.take().map(|due| (id.clone(), due)))
            .collect();
        for (session, (path, lines)) in whys {
            self.send_request(
                outbound,
                super::RequestKind::Why(session.clone()),
                "_yi/why",
                serde_json::json!({"sessionId": session.0, "path": path, "lines": lines}),
            );
        }
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

    pub(super) fn absorb_rewind(&mut self, session: &SessionId, result: &Value) {
        if let Some(restored) = result.get("restored").and_then(Value::as_str) {
            self.note(&format!("files: {restored}"));
        }
        if let Some(unsent) = result.get("unsent").and_then(Value::as_str) {
            for chat in self.state.chats_mut(session) {
                chat.app.set_draft_if_empty(unsent);
            }
        }
    }

    pub(super) fn absorb_why(&mut self, session: &SessionId, result: &Value) {
        let Some(path) = result.get("path").and_then(Value::as_str) else {
            return;
        };
        let lines = result
            .get("answers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|answer| {
                let line = answer.get("line").and_then(Value::as_u64).unwrap_or(0);
                let text = |key: &str| answer.get(key).and_then(Value::as_str).unwrap_or("");
                if answer.get("error").is_some() {
                    return format!("  ↳ L{line} not committed yet, so no chain");
                }
                let mut row = format!("  ↳ L{line} {} {}", text("commit"), text("subject"));
                for (label, key) in [("todo", "todo"), ("goal", "goal")] {
                    if !text(key).is_empty() {
                        row.push_str(&format!(" · {label} {}", text(key)));
                    }
                }
                row
            })
            .collect();
        self.state
            .diffs
            .entry(session.clone())
            .or_default()
            .why
            .insert(path.to_owned(), lines);
        self.dirty = true;
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

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
        let Some(PaneContent::SessionDiff { scope, .. }) =
            self.state.focused_pane_mut().map(|pane| &mut pane.content)
        else {
            return;
        };
        if key.code == KeyCode::Char('s') {
            *scope = scope.next();
        }
        let scope = *scope;
        let diff = self.state.diffs.entry(session.clone()).or_default();
        let armed = diff.land_armed.take();
        // Invariant: the list the selection indexes can shrink under it (a scope switch, a
        // refresh), so every use clamps, as the view that draws the `▸` does.
        let files = crate::render::review_files(diff, scope);
        diff.selected = diff.selected.min(files.len().saturating_sub(1));
        match key.code {
            KeyCode::Char('s') => {
                diff.selected = 0;
                self.refresh_branch(&session);
            }
            KeyCode::Up => diff.selected = diff.selected.saturating_sub(1),
            KeyCode::Down => {
                diff.selected = diff
                    .selected
                    .saturating_add(1)
                    .min(files.len().saturating_sub(1));
            }
            KeyCode::Char('w') => {
                let chosen = files.into_iter().nth(diff.selected);
                if let Some((path, patch)) = chosen {
                    let lines = crate::render::hunk_lines(&patch);
                    self.state.diffs.entry(session.clone()).or_default().why_due =
                        Some((path, lines));
                }
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
        let answers = result.get("answers").and_then(Value::as_array);
        let mut lines: Vec<String> = answers
            .into_iter()
            .flatten()
            .map(|answer| {
                let line = answer.get("line").and_then(Value::as_u64).unwrap_or(0);
                let text = |key: &str| answer.get(key).and_then(Value::as_str).unwrap_or("");
                if answer.get("uncommitted").is_some() {
                    return format!("  ↳ L{line} not committed yet, so no chain");
                }
                if let Some(error) = answer.get("error").and_then(Value::as_str) {
                    let first = error.lines().next().unwrap_or(error);
                    return format!("  ↳ L{line} no chain: {first}");
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
        let unasked = result.get("unasked").and_then(Value::as_array);
        if let Some(next) = unasked.and_then(|rest| rest.first()) {
            let kept = answers.map_or(0, Vec::len);
            let total = kept.saturating_add(unasked.map_or(0, Vec::len));
            let cap = result.get("cap").and_then(Value::as_u64).unwrap_or(0);
            lines.push(format!(
                "  […] {kept} of {total} hunks asked (_yi/why answers {cap} a call); the next: yi why {path}:{next}"
            ));
        }
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
            }
            AcpSessionUpdate::ToolCallUpdate(yi_types::acp::AcpToolCallUpdate {
                title: Some(title),
                raw_input: Some(input),
                ..
            }) if title == "read" => {
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
        list.running().map(|item| item.label.as_str().to_owned())
    }
}

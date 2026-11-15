use serde_json::Value;

use super::App;
use crate::client::Outbound;
use crate::model::{SessionId, SessionRow, SessionStatus};

impl App {
    pub(super) fn finish_resume(
        &mut self,
        outbound: &Outbound,
        session: SessionId,
        result: &Value,
    ) {
        let name = result
            .pointer("/_meta/yi/name")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match self.state.sessions.get_mut(&session) {
            Some(row) => row.attached = true,
            None => self.state.upsert_row(SessionRow {
                id: session.clone(),
                root: self.state.root.clone(),
                status: SessionStatus::Idle,
                attached: true,
                name,
                created_ms: 0,
                last_ms: 0,
                pinned: None,
            }),
        }
        if let Some(replayed_to) = result
            .pointer("/_meta/yi/replayedTo")
            .and_then(Value::as_u64)
        {
            self.resume_offsets.insert(session.clone(), replayed_to);
        }
        self.absorb_result(&session, result);
        if !self.replayed.contains(&session) {
            self.note(
                "the daemon is an older yi and streams nothing this pane can draw — \
                 restart it: pkill -f 'yi serve', then run yi again",
            );
        }
        self.mark_seen(outbound, &session);
    }

    pub(super) fn merge_worker_list(&mut self, result: &Value) {
        let Some(sessions) = result.get("sessions").and_then(Value::as_array) else {
            return;
        };
        let root = self.state.root.clone();
        for entry in sessions {
            let Some(id) = entry.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let id = SessionId(id.to_owned());
            let attached = entry
                .pointer("/_meta/yi/attached")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || self.state.sessions.get(&id).is_some_and(|row| row.attached);
            let status = self
                .state
                .sessions
                .get(&id)
                .map_or(SessionStatus::Unknown, |row| row.status);
            self.state.upsert_row(SessionRow {
                id,
                root: root.clone(),
                status,
                attached,
                name: entry
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                created_ms: entry
                    .pointer("/_meta/yi/createdAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                last_ms: 0,
                pinned: None,
            });
        }
    }

    pub(super) fn merge_daemon_list(&mut self, result: &Value) {
        let Some(sessions) = result.get("sessions").and_then(Value::as_array) else {
            return;
        };
        let listed: std::collections::HashSet<&str> = sessions
            .iter()
            .filter_map(|entry| entry.get("sessionId").and_then(Value::as_str))
            .collect();
        for (id, row) in &mut self.state.sessions {
            if !listed.contains(id.0.as_str()) {
                row.pinned = None;
            }
        }
        for entry in sessions {
            let Some(id) = entry.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let id = SessionId(id.to_owned());
            let pinned = entry.pointer("/_meta/yi/pinnedMs").and_then(Value::as_u64);
            if let Some(row) = self.state.sessions.get_mut(&id) {
                row.pinned = pinned;
            }
            let name = entry
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let last_ms = entry
                .pointer("/_meta/yi/lastEventMs")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // A session on screen streams its own state; the ledger covers
            // the ones nobody is attached to, and names whichever it can.
            if self.state.session_visible(&id) {
                if let Some(row) = self.state.sessions.get_mut(&id) {
                    row.name = name.or(row.name.take());
                    row.last_ms = row.last_ms.max(last_ms);
                }
                continue;
            }
            let unseen = entry
                .pointer("/_meta/yi/unseen")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let status = entry
                .pointer("/_meta/yi/lastState")
                .and_then(Value::as_str)
                .map_or(SessionStatus::Unknown, |state| {
                    SessionStatus::from_ledger(state, unseen)
                });
            let attached = entry
                .pointer("/_meta/yi/attached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let root = entry
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or(&self.state.root)
                .to_owned();
            self.state.upsert_row(SessionRow {
                id,
                root,
                status,
                attached,
                name,
                created_ms: 0,
                last_ms,
                pinned,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red_gate(app: &mut App, id: &str) {
        use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
        let id = SessionId(id.to_owned());
        let mut chat = app.make_chat(&id);
        chat.app
            .reduce_agent(yi_types::event::AgentEvent::LandingState {
                landing: Landing::Open {
                    pr: PrNumber(612),
                    jobs: vec![LandingJob {
                        name: "gate (test)".to_owned(),
                        state: JobState::Red,
                    }],
                    behind: 0,
                },
            });
        app.state.parked.insert(id, chat);
    }

    /// A fold must not hide a session waiting on a red gate: the Working row ranks need 2 only
    /// while its gate is red, so the group's header marks it and a green neighbour's does not.
    #[test]
    fn a_folded_group_marks_a_red_gate_session() {
        use crate::model::{SessionRow, SessionStatus, SidebarMode};
        use crate::render::Hit;
        use crate::sidebar::sidebar_lines;
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        app.state.sidebar = SidebarMode::Full;
        for (id, root) in [("s-red", "/red"), ("s-green", "/green")] {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: root.to_owned(),
                status: SessionStatus::Working,
                attached: true,
                name: Some(id.to_owned()),
                created_ms: 0,
                last_ms: 1,
                pinned: None,
            });
            app.state.toggle_fold(root);
        }
        red_gate(&mut app, "s-red");
        let header = |root: &str| {
            sidebar_lines(&app, &theme, 60)
                .into_iter()
                .find(|row| matches!(&row.control, Some(Hit::Fold(r)) if r == root))
                .map(|row| row.line.to_string())
                .unwrap_or_default()
        };
        let mark = SessionStatus::Working.glyph();
        assert!(header("/red").contains(mark), "{:?}", header("/red"));
        assert!(!header("/green").contains(mark), "{:?}", header("/green"));
    }

    #[test]
    fn a_red_gate_ranks_between_done_and_working() {
        use crate::model::{SessionRow, SessionStatus};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        for (id, status, last_ms) in [
            ("s-work", SessionStatus::Working, 30),
            ("s-red", SessionStatus::Working, 10),
            ("s-done", SessionStatus::DoneUnseen, 5),
        ] {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: "/r".to_owned(),
                status,
                attached: true,
                name: Some(id.to_owned()),
                created_ms: 0,
                last_ms,
                pinned: None,
            });
        }
        red_gate(&mut app, "s-red");
        let order: Vec<String> = app
            .state
            .visible_rows()
            .into_iter()
            .filter_map(|index| app.state.order.get(index).map(|id| id.0.clone()))
            .collect();
        assert_eq!(order, ["s-done", "s-red", "s-work"]);
    }

    #[test]
    fn a_row_the_daemon_no_longer_lists_loses_its_pin() {
        use crate::model::{SessionRow, SessionStatus};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        for id in ["s-listed", "s-gone"] {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: "/r".to_owned(),
                status: SessionStatus::Idle,
                attached: false,
                name: Some(id.to_owned()),
                created_ms: 0,
                last_ms: 1,
                pinned: Some(7),
            });
        }
        let listed = serde_json::json!({"sessions": [{"sessionId": "s-listed", "cwd": "/r",
        "_meta": {"yi": {"pinnedMs": 7, "lastState": "idle"}}}]});
        app.merge_daemon_list(&listed);
        let pins: Vec<_> = ["s-listed", "s-gone"]
            .map(|id| {
                app.state
                    .sessions
                    .get(&SessionId(id.to_owned()))
                    .map(|row| row.pinned)
            })
            .into();
        assert_eq!(pins, [Some(Some(7)), Some(None)]);
    }

    /// Dies with an idle session holding a red gate, a todo list or a claimed question collapsing
    /// to one row, and with its identicon (a kitty image over the rows below) outgrowing its rows.
    #[test]
    fn an_idle_session_with_news_keeps_its_state_line_and_its_avatar_height() {
        use crate::model::{SessionRow, SessionStatus, SidebarMode};
        use crate::sidebar::sidebar_lines;
        use yi_types::plan::doc::{Todo, TodoLabel};
        use yi_types::todo::{Claim, PhaseName, TodoList, TodoPhase};
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        app.state.sidebar = SidebarMode::Full;
        let ids = ["quiet", "red", "todo", "claim", "done-claim"];
        for id in ids {
            app.state.upsert_row(SessionRow {
                id: SessionId(id.to_owned()),
                root: "/r".to_owned(),
                status: SessionStatus::Idle,
                attached: false,
                name: Some(id.to_owned()),
                created_ms: 0,
                last_ms: 1,
                pinned: None,
            });
        }
        red_gate(&mut app, "red");
        let claim = |observed: Option<&str>| Claim {
            label: "is it so".to_owned(),
            observed: observed.map(str::to_owned),
        };
        let mut todo = app.make_chat(&SessionId("todo".to_owned()));
        let items = TodoLabel::new("write the plan")
            .map(|label| vec![Todo::pending(label)])
            .unwrap_or_default();
        let phases = PhaseName::new("Tasks")
            .map(|name| {
                vec![TodoPhase {
                    name,
                    items,
                    extra: serde_json::Map::new(),
                }]
            })
            .unwrap_or_default();
        todo.port.set_todos(Some(TodoList {
            phases,
            ..TodoList::default()
        }));
        app.state.parked.insert(SessionId("todo".to_owned()), todo);
        for (id, observed) in [("claim", None), ("done-claim", Some("yes"))] {
            let mut chat = app.make_chat(&SessionId(id.to_owned()));
            chat.port.set_claims(vec![claim(observed)]);
            app.state.parked.insert(SessionId(id.to_owned()), chat);
        }
        let lines = sidebar_lines(&app, &theme, 60);
        let drawn = |id: &str| -> Option<(usize, (u16, u16))> {
            let index = app.state.order.iter().position(|row| row.0 == id)?;
            let mine = lines.iter().filter(|row| row.index == Some(index));
            let avatar = mine.clone().find_map(|row| row.avatar.as_ref())?;
            Some((mine.count(), (avatar.cols, avatar.rows)))
        };
        assert_eq!(drawn("quiet"), Some((1, (2, 1))));
        assert_eq!(drawn("done-claim"), Some((1, (2, 1))));
        for id in ["red", "todo", "claim"] {
            assert_eq!(drawn(id), Some((2, (4, 2))), "{id} keeps its state line");
        }
    }
}

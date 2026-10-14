use super::App;
use crate::model::SessionId;

fn red_gate(app: &mut App, id: &str) {
    use yi_types::lane::{JobState, Landing, LandingJob, PrNumber};
    let id = SessionId(id.to_owned());
    let Some(pane) = app.state.focused_pane_id() else {
        return;
    };
    let mut chat = app.make_chat(pane, &id);
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
    use crate::sidebar::{Control, sidebar_lines};
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
            .find(|row| matches!(&row.control, Some(Control::Fold(r)) if r == root))
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

//! The sidebar's session rows as polls and focus change them.

use yi_console::model::{ConsoleState, SessionId, SessionRow, SessionStatus};

fn row(status: SessionStatus) -> SessionRow {
    SessionRow {
        id: SessionId("s1".to_owned()),
        root: "/r".to_owned(),
        status,
        attached: true,
        name: None,
        created_ms: 0,
        last_ms: 0,
        pinned: None,
    }
}

/// Dies with the poll's word taken over the console's: an attached session's unseen count
/// stays 0 on the daemon, so the next poll turned a finished session's ● back into ○.
#[test]
fn a_poll_never_clears_a_done_session_only_a_focus_does() {
    let mut state = ConsoleState::new("/r".to_owned());
    let status = |state: &ConsoleState| {
        state
            .sessions
            .get(&SessionId("s1".to_owned()))
            .map(|row| row.status)
    };
    state.upsert_row(row(SessionStatus::DoneUnseen));
    state.upsert_row(row(SessionStatus::Idle));
    assert_eq!(status(&state), Some(SessionStatus::DoneUnseen));
    state.upsert_row(row(SessionStatus::Working));
    assert_eq!(status(&state), Some(SessionStatus::Working));
}

/// Dies with both frames dropped as `Other`: a console pane never showed the plan's progress
/// or which done todos the ledger backs, where the solo TUI showed the plan line.
#[test]
fn claims_and_plan_progress_reach_a_pane() -> Result<(), String> {
    use yi_console::app::port::{Decoded, decode};
    use yi_types::acp::AcpExtensionUpdate;
    let frame = |kind: &str, key: &str, value: serde_json::Value| AcpExtensionUpdate {
        session_update: kind.to_owned(),
        fields: std::iter::once((key.to_owned(), value)).collect(),
    };
    let claims = frame(
        "_yi/claims",
        "claims",
        serde_json::json!([{"label": "run the suite", "observed": "c1"}, {"label": "write"}]),
    );
    match decode(claims).map_err(|_| "malformed claims")? {
        Decoded::Claims(claims) => assert_eq!(
            claims
                .iter()
                .map(|c| c.observed.is_some())
                .collect::<Vec<_>>(),
            vec![true, false]
        ),
        _ => return Err("claims decoded as something else".to_owned()),
    }
    let plan = frame(
        "_yi/plan_progress",
        "plan",
        serde_json::json!({"done": 19, "total": 19, "running": null}),
    );
    match decode(plan).map_err(|_| "malformed plan")? {
        Decoded::Plan(Some(plan)) => assert_eq!(plan.line(true), "Plan 19/19"),
        _ => return Err("plan decoded as something else".to_owned()),
    }
    Ok(())
}

fn transmits(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).matches("a=t").count()
}

/// Dies with identity only: a working session's avatar stayed its identicon, so the inbox
/// could not show which sessions were doing what; one back at rest shows its identicon again.
#[test]
fn a_working_avatar_plays_its_loop_and_rests_as_its_identicon() {
    use yi_console::avatar::{Art, Avatars, Placement};
    use yi_tui::orb::OrbState;
    let place = |state: Option<OrbState>| Placement {
        col: 2,
        row: 0,
        cols: 4,
        rows: 2,
        key: "s1".to_owned(),
        seed: "s1".to_owned(),
        accent: (200, 211, 245),
        state,
        art: Art::Identicon,
    };
    let mut avatars = Avatars::default();
    let mut out = Vec::new();
    avatars.sync(&mut out, &[place(Some(OrbState::Reading))]);
    assert_eq!(transmits(&out), 1, "the identicon goes out first");
    std::thread::sleep(std::time::Duration::from_millis(80));
    out.clear();
    avatars.animate(&mut out);
    assert_eq!(
        transmits(&out),
        1,
        "a frame of the loop replaces it in place"
    );
    assert!(
        avatars.wake().is_some(),
        "a playing avatar asks for the next frame"
    );
    out.clear();
    avatars.sync(&mut out, &[place(None)]);
    assert_eq!(transmits(&out), 1, "back at rest, the identicon returns");
    assert!(avatars.wake().is_none());
}

#[test]
fn a_titled_session_name_reaches_a_pane() -> Result<(), String> {
    use yi_console::app::port::{Decoded, decode};
    let update = yi_types::acp::AcpExtensionUpdate {
        session_update: "_yi/name".to_owned(),
        fields: std::iter::once((
            "name".to_owned(),
            serde_json::json!("Fix the context gauge"),
        ))
        .collect(),
    };
    match decode(update).map_err(|_| "malformed name")? {
        Decoded::Name(name) => assert_eq!(name, "Fix the context gauge"),
        _ => return Err("name decoded as something else".to_owned()),
    }
    Ok(())
}

/// Pins head the sidebar in pin order, out of their groups; a group ranks need, then recency,
/// and folded keeps its header, marked when a session in it needs you.
#[test]
fn the_sidebar_lists_pins_then_each_workspace_need_first() -> Result<(), String> {
    use yi_console::app::App;
    use yi_console::model::SidebarMode;
    use yi_console::sidebar::{Control, sidebar_lines};
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    let mut app = App::new("/w/yi".to_owned(), theme);
    app.state.sidebar = SidebarMode::Full;
    let rows = [
        ("old-idle", "/w/yi", SessionStatus::Idle, 100, None),
        ("new-idle", "/w/yi", SessionStatus::Idle, 900, None),
        ("asks", "/w/yi", SessionStatus::Blocked, 50, None),
        (
            "pin-late",
            "/w/legios",
            SessionStatus::Idle,
            10,
            Some(2_000),
        ),
        (
            "pin-early",
            "/w/buildhost",
            SessionStatus::Working,
            10,
            Some(1_000),
        ),
        (
            "legios-asks",
            "/w/legios",
            SessionStatus::Blocked,
            300,
            None,
        ),
        ("buildhost-idle", "/w/buildhost", SessionStatus::Idle, 400, None),
        ("solo-pin", "/w/solo", SessionStatus::Idle, 20, Some(3_000)),
    ];
    for (id, root, status, last_ms, pinned) in rows {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: root.to_owned(),
            status,
            attached: false,
            name: Some(id.to_owned()),
            created_ms: 0,
            last_ms,
            pinned,
        });
    }
    let drawn = |app: &App| -> Vec<String> {
        let mut last = None;
        sidebar_lines(app, &theme, 60)
            .into_iter()
            .filter_map(|row| match (row.control, row.index) {
                (Some(Control::Fold(_)), _) => {
                    let words: Vec<String> = row
                        .line
                        .to_string()
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    Some(words.join(" "))
                }
                (Some(Control::New(_)), _) => Some("new".to_owned()),
                (Some(Control::Search), _) => Some("search".to_owned()),
                (None, Some(index)) if last != Some(index) => {
                    last = Some(index);
                    let id = app.state.order.get(index)?;
                    app.state.sessions.get(id)?.name.clone()
                }
                (None, None) if row.line.to_string().trim() == "PINNED" => {
                    Some("PINNED".to_owned())
                }
                (None, _) => None,
            })
            .collect()
    };
    let want = [
        "new",
        "search",
        "PINNED",
        "pin-early",
        "pin-late",
        "solo-pin",
        "yi ▾ +",
        "asks",
        "new-idle",
        "old-idle",
        "legios ▾ +",
        "legios-asks",
        "buildhost ▾ +",
        "buildhost-idle",
        "solo ▾ +",
    ];
    assert_eq!(drawn(&app), want);
    let asks = app
        .state
        .order
        .iter()
        .position(|id| id.0 == "legios-asks")
        .ok_or("legios-asks has a row")?;
    app.state.selected = asks;
    app.state.toggle_fold("/w/legios");
    let landed = app.state.selected;
    let name = |index: usize| app.state.order.get(index).map(|id| id.0.as_str());
    assert!(
        app.state.visible_rows().contains(&landed) && name(landed) != Some("legios-asks"),
        "the fold moved the cursor off the hidden row: {:?}",
        name(landed)
    );
    let folded: Vec<String> = want
        .iter()
        .filter(|row| **row != "legios-asks")
        .map(|row| {
            row.replace(
                "legios ▾ +",
                &format!("legios ▸ {} +", SessionStatus::Blocked.glyph()),
            )
        })
        .collect();
    assert_eq!(drawn(&app), folded);
    Ok(())
}

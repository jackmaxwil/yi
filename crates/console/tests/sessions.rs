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
    use yi_console::avatar::{Avatars, Placement};
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

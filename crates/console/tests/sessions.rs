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
    use yi_console::render::Hit;
    use yi_console::sidebar::sidebar_lines;
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
                (Some(Hit::Fold(_)), _) => {
                    let words: Vec<String> = row
                        .line
                        .to_string()
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    Some(words.join(" "))
                }
                (Some(Hit::New(_)), _) => Some("new".to_owned()),
                (Some(Hit::Search), _) => Some("search".to_owned()),
                (None, Some(index)) if last != Some(index) => {
                    last = Some(index);
                    let id = app.state.order.get(index)?;
                    app.state.sessions.get(id)?.name.clone()
                }
                (None, None) if row.line.to_string().trim() == "PINNED" => {
                    Some("PINNED".to_owned())
                }
                (Some(other), _) => Some(format!("{other:?}")),
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

/// Dies with the rail mixing workspaces, dropping the pin to its group, or running two
/// workspaces together with no blank row between them.
#[test]
fn the_rail_lists_the_pin_first_and_parts_workspaces_with_a_blank_row() -> Result<(), String> {
    use yi_console::app::App;
    use yi_console::model::SidebarMode;
    use yi_console::sidebar::sidebar_lines;
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    let mut app = App::new("/w/yi".to_owned(), theme);
    app.state.sidebar = SidebarMode::Rail;
    let rows = [
        ("legios-new", "/w/legios", 900, None),
        ("yi-old", "/w/yi", 100, None),
        ("legios-pin", "/w/legios", 10, Some(1_000)),
        ("yi-new", "/w/yi", 500, None),
    ];
    for (id, root, last_ms, pinned) in rows {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: root.to_owned(),
            status: SessionStatus::Idle,
            attached: false,
            name: Some(id.to_owned()),
            created_ms: 0,
            last_ms,
            pinned,
        });
    }
    let mut last = None;
    let drawn: Vec<String> = sidebar_lines(&app, &theme, 60)
        .into_iter()
        .filter_map(|row| match row.index {
            Some(index) if last != Some(index) => {
                last = Some(index);
                Some(app.state.order.get(index)?.0.clone())
            }
            Some(_) => None,
            None => row.line.to_string().trim().is_empty().then(String::new),
        })
        .collect();
    let want = ["legios-pin", "", "yi-new", "yi-old", "", "legios-new"];
    assert_eq!(drawn, want);
    Ok(())
}

/// Owner, 2026-10-28: "Reduce visual weight of the sidebar". A session with nothing to
/// report is one row; the state line stays for a session that needs you or is working.
#[test]
fn a_quiet_session_draws_one_row() -> Result<(), String> {
    use yi_console::app::App;
    use yi_console::model::SidebarMode;
    use yi_console::sidebar::sidebar_lines;
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    let mut app = App::new("/w/yi".to_owned(), theme);
    app.state.sidebar = SidebarMode::Full;
    let rows = [
        ("quiet", SessionStatus::Idle),
        ("silent", SessionStatus::Unknown),
        ("asks", SessionStatus::Blocked),
        ("works", SessionStatus::Working),
    ];
    for (id, status) in rows {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: "/w/yi".to_owned(),
            status,
            attached: false,
            name: Some(id.to_owned()),
            created_ms: 0,
            last_ms: 1_000,
            pinned: None,
        });
    }
    let lines = sidebar_lines(&app, &theme, 60);
    let drawn = |id: &str| -> Option<usize> {
        let index = app.state.order.iter().position(|row| row.0 == id)?;
        Some(lines.iter().filter(|row| row.index == Some(index)).count())
    };
    for id in ["quiet", "silent"] {
        assert_eq!(drawn(id), Some(1), "{id} takes one row");
    }
    for id in ["asks", "works"] {
        assert_eq!(drawn(id), Some(2), "{id} keeps its state line");
    }
    Ok(())
}

/// The sidebar's chrome reads quieter than its sessions: the New and Search rows and a
/// workspace's header are dimmed, and a session's name is not.
#[test]
fn the_sidebars_chrome_is_dimmer_than_a_session_name() -> Result<(), String> {
    use ratatui::style::Modifier;
    use yi_console::app::App;
    use yi_console::model::{PaneContent, SidebarMode};
    use yi_console::render::Hit;
    use yi_console::sidebar::sidebar_lines;
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    let mut app = App::new("/w/yi".to_owned(), theme);
    app.state.sidebar = SidebarMode::Full;
    app.state.upsert_row(SessionRow {
        id: SessionId("quiet".to_owned()),
        root: "/w/yi".to_owned(),
        status: SessionStatus::Idle,
        attached: false,
        name: Some("quiet".to_owned()),
        created_ms: 0,
        last_ms: 1_000,
        pinned: None,
    });
    let lines = sidebar_lines(&app, &theme, 60);
    let dim = Modifier::DIM;
    let chrome = |kind: &dyn Fn(&Hit) -> bool| -> Result<bool, String> {
        let row = lines
            .iter()
            .find(|row| row.control.as_ref().is_some_and(kind))
            .ok_or("no control row")?;
        Ok(row
            .line
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| row.line.style.patch(span.style).add_modifier.contains(dim)))
    };
    assert!(
        chrome(&|hit| matches!(hit, Hit::New(_)))?,
        "the New row dims"
    );
    assert!(
        chrome(&|hit| matches!(hit, Hit::Search))?,
        "the Search row dims"
    );
    assert!(
        chrome(&|hit| matches!(hit, Hit::Fold(_)))?,
        "the workspace header dims"
    );
    let name = lines
        .iter()
        .flat_map(|row| row.line.spans.iter())
        .find(|span| span.content.contains("quiet"))
        .ok_or("no session name")?;
    assert!(
        !name.style.add_modifier.contains(dim),
        "the session's name reads at full strength: {:?}",
        name.style
    );
    app.state.upsert_row(SessionRow {
        id: SessionId("pinned".to_owned()),
        root: "/w/yi".to_owned(),
        status: SessionStatus::Idle,
        attached: false,
        name: Some("pinned".to_owned()),
        created_ms: 0,
        last_ms: 1_000,
        pinned: Some(1),
    });
    if let Some(pane) = app.state.focused_pane_mut() {
        pane.content = PaneContent::Session {
            session: Some(SessionId("quiet".to_owned())),
            chat: None,
        };
    }
    let lines = sidebar_lines(&app, &theme, 60);
    let weight = |needle: &str| -> Result<Modifier, String> {
        lines
            .iter()
            .find_map(|row| {
                let span = row.line.spans.iter().find(|s| s.content.contains(needle))?;
                Some(row.line.style.patch(span.style).add_modifier)
            })
            .ok_or_else(|| format!("no {needle} span"))
    };
    let focused = weight("quiet")?;
    assert!(
        !focused.intersects(Modifier::BOLD | dim),
        "the focused name is neither bold nor dim: {focused:?}"
    );
    let pinned = weight("PINNED")?;
    assert!(
        pinned.contains(dim) && !pinned.contains(Modifier::BOLD),
        "PINNED is dim, not bold: {pinned:?}"
    );
    Ok(())
}

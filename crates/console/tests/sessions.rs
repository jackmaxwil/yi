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
                (None, None) if row.line.to_string().trim().starts_with("PINNED") => {
                    Some(row.line.to_string().trim().to_owned())
                }
                (Some(other), _) => Some(format!("{other:?}")),
                (None, _) => None,
            })
            .collect()
    };
    let want = [
        "new",
        "search",
        "PINNED 3",
        "pin-early",
        "pin-late",
        "solo-pin",
        "yi 3 ▾ +",
        "asks",
        "new-idle",
        "old-idle",
        "legios 1 ▾ +",
        "legios-asks",
        "buildhost 1 ▾ +",
        "buildhost-idle",
        "solo 0 ▾ +",
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
                "legios 1 ▾ +",
                &format!("legios 1 ▸ {} +", SessionStatus::Blocked.glyph()),
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

fn sidebar_app(sidebar: yi_console::model::SidebarMode) -> yi_console::app::App {
    use yi_console::model::Link;
    use yi_tui::colors::{ColorTier, Theme};
    let mut app =
        yi_console::app::App::new("/w/yi".to_owned(), Theme::new(ColorTier::Ansi16, true));
    app.state.sidebar = sidebar;
    app.state.link = Link::Connected;
    for (id, root, last_ms) in [
        ("yi-new", "/w/yi", 900),
        ("yi-old", "/w/yi", 100),
        ("legios-new", "/w/legios", 800),
        ("legios-old", "/w/legios", 200),
    ] {
        app.state.upsert_row(SessionRow {
            id: SessionId(id.to_owned()),
            root: root.to_owned(),
            status: SessionStatus::Idle,
            attached: false,
            name: Some(id.to_owned()),
            created_ms: 0,
            last_ms,
            pinned: None,
        });
    }
    app
}

fn press(app: &mut yi_console::app::App, code: ratatui::crossterm::event::KeyCode, mods: u8) {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    let (events, _received) = yi_console::client::events();
    let socket = std::env::temp_dir().join("yi-sessions-no-daemon.sock");
    let (outbound, _threads) = yi_console::client::spawn(socket, events);
    let modifiers = KeyModifiers::from_bits_truncate(mods);
    app.handle_event(&outbound, Event::Key(KeyEvent::new(code, modifiers)));
    outbound.shutdown();
}

fn name_at(app: &yi_console::app::App) -> Option<&str> {
    app.state.selected_id().map(|id| id.0.as_str())
}

/// Dies with a header cursor surviving into the rail: `enter` folded a group the rail never
/// paints instead of opening the selected session, and the first `down` skipped a row.
#[test]
fn leaving_the_full_sidebar_drops_a_header_cursor() {
    use ratatui::crossterm::event::KeyCode;
    use yi_console::model::{SidebarMode, Slot};
    let mut app = sidebar_app(SidebarMode::Full);
    let first = app.state.slots().into_iter().find_map(|slot| match slot {
        Slot::Row(index) => Some(index),
        Slot::Group(_) => None,
    });
    app.state.selected = first.unwrap_or(0);
    press(&mut app, KeyCode::Up, 0);
    assert!(matches!(app.state.cursor(), Slot::Group(_)));
    press(&mut app, KeyCode::Char('b'), 8);
    assert_eq!(app.state.sidebar, SidebarMode::Rail);
    assert!(
        matches!(app.state.cursor(), Slot::Row(_)),
        "{:?}",
        app.state.cursor()
    );
    let before = name_at(&app).map(str::to_owned);
    press(&mut app, KeyCode::Down, 0);
    assert_ne!(name_at(&app).map(str::to_owned), before);
    press(&mut app, KeyCode::Enter, 0);
    assert!(app.state.folded.is_empty(), "{:?}", app.state.folded);
}

/// Dies with a fold from a header moving `selected` to another group's session, and with `p`
/// pinning that session while the highlight rests on the header.
#[test]
fn a_fold_from_a_header_keeps_the_session_and_pin_ignores_the_header() {
    use ratatui::crossterm::event::KeyCode;
    use yi_console::model::{SidebarMode, Slot};
    let mut app = sidebar_app(SidebarMode::Full);
    let came_from = app.state.selected;
    press(&mut app, KeyCode::Up, 0);
    assert!(matches!(app.state.cursor(), Slot::Group(_)));
    press(&mut app, KeyCode::Enter, 0);
    assert!(!app.state.folded.is_empty());
    assert_eq!(app.state.selected, came_from);
    press(&mut app, KeyCode::Char('p'), 4);
    assert_eq!(app.state.banner.as_deref(), Some("no session to pin"));
}

/// Dies with the cursor's order drifting from the painted order: the slots are what an
/// arrow steps over, so each must be a header or session the sidebar draws, in its order.
#[test]
fn the_cursor_slots_are_the_painted_rows_in_order() {
    use yi_console::model::{SidebarMode, Slot};
    use yi_console::render::Hit;
    use yi_console::sidebar::sidebar_lines;
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    for mode in [SidebarMode::Full, SidebarMode::Rail] {
        let mut app = sidebar_app(mode);
        app.state.folded.insert("/w/legios".to_owned());
        let mut painted = Vec::new();
        for row in sidebar_lines(&app, &theme, 60) {
            match (row.control, row.index) {
                (Some(Hit::Fold(root)), _) => painted.push(Slot::Group(root)),
                (None, Some(index)) if painted.last() != Some(&Slot::Row(index)) => {
                    painted.push(Slot::Row(index));
                }
                _ => {}
            }
        }
        assert_eq!(app.state.slots(), painted, "{mode:?}");
    }
}

/// Dies with the header's cursor band unpainted: the highlight is the only sign the cursor
/// stands on a header, and it shows only while the sidebar holds focus.
#[test]
fn a_header_under_the_cursor_is_banded_only_while_the_sidebar_has_focus() {
    use yi_console::model::{SidebarMode, Zone};
    use yi_console::render::Hit;
    use yi_console::sidebar::sidebar_lines;
    use yi_tui::colors::{ColorTier, Theme};
    let theme = Theme::new(ColorTier::Ansi16, true);
    let mut app = sidebar_app(SidebarMode::Full);
    app.state.header = Some("/w/legios".to_owned());
    let banded = |app: &yi_console::app::App, root: &str| -> Option<bool> {
        let rows = sidebar_lines(app, &theme, 60);
        let row = rows
            .iter()
            .find(|row| matches!(&row.control, Some(Hit::Fold(drawn)) if drawn == root))?;
        Some(
            row.line
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme.selection_bg())),
        )
    };
    assert_eq!(banded(&app, "/w/legios"), Some(true));
    assert_eq!(banded(&app, "/w/yi"), Some(false));
    app.state.zone = Zone::Panes;
    assert_eq!(banded(&app, "/w/legios"), Some(false));
}

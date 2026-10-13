//! The console's chrome under the pointer: what a click on it does, what a hover on it shows,
//! and how the sidebar and pane titles are laid out.

use std::error::Error;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::{Direction, Rect};
use yi_console::app::{App, MouseKind};
use yi_console::client;
use yi_console::layout::PaneId;
use yi_console::model::{
    Link, PaneContent, SessionId, SessionRow, SessionStatus, SidebarMode, Zone,
};
use yi_console::settings::{Settings, tab_name};
use yi_tui::colors::{ColorTier, Theme};

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

/// One idle session on the sidebar, its cursor away, and a settings pane split beside the home pane.
fn console() -> Result<(App, PaneId), Box<dyn Error>> {
    let mut app = App::new("/repo".to_owned(), theme());
    app.state.link = Link::Connected;
    app.state.upsert_row(SessionRow {
        id: SessionId("s-alpha".to_owned()),
        root: "/repo".to_owned(),
        status: SessionStatus::Idle,
        attached: true,
        name: Some("alpha".to_owned()),
        created_ms: 0,
        last_ms: 0,
    });
    let pane = app
        .state
        .split_focused(Direction::Horizontal)
        .ok_or("no split")?;
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Settings(Box::new(Settings::new(None, 0)));
    app.state.zone = Zone::Panes;
    Ok((app, pane))
}

fn draw(app: &mut App, terminal: &mut Terminal<TestBackend>) -> TestResult {
    let theme = theme();
    terminal.draw(|frame| {
        let mut view = yi_console::render::compute_view(app, frame.area(), &theme);
        yi_console::render::render(app, frame, &mut view, &theme);
    })?;
    app.dirty = false;
    Ok(())
}

fn row_text(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
        .collect()
}

fn mouse(app: &mut App, kind: MouseKind, x: u16, y: u16) {
    let (events, _received) = client::events();
    let socket = std::env::temp_dir().join("yi-chrome-no-daemon.sock");
    let (outbound, _threads) = client::spawn(socket, events);
    app.handle_mouse(&outbound, kind, x, y);
    outbound.shutdown();
}

fn hits(app: &App) -> Result<&yi_console::render::Hits, Box<dyn Error>> {
    Ok(app.hits.as_ref().ok_or("nothing drawn")?)
}

#[test]
fn a_click_on_a_settings_tab_opens_that_tab() -> TestResult {
    let (mut app, pane) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let (keys, _, want) = hits(&app)?
        .settings_tabs
        .iter()
        .copied()
        .rfind(|(_, at, tab)| *at == pane && *tab > 0)
        .ok_or("no other tab has a click target")?;
    let strip = row_text(&terminal, keys.y);
    let label: String = strip
        .chars()
        .skip(usize::from(keys.x))
        .take(usize::from(keys.width))
        .collect();
    assert_eq!(
        label,
        tab_name(want),
        "the target sits on the drawn name: {strip}"
    );
    mouse(&mut app, MouseKind::Down, keys.x.saturating_add(1), keys.y);
    let tab = match app.state.panes.get(&pane).map(|pane| &pane.content) {
        Some(PaneContent::Settings(settings)) => settings.tab,
        _ => return Err("the settings pane went away".into()),
    };
    assert_eq!(tab, want, "the click left the pane on tab {tab}");
    Ok(())
}

#[test]
fn a_pane_with_no_session_titles_itself_without_a_status_dot() -> TestResult {
    let (mut app, _) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let top = row_text(&terminal, 0);
    assert!(top.contains("─ Settings ─"), "{top}");
    assert!(top.contains("─ ❯ no session ─"), "{top}");
    Ok(())
}

#[test]
fn hovering_a_control_lifts_it_and_only_a_new_target_redraws() -> TestResult {
    let (mut app, pane) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let found = hits(&app)?;
    let session = found.sidebar_rows.first().ok_or("no sidebar row")?.0;
    let title = found.titles.iter().find(|(at, _)| *at == pane);
    let tab = found.settings_tabs.iter().find(|(_, _, tab)| *tab > 0);
    let targets: Vec<(&str, Rect)> = vec![
        ("sidebar row", Rect::new(1, session, 1, 1)),
        ("gear", found.gear.ok_or("no gear")?),
        ("pane title", title.ok_or("no title target")?.1),
        ("closer", found.closers.first().ok_or("no closer")?.1),
        ("settings tab", tab.ok_or("no tab target")?.0),
    ];
    let look = |terminal: &Terminal<TestBackend>, x: u16, y: u16| {
        terminal
            .backend()
            .buffer()
            .cell((x, y))
            .map(|cell| cell.style())
    };
    for (name, rect) in targets {
        let (x, y) = (rect.x, rect.y);
        let rest = look(&terminal, x, y);
        mouse(&mut app, MouseKind::Moved, x, y);
        assert!(app.dirty, "entering the {name} asks for no frame");
        draw(&mut app, &mut terminal)?;
        assert_ne!(
            look(&terminal, x, y),
            rest,
            "the hovered {name} looks the same"
        );
        mouse(
            &mut app,
            MouseKind::Moved,
            rect.right().saturating_sub(1),
            y,
        );
        assert!(!app.dirty, "a move inside the {name} redraws");
        mouse(&mut app, MouseKind::Moved, 60, 20);
        assert!(app.dirty, "leaving the {name} asks for no frame");
        draw(&mut app, &mut terminal)?;
        assert_eq!(look(&terminal, x, y), rest, "the {name} stayed lit");
    }
    mouse(&mut app, MouseKind::Moved, 1, session);
    draw(&mut app, &mut terminal)?;
    let detail = terminal
        .backend()
        .buffer()
        .cell((6, session.saturating_add(1)));
    assert_eq!(
        detail.map(|cell| cell.bg),
        Some(theme().selection_bg()),
        "the session's detail row is the same target and lifts with it"
    );
    Ok(())
}

#[test]
fn the_sidebar_keeps_one_column_between_tile_and_text() -> TestResult {
    let (mut app, _) = console()?;
    app.state.sidebar = SidebarMode::Rail;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let y = hits(&app)?.sidebar_rows.first().ok_or("no sidebar row")?.0;
    let rail: Vec<String> = row_text(&terminal, y).chars().map(String::from).collect();
    let glyph = SessionStatus::Idle.glyph();
    assert_eq!(rail.get(6).map(String::as_str), Some(glyph), "{rail:?}");
    assert_eq!(rail.get(4..6), Some(&[" ".to_owned(), " ".to_owned()][..]));
    assert_eq!(hits(&app)?.sidebar_width, 8);
    app.state.sidebar = SidebarMode::Full;
    draw(&mut app, &mut terminal)?;
    let y = hits(&app)?.sidebar_rows.first().ok_or("no sidebar row")?.0;
    let full = row_text(&terminal, y);
    let label = full
        .find("alpha")
        .map(|at| full.get(..at).map_or(0, |head| head.chars().count()));
    assert_eq!(label, Some(6), "{full}");
    Ok(())
}

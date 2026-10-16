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
use yi_console::settings::{Edit, Settings, tab_name};
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

/// Dies with a typed edit left pending across a tab click, so Enter saves it onto row 0 of the new tab.
#[test]
fn a_click_on_a_settings_tab_drops_a_pending_edit() -> TestResult {
    let (mut app, pane) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    if let Some(PaneContent::Settings(settings)) =
        app.state.panes.get_mut(&pane).map(|p| &mut p.content)
    {
        settings.edit = Some(Edit::Type("typed".to_owned()));
        settings.status = Some("saved".to_owned());
    }
    let (keys, ..) = hits(&app)?
        .settings_tabs
        .iter()
        .copied()
        .rfind(|(_, at, tab)| *at == pane && *tab > 0)
        .ok_or("no other tab has a click target")?;
    mouse(&mut app, MouseKind::Down, keys.x.saturating_add(1), keys.y);
    match app.state.panes.get(&pane).map(|pane| &pane.content) {
        Some(PaneContent::Settings(settings)) => {
            assert!(settings.edit.is_none(), "the edit survived the tab click");
            assert!(settings.status.is_none(), "the old tab's status survived");
        }
        _ => return Err("the settings pane went away".into()),
    }
    Ok(())
}

#[test]
fn a_pane_with_no_session_titles_itself_without_a_status_dot() -> TestResult {
    let (mut app, _) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let top = row_text(&terminal, 2);
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

fn row_of(id: &str, name: &str) -> SessionRow {
    SessionRow {
        id: SessionId(id.to_owned()),
        root: "/repo".to_owned(),
        status: SessionStatus::Idle,
        attached: true,
        name: Some(name.to_owned()),
        created_ms: 0,
        last_ms: 0,
    }
}

/// Two sessions side by side, `s-alpha` on the left and `s-beta` on the right, focus on the right.
fn pair() -> Result<(App, PaneId, PaneId), Box<dyn Error>> {
    pair_in(Direction::Horizontal)
}

fn pair_in(direction: Direction) -> Result<(App, PaneId, PaneId), Box<dyn Error>> {
    let mut app = App::new("/repo".to_owned(), theme());
    app.state.link = Link::Connected;
    app.state.upsert_row(row_of("s-alpha", "alpha"));
    app.state.upsert_row(row_of("s-beta", "beta"));
    let left = app.state.focused_pane_id().ok_or("no home pane")?;
    let right = app.state.split_focused(direction).ok_or("no split")?;
    for (pane, id) in [(left, "s-alpha"), (right, "s-beta")] {
        let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
        slot.content = PaneContent::Session {
            session: Some(SessionId(id.to_owned())),
            chat: None,
        };
    }
    app.state.zone = Zone::Panes;
    Ok((app, left, right))
}

fn cell_at(
    terminal: &Terminal<TestBackend>,
    x: u16,
    y: u16,
) -> Result<ratatui::buffer::Cell, Box<dyn Error>> {
    Ok(terminal
        .backend()
        .buffer()
        .cell((x, y))
        .ok_or("off screen")?
        .clone())
}

fn hue_of(app: &App, id: &str, name: &str) -> ratatui::style::Color {
    let row = row_of(id, name);
    yi_tui::colors::accent(app.accent_of(id, row.seed()))
}

#[test]
fn each_pane_wears_its_own_sessions_hue_and_an_unfocused_one_is_dim() -> TestResult {
    use ratatui::style::Modifier;
    let (mut app, left, right) = pair()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let (alpha, beta) = (
        hue_of(&app, "s-alpha", "alpha"),
        hue_of(&app, "s-beta", "beta"),
    );
    assert_ne!(alpha, beta, "two sessions share one hue");
    let found = hits(&app)?;
    let rect_of = |id: PaneId| {
        found
            .panes
            .iter()
            .find(|(at, _)| *at == id)
            .map(|(_, r)| *r)
    };
    let title_of = |id: PaneId| {
        found
            .titles
            .iter()
            .find(|(at, _)| *at == id)
            .map(|(_, r)| *r)
    };
    let (left_rect, right_rect) = (
        rect_of(left).ok_or("no left rect")?,
        rect_of(right).ok_or("no right rect")?,
    );
    let (left_title, right_title) = (
        title_of(left).ok_or("no left title")?,
        title_of(right).ok_or("no right title")?,
    );
    let dim = theme().dim;
    let side = left_rect.y.saturating_add(3);
    let cases = [
        ("left", left_title, left_rect.x, alpha, false),
        (
            "right",
            right_title,
            right_rect.right().saturating_sub(1),
            beta,
            true,
        ),
    ];
    for (name, title, edge, hue, focused) in cases {
        let chip = cell_at(&terminal, title.x, title.y)?;
        assert_eq!(chip.fg, hue, "the {name} title is not its session's hue");
        assert_eq!(
            chip.modifier.contains(Modifier::REVERSED),
            focused,
            "the {name} title is a chip only when focused"
        );
        let border = cell_at(&terminal, edge, side)?;
        let want = if focused { hue } else { dim };
        assert_eq!(border.fg, want, "the {name} border has the wrong colour");
    }
    Ok(())
}

#[test]
fn a_framed_pane_keeps_one_blank_column_inside_its_border() -> TestResult {
    let (mut app, pane) = console()?;
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Markdown {
        path: "/repo/NOTES.md".to_owned(),
        source: "hello".to_owned(),
    };
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let rect = hits(&app)?
        .panes
        .iter()
        .find(|(at, _)| *at == pane)
        .map(|(_, rect)| *rect)
        .ok_or("no pane rect")?;
    let y = rect.y.saturating_add(1);
    let row: Vec<String> = row_text(&terminal, y).chars().map(String::from).collect();
    let at = usize::from(rect.x);
    assert_eq!(row.get(at).map(String::as_str), Some("│"), "{row:?}");
    assert_eq!(row.get(at + 1).map(String::as_str), Some(" "), "{row:?}");
    assert_eq!(row.get(at + 2).map(String::as_str), Some("h"), "{row:?}");
    Ok(())
}

#[test]
fn the_palette_and_the_settings_tabs_use_no_accent_blue() -> TestResult {
    use ratatui::style::Modifier;
    let (mut app, _) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let tab = hits(&app)?
        .settings_tabs
        .iter()
        .find(|(_, _, tab)| *tab == 0)
        .map(|(rect, ..)| *rect)
        .ok_or("no first tab")?;
    let chip = cell_at(&terminal, tab.x.saturating_add(1), tab.y)?;
    assert_eq!(
        chip.bg,
        theme().selection_bg(),
        "the selected tab has no band"
    );
    assert!(chip.modifier.contains(Modifier::BOLD));
    assert!(!chip.modifier.contains(Modifier::REVERSED));
    app.state.mode = yi_console::model::Mode::Navigator {
        query: String::new(),
        selected: 0,
    };
    draw(&mut app, &mut terminal)?;
    let popup = *hits(&app)?.overlays.first().ok_or("no palette")?;
    let blue = theme().accent;
    for at in popup.positions() {
        let cell = cell_at(&terminal, at.x, at.y)?;
        assert!(
            cell.fg != blue && cell.bg != blue,
            "accent blue at {at:?}: {:?}",
            cell.symbol()
        );
    }
    let prompt = popup.y.saturating_add(1);
    let glyph = cell_at(&terminal, popup.x.saturating_add(2), prompt)?;
    assert_eq!(glyph.symbol(), "›");
    assert_eq!(glyph.fg, theme().warning, "the prompt glyph is not amber");
    let picked = (popup.y..popup.bottom())
        .find(|y| {
            cell_at(&terminal, popup.x.saturating_add(2), *y).is_ok_and(|c| c.symbol() == "▸")
        })
        .ok_or("no selected row")?;
    let band = cell_at(&terminal, popup.right().saturating_sub(3), picked)?;
    assert_eq!(
        band.bg,
        theme().selection_bg(),
        "the selected row has no band"
    );
    Ok(())
}

#[test]
fn the_header_row_carries_four_buttons_that_hover_and_open_their_panes() -> TestResult {
    use yi_console::keys::Action;
    use yi_console::model::Mode;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, _, _) = pair()?;
    draw(&mut app, &mut terminal)?;
    let buttons = hits(&app)?.header_buttons.clone();
    let actions: Vec<Action> = buttons.iter().map(|(_, action)| *action).collect();
    assert_eq!(
        actions,
        [
            Action::OpenTerminal,
            Action::ToggleDiff,
            Action::OpenBrowser,
            Action::OpenEditor
        ]
    );
    let top = row_text(&terminal, 0);
    for glyph in [">_", "±", "◉", "▤"] {
        assert!(top.contains(glyph), "no {glyph} in the header: {top}");
    }
    assert!(
        hits(&app)?.titles.iter().all(|(_, rect)| rect.y > 0),
        "a pane title sits in the header row"
    );
    let at = |action: Action| {
        buttons
            .iter()
            .find(|(_, a)| *a == action)
            .map(|(rect, _)| *rect)
            .ok_or("no such button")
    };
    let diff = at(Action::ToggleDiff)?;
    let face = |terminal: &Terminal<TestBackend>| {
        cell_at(terminal, diff.x, diff.y).map(|cell| (cell.fg, cell.bg))
    };
    let rest = face(&terminal)?;
    mouse(&mut app, MouseKind::Moved, diff.x, diff.y);
    assert!(app.dirty, "entering the diff button asks for no frame");
    draw(&mut app, &mut terminal)?;
    let lit = face(&terminal)?;
    assert_ne!(lit, rest, "the hovered diff button looks the same");
    let browser = at(Action::OpenBrowser)?;
    mouse(&mut app, MouseKind::Down, browser.x, browser.y);
    assert!(
        matches!(
            app.state.focused_pane_mut().map(|pane| &pane.content),
            Some(PaneContent::Browser(_))
        ),
        "the browser button opened no browser pane"
    );
    mouse(&mut app, MouseKind::Down, browser.x, browser.y);
    let files = at(Action::OpenEditor)?;
    mouse(&mut app, MouseKind::Down, files.x, files.y);
    assert!(
        matches!(&app.state.mode, Mode::Navigator { query, .. } if query == "e "),
        "files did not open the picker"
    );
    app.state.mode = Mode::Normal;
    mouse(&mut app, MouseKind::Down, diff.x, diff.y);
    let diffs = app
        .state
        .panes
        .values()
        .filter(|pane| matches!(pane.content, PaneContent::SessionDiff { .. }))
        .count();
    assert_eq!(diffs, 1, "the diff button opened no review pane");
    Ok(())
}

#[test]
fn header_buttons_are_worded_chips_with_a_blank_row_above_every_pane() -> TestResult {
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, _, _) = pair()?;
    draw(&mut app, &mut terminal)?;
    let top = row_text(&terminal, 0);
    for word in [">_ Terminal", "± Diff", "◉ Browser", "▤ Files"] {
        assert!(top.contains(word), "no {word:?} in the header: {top}");
    }
    let gap = row_text(&terminal, 1);
    assert!(
        !gap.contains('─') && !gap.contains('✕'),
        "the row under the header holds a frame: {gap}"
    );
    let closers = hits(&app)?.closers.clone();
    assert!(!closers.is_empty(), "no pane closer to check");
    assert!(
        closers.iter().all(|(_, rect)| rect.y >= 2),
        "a closer touches the header: {closers:?}"
    );
    let mut narrow = Terminal::new(TestBackend::new(40, 30))?;
    draw(&mut app, &mut narrow)?;
    let top = row_text(&narrow, 0);
    assert!(
        !top.contains("Terminal") && top.contains(">_"),
        "a narrow header kept words: {top}"
    );
    Ok(())
}

fn focus(app: &mut App, pane: PaneId) {
    if let Some(tab) = app.state.tab_mut() {
        tab.layout.focus_pane(pane);
    }
}

/// Dies with a title click leaving the finished session marked unseen: the pane body click clears it.
#[test]
fn a_click_on_a_pane_title_marks_its_finished_session_seen() -> TestResult {
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, left, right) = pair()?;
    let beta = SessionId("s-beta".to_owned());
    if let Some(row) = app.state.sessions.get_mut(&beta) {
        row.status = SessionStatus::DoneUnseen;
    }
    focus(&mut app, left);
    draw(&mut app, &mut terminal)?;
    let title = hits(&app)?
        .titles
        .iter()
        .find(|(pane, _)| *pane == right)
        .map(|(_, rect)| *rect)
        .ok_or("no title for the right pane")?;
    mouse(
        &mut app,
        MouseKind::Down,
        title.x.saturating_add(1),
        title.y,
    );
    assert_eq!(
        app.state.focused_pane_id(),
        Some(right),
        "focus did not move"
    );
    let status = app.state.sessions.get(&beta).map(|row| row.status);
    assert_eq!(
        status,
        Some(SessionStatus::Idle),
        "the title click left it unseen"
    );
    Ok(())
}

/// Dies with a press on the lower pane's title, on the split row, focusing it instead of resizing.
#[test]
fn a_press_on_the_lower_title_at_the_split_row_drags_the_split() -> TestResult {
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, top, bottom) = pair_in(Direction::Vertical)?;
    focus(&mut app, top);
    draw(&mut app, &mut terminal)?;
    let found = hits(&app)?;
    let split = found.splits.first().ok_or("no split border")?.pos;
    let title = found
        .titles
        .iter()
        .find(|(pane, _)| *pane == bottom)
        .map(|(_, rect)| *rect)
        .ok_or("no title for the lower pane")?;
    assert_eq!(title.y, split, "the title no longer sits on the split row");
    let height = |app: &App| {
        hits(app).map(|found| {
            found
                .panes
                .iter()
                .find(|(pane, _)| *pane == top)
                .map_or(0, |(_, rect)| rect.height)
        })
    };
    let before = height(&app)?;
    let x = title.x.saturating_add(1);
    mouse(&mut app, MouseKind::Down, x, title.y);
    mouse(&mut app, MouseKind::Drag, x, title.y.saturating_add(4));
    mouse(&mut app, MouseKind::Up, x, title.y.saturating_add(4));
    draw(&mut app, &mut terminal)?;
    assert_eq!(
        app.state.focused_pane_id(),
        Some(top),
        "the press focused the lower pane"
    );
    assert!(
        height(&app)? > before,
        "the split did not move from {before}"
    );
    Ok(())
}

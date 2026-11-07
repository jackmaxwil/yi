//! The console's chrome under the pointer: what a click on it does, what a hover on it shows,
//! and how the sidebar and pane titles are laid out.

use std::error::Error;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Direction, Rect};
use yi_console::app::{App, MouseKind};
use yi_console::client;
use yi_console::layout::PaneId;
use yi_console::model::{
    Link, PaneContent, SessionId, SessionRow, SessionStatus, SidebarMode, Zone,
};
use yi_console::settings::{Edit, Settings, Target, tab_name};
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
        pinned: None,
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

pub(crate) fn draw(app: &mut App, terminal: &mut Terminal<TestBackend>) -> TestResult {
    let theme = theme();
    terminal.draw(|frame| {
        let mut view = yi_console::render::compute_view(app, frame.area(), &theme);
        yi_console::render::render(app, frame, &mut view, &theme);
    })?;
    app.dirty = false;
    Ok(())
}

pub(crate) fn row_text(terminal: &Terminal<TestBackend>, y: u16) -> String {
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
    let (keys, _, target) = hits(&app)?
        .settings_targets
        .iter()
        .copied()
        .rfind(|(_, at, target)| *at == pane && matches!(target, Target::Tab(tab) if *tab > 0))
        .ok_or("no other tab has a click target")?;
    let Target::Tab(want) = target else {
        return Err("no other tab has a click target".into());
    };
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
        .settings_targets
        .iter()
        .copied()
        .rfind(|(_, at, target)| *at == pane && matches!(target, Target::Tab(tab) if *tab > 0))
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

fn settings_of(app: &App, pane: PaneId) -> Result<&Settings, Box<dyn Error>> {
    match app.state.panes.get(&pane).map(|pane| &pane.content) {
        Some(PaneContent::Settings(settings)) => Ok(settings),
        _ => Err("the settings pane went away".into()),
    }
}

/// The tab's rows, each beside the click target the last frame drew for it.
fn page_with_targets(
    app: &App,
    pane: PaneId,
    page: &[yi_types::settings::SettingRow],
) -> Result<Vec<(usize, Rect)>, Box<dyn Error>> {
    let targets = &hits(app)?.settings_targets;
    (0..page.len())
        .map(|index| {
            let at = targets
                .iter()
                .find(|(_, at, target)| *at == pane && *target == Target::Row(index))
                .map(|(rect, ..)| *rect)
                .ok_or(format!("row {index} has no click target"))?;
            Ok((index, at))
        })
        .collect()
}

/// Incident: the page's rows carried no click target, so a press on a setting began a text
/// selection and left the control alone; every row answers the pointer now.
#[test]
fn a_press_on_a_settings_row_works_the_control_it_names() -> TestResult {
    let (mut app, pane) = console()?;
    let mut settings = Settings::new(None, yi_types::settings::TABS.len());
    settings.rows = Some(yi_types::settings::rows());
    app.state.panes.get_mut(&pane).ok_or("no pane")?.content =
        PaneContent::Settings(Box::new(settings));
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let tab = tab_name(yi_types::settings::TABS.len());
    let page: Vec<_> = yi_types::settings::rows()
        .into_iter()
        .filter(|row| row.tab == tab)
        .collect();
    let rows = page_with_targets(&app, pane, &page)?;
    assert_eq!(rows.len(), page.len(), "a row on the page has no target");
    for (index, at) in &rows {
        let drawn = row_text(&terminal, at.y);
        assert!(
            drawn.contains(&page[*index].label),
            "row {index} sits at {} over {drawn:?}",
            at.y
        );
    }
    let find = |id: &str| -> Result<(usize, Rect), Box<dyn Error>> {
        let index = page
            .iter()
            .position(|row| row.id == id)
            .ok_or(format!("no {id} row"))?;
        let (index, at) = rows
            .iter()
            .find(|(at, _)| *at == index)
            .copied()
            .ok_or(format!("{id} has no target"))?;
        Ok((index, at))
    };
    let look = |terminal: &Terminal<TestBackend>, x: u16, y: u16| {
        terminal
            .backend()
            .buffer()
            .cell((x, y))
            .map(|cell| cell.style())
    };
    let (_, first) = find("console.autoSide")?;
    let rest = look(&terminal, first.x, first.y);
    mouse(&mut app, MouseKind::Moved, first.x, first.y);
    assert!(app.dirty, "moving onto a settings row asks for no frame");
    draw(&mut app, &mut terminal)?;
    assert_ne!(
        look(&terminal, first.x, first.y),
        rest,
        "the hovered row looks the same"
    );
    let (index, toggle) = find("console.autoSide")?;
    mouse(&mut app, MouseKind::Down, toggle.x, toggle.y);
    let settings = settings_of(&app, pane)?;
    assert_eq!(settings.cursor, index, "the press left the cursor alone");
    assert!(settings.edit.is_none(), "a toggle opened an editor");
    assert!(app.selection.is_none(), "the press began a text selection");
    let (index, field) = find("browser.path")?;
    mouse(&mut app, MouseKind::Down, field.x, field.y);
    let settings = settings_of(&app, pane)?;
    assert_eq!(settings.cursor, index, "the press left the cursor alone");
    assert!(
        matches!(&settings.edit, Some(Edit::Type(text)) if text.is_empty()),
        "a text row opened no editor"
    );
    let (index, number) = find("tui.pace")?;
    mouse(&mut app, MouseKind::Down, number.x, number.y);
    let settings = settings_of(&app, pane)?;
    assert_eq!(settings.cursor, index, "the press left the cursor alone");
    assert!(
        matches!(&settings.edit, Some(Edit::Type(_))),
        "a number row opened no editor"
    );
    let (_, json) = find("keys")?;
    mouse(&mut app, MouseKind::Down, json.x, json.y);
    let settings = settings_of(&app, pane)?;
    assert!(
        settings
            .status
            .as_deref()
            .is_some_and(|note| note.contains("config.json")),
        "a json row said nothing: {:?}",
        settings.status
    );
    Ok(())
}

/// The pane draws only a window of a long page; a wheel over it moves that window now, so a row
/// past the fold is drawn, hoverable and pressable with no key at all.
#[test]
fn a_wheel_over_a_settings_pane_brings_a_row_past_the_fold_into_reach() -> TestResult {
    let (mut app, pane) = console()?;
    let rows = yi_types::settings::rows();
    let tab = (1..=yi_types::settings::TABS.len())
        .max_by_key(|tab| rows.iter().filter(|row| row.tab == tab_name(*tab)).count())
        .ok_or("no config tab")?;
    let page: Vec<_> = rows
        .into_iter()
        .filter(|row| row.tab == tab_name(tab))
        .collect();
    let mut settings = Settings::new(None, tab);
    settings.rows = Some(yi_types::settings::rows());
    app.state.panes.get_mut(&pane).ok_or("no pane")?.content =
        PaneContent::Settings(Box::new(settings));
    let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
    draw(&mut app, &mut terminal)?;
    let last = page.len().saturating_sub(1);
    let target_of = |app: &App, row: usize| -> Option<Rect> {
        hits(app)
            .ok()?
            .settings_targets
            .iter()
            .find_map(|(rect, at, target)| {
                (*at == pane && *target == Target::Row(row)).then_some(*rect)
            })
    };
    assert!(
        target_of(&app, last).is_none(),
        "the page fits at 80x24, so no row is past the fold"
    );
    let at = hits(&app)?
        .panes
        .iter()
        .find_map(|(id, rect)| (*id == pane).then_some(*rect))
        .ok_or("no pane rect")?;
    let (x, y) = (
        at.x.saturating_add(at.width / 2),
        at.y.saturating_add(at.height / 2),
    );
    for _ in 0..page.len() {
        if target_of(&app, last).is_some() {
            break;
        }
        mouse(&mut app, MouseKind::ScrollDown, x, y);
        draw(&mut app, &mut terminal)?;
    }
    let rect = target_of(&app, last).ok_or("the wheel never brought the last row in")?;
    let drawn = row_text(&terminal, rect.y);
    assert!(
        drawn.contains(&page[last].label),
        "the last row's target sits at {} over {drawn:?}",
        rect.y
    );
    mouse(&mut app, MouseKind::Down, rect.x, rect.y);
    assert_eq!(
        settings_of(&app, pane)?.cursor,
        last,
        "the press missed the row the wheel brought in"
    );
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

/// Dies with the pane calling the reveal pace solo-only, though every console chat takes it.
#[test]
fn the_pane_says_a_console_restart_applies_the_reveal_pace() -> TestResult {
    let (mut app, pane) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let mut settings = Settings::new(None, yi_types::settings::TABS.len());
    settings.rows = Some(yi_types::settings::rows());
    app.state.panes.get_mut(&pane).ok_or("no pane")?.content =
        PaneContent::Settings(Box::new(settings));
    draw(&mut app, &mut terminal)?;
    let screen: Vec<_> = (0..30).map(|y| row_text(&terminal, y)).collect();
    let row = screen.iter().find(|text| text.contains("Reveal pace %"));
    assert!(
        row.is_some_and(|text| text.contains("next console")),
        "{screen:#?}"
    );
    Ok(())
}

#[test]
fn hovering_a_control_lifts_it_and_only_a_new_target_redraws() -> TestResult {
    let (mut app, pane) = console()?;
    app.state.sidebar = SidebarMode::Full;
    if let Some(row) = app.state.sessions.get_mut(&SessionId("s-alpha".to_owned())) {
        row.status = SessionStatus::Blocked;
    }
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let found = hits(&app)?;
    let session = found.sidebar_rows.first().ok_or("no sidebar row")?.0;
    let title = found.titles.iter().find(|(at, _)| *at == pane);
    let tab = found
        .settings_targets
        .iter()
        .find(|(_, _, target)| matches!(target, Target::Tab(tab) if *tab > 0));
    let controls = found
        .controls
        .iter()
        .map(|(rect, _)| ("sidebar control", *rect));
    assert_eq!(controls.len(), 4, "new, search, a header and its +");
    let targets: Vec<(&str, Rect)> = controls
        .chain([
            ("sidebar row", Rect::new(1, session, 1, 1)),
            ("gear", found.gear.ok_or("no gear")?),
            ("pane title", title.ok_or("no title target")?.1),
            ("closer", found.closers.first().ok_or("no closer")?.1),
            ("settings tab", tab.ok_or("no tab target")?.0),
        ])
        .collect();
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
        pinned: None,
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
        workdir: "/repo".to_owned(),
        drawn: None,
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
        .settings_targets
        .iter()
        .find(|(_, _, target)| *target == Target::Tab(0))
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
    let files = at(Action::OpenEditor)?;
    mouse(&mut app, MouseKind::Down, files.x, files.y);
    assert!(app.state.files.is_some(), "files did not open the tree");
    app.state.files = None;
    mouse(&mut app, MouseKind::Down, diff.x, diff.y);
    let diffs = app
        .state
        .panes
        .values()
        .filter(|pane| matches!(pane.content, PaneContent::SessionDiff { .. }))
        .count();
    assert_eq!(diffs, 1, "the diff button opened no review pane");
    let browser = at(Action::OpenBrowser)?;
    mouse(&mut app, MouseKind::Down, browser.x, browser.y);
    let browsers = (app.state.panes.values())
        .filter(|pane| matches!(pane.content, PaneContent::Browser(_)))
        .count();
    assert_eq!(browsers, 1, "the browser button opened no browser pane");
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

/// Dies when a press on the split row beside the lower pane's title stops resizing, or focuses the
/// lower pane: only the title label is a move handle there, the rest of the line is the split.
#[test]
fn a_press_on_the_split_row_beside_the_lower_title_still_drags_the_split() -> TestResult {
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
    let x = title.right().saturating_add(3);
    mouse(&mut app, MouseKind::Down, x, title.y);
    mouse(&mut app, MouseKind::Drag, x, title.y.saturating_add(4));
    mouse(&mut app, MouseKind::Up, x, title.y.saturating_add(4));
    draw(&mut app, &mut terminal)?;
    assert_eq!(
        app.state.focused_pane_id(),
        Some(top),
        "the press beside the title focused the lower pane"
    );
    assert!(
        height(&app)? > before,
        "the split did not move from {before}"
    );
    Ok(())
}

/// Dies when a press on a lower pane's title label on the split row resizes instead of lifting the
/// panel: dragged to the right edge it must land on the right half and leave the split alone.
#[test]
fn a_press_on_the_lower_title_label_moves_the_panel_instead_of_resizing() -> TestResult {
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, top, bottom) = pair_in(Direction::Vertical)?;
    focus(&mut app, top);
    draw(&mut app, &mut terminal)?;
    let found = hits(&app)?;
    let title = found
        .titles
        .iter()
        .find(|(pane, _)| *pane == bottom)
        .map(|(_, rect)| *rect)
        .ok_or("no title for the lower pane")?;
    let x = title.x.saturating_add(1);
    mouse(&mut app, MouseKind::Down, x, title.y);
    assert_eq!(
        app.state.focused_pane_id(),
        Some(bottom),
        "the press on the title did not focus its panel"
    );
    mouse(&mut app, MouseKind::Drag, 136, 12);
    mouse(&mut app, MouseKind::Up, 136, 12);
    draw(&mut app, &mut terminal)?;
    let rect_of = |app: &App, id| -> Result<Rect, Box<dyn Error>> {
        Ok(hits(app)?
            .panes
            .iter()
            .find(|(pane, _)| *pane == id)
            .ok_or("pane gone")?
            .1)
    };
    let (moved, stayed) = (rect_of(&app, bottom)?, rect_of(&app, top)?);
    assert!(
        moved.x > stayed.x && moved.height == stayed.height,
        "the panel did not move to the right half: {moved:?} beside {stayed:?}"
    );
    Ok(())
}

/// One client for the whole test: no daemon listens, so what a key sends goes nowhere.
fn no_daemon() -> (client::Outbound, client::ClientThreads) {
    let (events, _received) = client::events();
    let socket = std::env::temp_dir().join("yi-chrome-no-daemon-keys.sock");
    client::spawn(socket, events)
}

fn press(app: &mut App, outbound: &client::Outbound, code: KeyCode, modifiers: KeyModifiers) {
    crate::editor_view::key(app, outbound, KeyEvent::new(code, modifiers));
}

/// #1713: the third state gives its columns to the pane. #1923: the hint row names the chord back, so the header does not.
#[test]
fn a_hidden_sidebar_frees_its_width_and_leaves_the_header_bare() -> TestResult {
    let (mut app, _) = console()?;
    let (outbound, _threads) = no_daemon();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let shown = hits(&app)?.panes.first().ok_or("no pane")?.1;
    assert_eq!(hits(&app)?.sidebar_width, 8, "the rail is not drawn");
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(app.state.sidebar, SidebarMode::Hidden);
    draw(&mut app, &mut terminal)?;
    let found = hits(&app)?;
    assert_eq!(found.sidebar_width, 0, "the sidebar kept its columns");
    assert!(found.sidebar_rows.is_empty(), "a hidden sidebar drew rows");
    let freed = found.panes.first().ok_or("no pane")?.1;
    let edge = found.panes.iter().map(|(_, rect)| rect.right()).max();
    assert_eq!(edge, Some(140), "the panes stop short of the edge");
    assert!(
        freed.width > shown.width,
        "the pane kept its width: {} from {}",
        freed.width,
        shown.width
    );
    let header = row_text(&terminal, 0);
    assert!(
        !header.contains("sidebar"),
        "the header still names the sidebar: {header:?}"
    );
    Ok(())
}

/// #1713: one chord walks all three states, and focus never sits in a sidebar nobody draws.
#[test]
fn the_sidebar_chord_cycles_three_states_and_never_focuses_a_hidden_one() -> TestResult {
    let (mut app, _) = console()?;
    let (outbound, _threads) = no_daemon();
    app.state.sidebar = SidebarMode::Rail;
    for want in [SidebarMode::Full, SidebarMode::Hidden, SidebarMode::Rail] {
        press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(app.state.sidebar, want, "the cycle skipped {want:?}");
    }
    app.state.sidebar = SidebarMode::Rail;
    app.state.zone = Zone::Sidebar;
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(app.state.zone, Zone::Sidebar, "a drawn sidebar lost focus");
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(
        app.state.zone,
        Zone::Panes,
        "focus stayed in the hidden sidebar"
    );
    press(&mut app, &outbound, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(
        app.state.zone,
        Zone::Panes,
        "tab walked into a sidebar nobody draws"
    );
    Ok(())
}

/// #1713: a pane the user moved into while the sidebar was hidden keeps the keyboard.
#[test]
fn a_pane_taken_while_the_sidebar_was_hidden_keeps_the_keyboard() -> TestResult {
    let (mut app, _) = console()?;
    let (outbound, _threads) = no_daemon();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    app.state.sidebar = SidebarMode::Full;
    app.state.zone = Zone::Sidebar;
    draw(&mut app, &mut terminal)?;
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(app.state.sidebar, SidebarMode::Hidden);
    assert_eq!(
        app.state.zone,
        Zone::Panes,
        "focus stayed in the hidden sidebar"
    );
    draw(&mut app, &mut terminal)?;
    let (_, rect) = *hits(&app)?.panes.first().ok_or("no pane")?;
    mouse(
        &mut app,
        MouseKind::Down,
        rect.x.saturating_add(2),
        rect.y.saturating_add(2),
    );
    assert_eq!(app.state.zone, Zone::Panes);
    press(&mut app, &outbound, KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(app.state.sidebar, SidebarMode::Rail);
    assert_eq!(
        app.state.zone,
        Zone::Panes,
        "the chord took the keyboard back from the pane the user chose"
    );
    Ok(())
}

/// The owner's ask (#1738): the Files button opens a tree of the workspace, a folder folds by
/// click and a file opens in the editor; a gitignored folder never reaches the tree.
#[test]
fn the_files_button_opens_a_tree_that_folds_a_folder_and_opens_a_file() -> TestResult {
    use yi_console::keys::Action;
    let dir = crate::scratch::Scratch::new("yi-console-files")?;
    std::fs::create_dir_all(dir.join("crates/inner"))?;
    std::fs::create_dir_all(dir.join("build"))?;
    std::fs::write(dir.join("crates/inner/a.rs"), "pub fn a() {}\n")?;
    std::fs::write(dir.join("readme.md"), "# hi\n")?;
    std::fs::write(dir.join("build/skip.rs"), "ignored\n")?;
    std::fs::write(dir.join(".gitignore"), "build/\n")?;
    let root = dir.display().to_string();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, pane) = console()?;
    app.state.upsert_row(SessionRow {
        root: root.clone(),
        ..row_of("s-alpha", "alpha")
    });
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Session {
        session: Some(SessionId("s-alpha".to_owned())),
        chat: None,
    };
    draw(&mut app, &mut terminal)?;
    let files = hits(&app)?
        .header_buttons
        .iter()
        .find(|(_, action)| *action == Action::OpenEditor)
        .map(|(rect, _)| *rect)
        .ok_or("no Files button")?;
    mouse(&mut app, MouseKind::Down, files.x, files.y);
    assert!(app.state.files.is_some(), "the Files button opened no tree");
    draw(&mut app, &mut terminal)?;
    let popup = hits(&app)?.files.ok_or("no tree sidebar")?;
    assert_eq!(
        popup.right(),
        140,
        "the tree is not on the right edge: {popup:?}"
    );
    let leaf = std::path::Path::new(&root)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let title = row_text(&terminal, popup.y);
    let seen: String = leaf.to_uppercase().chars().take(8).collect();
    assert!(
        title.contains(&seen),
        "the tree is not rooted at the focused session's dir: {title}"
    );
    let row_of = |terminal: &Terminal<TestBackend>, needle: &str| -> Option<u16> {
        (popup.y.saturating_add(1)..popup.bottom().saturating_sub(1))
            .find(|y| row_text(terminal, *y).contains(needle))
    };
    let drawn: Vec<String> = (popup.y.saturating_add(1)..popup.bottom().saturating_sub(1))
        .map(|y| row_text(&terminal, y))
        .collect();
    assert!(
        drawn.iter().any(|row| row.contains("crates")),
        "no crates row in {drawn:?}"
    );
    assert!(
        drawn.iter().all(|row| !row.contains("skip.rs")),
        "the gitignored folder reached the tree: {drawn:?}"
    );
    let (events, _received) = client::events();
    let socket = std::env::temp_dir().join("yi-chrome-no-daemon.sock");
    let (outbound, _threads) = client::spawn(socket, events);
    let inner = row_of(&terminal, "inner").ok_or("no inner row")?;
    mouse(&mut app, MouseKind::Down, popup.x.saturating_add(3), inner);
    draw(&mut app, &mut terminal)?;
    assert!(
        row_of(&terminal, "a.rs").is_some(),
        "the clicked folder did not unfold"
    );
    crate::review::press(&mut app, &outbound, KeyCode::Enter);
    draw(&mut app, &mut terminal)?;
    assert!(
        row_of(&terminal, "a.rs").is_none(),
        "⏎ on a folder did not fold it"
    );
    crate::review::press(&mut app, &outbound, KeyCode::Enter);
    draw(&mut app, &mut terminal)?;
    assert!(
        row_of(&terminal, "a.rs").is_some(),
        "⏎ did not unfold the folder again"
    );
    let before = app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    crate::review::press(&mut app, &outbound, KeyCode::Down);
    crate::review::press(&mut app, &outbound, KeyCode::Enter);
    assert!(
        app.state.files.as_ref().is_some_and(|files| !files.focused),
        "⏎ on a file left the keys with the tree"
    );
    assert_eq!(
        app.state.tab().map_or(0, |tab| tab.layout.pane_count()),
        before + 1,
        "the file replaced a pane instead of opening beside it"
    );
    let editor = app
        .state
        .panes
        .values()
        .find_map(|pane| match &pane.content {
            PaneContent::Editor(editor) => Some(editor.path.clone()),
            _ => None,
        })
        .ok_or("no editor pane opened")?;
    assert!(
        editor.ends_with("crates/inner/a.rs"),
        "⏎ opened {editor}, not the chosen file"
    );
    draw(&mut app, &mut terminal)?;
    let files = hits(&app)?
        .header_buttons
        .iter()
        .find(|(_, action)| *action == Action::OpenEditor)
        .map(|(rect, _)| *rect)
        .ok_or("no Files button")?;
    mouse(&mut app, MouseKind::Down, files.x, files.y);
    assert!(
        app.state.files.as_ref().is_some_and(|files| files.focused),
        "the Files button did not hand the tree the keys back"
    );
    crate::review::press(&mut app, &outbound, KeyCode::Esc);
    outbound.shutdown();
    assert!(app.state.files.is_none(), "Esc left the tree up");
    Ok(())
}

/// Dies when a mouse user cannot work the tree: a click on a folder row must unfold it, on a file
/// row must open it in a panel beside the chat, a second file must reuse that panel rather than
/// stack another, and a press on a pane must hand the keys back while the tree stays up.
#[test]
fn the_files_sidebar_works_by_mouse_and_opens_files_in_one_panel() -> TestResult {
    use yi_console::keys::Action;
    let dir = crate::scratch::Scratch::new("yi-console-files-mouse")?;
    std::fs::create_dir_all(dir.join("src/inner"))?;
    std::fs::write(dir.join("src/inner/a.rs"), "pub fn a() {}\n")?;
    std::fs::write(dir.join("src/inner/b.rs"), "pub fn b() {}\n")?;
    let root = dir.display().to_string();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, pane) = console()?;
    app.state.upsert_row(SessionRow {
        root,
        ..row_of("s-alpha", "alpha")
    });
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Session {
        session: Some(SessionId("s-alpha".to_owned())),
        chat: None,
    };
    draw(&mut app, &mut terminal)?;
    let button = hits(&app)?
        .header_buttons
        .iter()
        .find(|(_, action)| *action == Action::OpenEditor)
        .map(|(rect, _)| *rect)
        .ok_or("no Files button")?;
    mouse(&mut app, MouseKind::Down, button.x, button.y);
    draw(&mut app, &mut terminal)?;
    let side = hits(&app)?.files.ok_or("no sidebar")?;
    let row_of_text = |terminal: &Terminal<TestBackend>, needle: &str| -> Option<u16> {
        (side.y.saturating_add(1)..side.bottom().saturating_sub(1))
            .find(|y| row_text(terminal, *y).contains(needle))
    };
    let inner = row_of_text(&terminal, "inner").ok_or("no inner row")?;
    mouse(&mut app, MouseKind::Down, side.x.saturating_add(3), inner);
    draw(&mut app, &mut terminal)?;
    assert!(
        row_of_text(&terminal, "a.rs").is_some(),
        "a click on the folder row left it folded"
    );
    let panes = |app: &App| app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    let before = panes(&app);
    let a = row_of_text(&terminal, "a.rs").ok_or("no a.rs row")?;
    mouse(&mut app, MouseKind::Down, side.x.saturating_add(5), a);
    draw(&mut app, &mut terminal)?;
    assert_eq!(panes(&app), before + 1, "a file click opened no panel");
    assert!(
        hits(&app)?.files.is_some(),
        "opening a file took the sidebar away"
    );
    let b = row_of_text(&terminal, "b.rs").ok_or("no b.rs row")?;
    mouse(&mut app, MouseKind::Down, side.x.saturating_add(5), b);
    assert_eq!(
        panes(&app),
        before + 1,
        "a second file stacked a second panel"
    );
    let open = app
        .state
        .panes
        .values()
        .find_map(|pane| match &pane.content {
            PaneContent::Editor(editor) => Some(editor.path.clone()),
            _ => None,
        })
        .ok_or("no editor")?;
    assert!(open.ends_with("inner/b.rs"), "the panel still shows {open}");
    mouse(&mut app, MouseKind::Down, 70, 12);
    assert!(
        app.state.files.as_ref().is_some_and(|files| !files.focused),
        "a press on a pane left the keys with the tree"
    );
    let tool = |back: u16| side.right().saturating_sub(back);
    draw(&mut app, &mut terminal)?;
    mouse(&mut app, MouseKind::Down, tool(7), side.y);
    draw(&mut app, &mut terminal)?;
    assert!(
        row_of_text(&terminal, "inner").is_none(),
        "the collapse-all button left a folder open"
    );
    mouse(&mut app, MouseKind::Down, tool(3), side.y);
    assert!(app.state.files.is_none(), "the ✕ left the sidebar up");
    Ok(())
}

/// Dies when an empty notebook is blank, when its start rows are not the control table's rows, or
/// when a press on one does nothing: the row for the palette must open the palette.
#[test]
fn an_empty_notebook_shows_start_rows_that_a_press_works() -> TestResult {
    use yi_console::keys::Action;
    use yi_console::model::Mode;
    use yi_console::render::Hit;
    let mut app = App::new("/repo".to_owned(), theme());
    app.state.link = Link::Connected;
    let notebook = app
        .state
        .split_focused(Direction::Horizontal)
        .ok_or("no split")?;
    let slot = app.state.panes.get_mut(&notebook).ok_or("no pane")?;
    slot.content = PaneContent::Notebook {
        session: None,
        full: false,
        drawn: None,
    };
    app.state.zone = Zone::Panes;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let rect = hits(&app)?
        .controls
        .iter()
        .find(|(_, hit)| *hit == Hit::Button(Action::Navigator))
        .map(|(rect, _)| *rect)
        .ok_or("no palette row in the control table")?;
    let row = row_text(&terminal, rect.y);
    assert!(
        row.contains("command palette"),
        "the control row is not the palette's: {row}"
    );
    let all: String = (0..30)
        .map(|y| row_text(&terminal, y))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("no kernel cells yet"), "{all}");
    mouse(&mut app, MouseKind::Down, rect.x, rect.y);
    assert!(
        matches!(app.state.mode, Mode::Navigator { .. }),
        "a press on the palette row opened nothing"
    );
    Ok(())
}

/// Dies when a drag from a session row shows no lit zone under the pointer, when the drop does not
/// make a panel on that half of the screen, when a press that never moved stops being a click, or
/// when Esc does not cancel a drag in flight.
#[test]
fn a_drag_from_a_session_row_lights_the_zone_and_drops_a_panel_there() -> TestResult {
    let (mut app, _) = console()?;
    let (outbound, _threads) = no_daemon();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    app.state.sidebar = SidebarMode::Full;
    draw(&mut app, &mut terminal)?;
    let row = hits(&app)?.sidebar_rows.first().ok_or("no session row")?.0;
    let panes = |app: &App| app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    let before = panes(&app);
    mouse(&mut app, MouseKind::Down, 3, row);
    mouse(&mut app, MouseKind::Up, 3, row);
    assert_eq!(panes(&app), before, "a click that never moved made a panel");
    mouse(&mut app, MouseKind::Down, 3, row);
    mouse(&mut app, MouseKind::Drag, 136, 12);
    draw(&mut app, &mut terminal)?;
    let lit = cell_at(&terminal, 130, 5)?.bg;
    assert_eq!(lit, theme().selection_bg(), "the right half is not lit");
    assert_ne!(
        cell_at(&terminal, 60, 5)?.bg,
        lit,
        "the left half is lit as well"
    );
    let words: String = (0..30)
        .map(|y| row_text(&terminal, y))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(words.contains("right half"), "no label on the lit zone");
    press(&mut app, &outbound, KeyCode::Esc, KeyModifiers::NONE);
    mouse(&mut app, MouseKind::Up, 136, 12);
    assert_eq!(panes(&app), before, "Esc did not cancel the drag");
    mouse(&mut app, MouseKind::Down, 3, row);
    mouse(&mut app, MouseKind::Drag, 136, 12);
    mouse(&mut app, MouseKind::Up, 136, 12);
    assert_eq!(panes(&app), before + 1, "the drop made no panel");
    draw(&mut app, &mut terminal)?;
    let area_left = hits(&app)?
        .panes
        .iter()
        .map(|(_, r)| r.x)
        .min()
        .ok_or("no panes")?;
    let newest = hits(&app)?
        .panes
        .iter()
        .map(|(_, r)| r.x)
        .max()
        .ok_or("no panes")?;
    let middle = area_left + (140 - area_left) / 2;
    assert!(
        newest.abs_diff(middle) <= 2,
        "the new panel starts at {newest}, not the right half ({middle})"
    );
    Ok(())
}

/// Dies when ⌘⌥G leaves the focused panel anywhere but the right third of the screen.
#[test]
fn the_panel_chords_move_the_focused_panel_to_its_third() -> TestResult {
    let (mut app, pane) = console()?;
    let (outbound, _threads) = no_daemon();
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    app.state.tab_mut().ok_or("no tab")?.layout.focus_pane(pane);
    let chord = KeyModifiers::SUPER | KeyModifiers::ALT;
    press(&mut app, &outbound, KeyCode::Char('g'), chord);
    draw(&mut app, &mut terminal)?;
    let moved = hits(&app)?
        .panes
        .iter()
        .find(|(id, _)| *id == pane)
        .ok_or("pane gone")?
        .1;
    let all = hits(&app)?
        .panes
        .iter()
        .map(|(_, r)| *r)
        .reduce(|a, b| a.union(b))
        .ok_or("no panes")?;
    assert_eq!(moved.height, all.height, "the panel is not full height");
    assert!(
        moved.right() >= all.right().saturating_sub(1) && moved.width.abs_diff(all.width / 3) <= 2,
        "not the right third: {moved:?} of {all:?}"
    );
    Ok(())
}

/// Dies when the terminal's pointer stays an arrow over a button or a panel edge: each change of
/// what is under the pointer asks the terminal (OSC 22) for the matching shape, once.
#[test]
fn the_pointer_takes_the_shape_of_what_is_under_it() -> TestResult {
    use yi_console::keys::Action;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, _, _) = pair()?;
    draw(&mut app, &mut terminal)?;
    let button = hits(&app)?
        .header_buttons
        .iter()
        .find(|(_, action)| *action == Action::OpenTerminal)
        .map(|(rect, _)| *rect)
        .ok_or("no Terminal button")?;
    let edge = hits(&app)?.splits.first().ok_or("no split border")?.clone();
    let asked = |app: &mut App| app.osc_out.drain(..).collect::<Vec<_>>().concat();
    mouse(&mut app, MouseKind::Moved, button.x, button.y);
    assert!(
        asked(&mut app).contains("]22;pointer"),
        "no pointer over a button"
    );
    mouse(&mut app, MouseKind::Moved, button.x, button.y);
    assert_eq!(asked(&mut app), "", "the same shape was asked for twice");
    mouse(
        &mut app,
        MouseKind::Moved,
        edge.pos,
        edge.area.y.saturating_add(5),
    );
    assert!(
        asked(&mut app).contains("]22;ew-resize"),
        "no resize over the edge"
    );
    mouse(&mut app, MouseKind::Moved, 70, 14);
    assert!(
        asked(&mut app).contains("]22;default"),
        "no arrow over a pane"
    );
    Ok(())
}

/// Dies when a second file replaces the first instead of joining it as a tab, when a click on a tab
/// or its ✕ does nothing, when a tab dragged to an edge does not become its own panel, or when one
/// dropped onto another editor panel does not join its strip.
#[test]
fn files_open_as_sticky_tabs_that_switch_close_split_out_and_merge() -> TestResult {
    use yi_console::render::Hit;
    let dir = crate::scratch::Scratch::new("yi-console-tabs")?;
    std::fs::create_dir_all(dir.join("src/inner"))?;
    for name in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(dir.join("src/inner").join(name), format!("// {name}\n"))?;
    }
    let mut terminal = Terminal::new(TestBackend::new(220, 30))?;
    let (mut app, pane) = console()?;
    app.state.upsert_row(SessionRow {
        root: dir.display().to_string(),
        ..row_of("s-alpha", "alpha")
    });
    let slot = app.state.panes.get_mut(&pane).ok_or("no pane")?;
    slot.content = PaneContent::Session {
        session: Some(SessionId("s-alpha".to_owned())),
        chat: None,
    };
    draw(&mut app, &mut terminal)?;
    let button = hits(&app)?
        .header_buttons
        .iter()
        .find(|(_, action)| *action == yi_console::keys::Action::OpenEditor)
        .map(|(rect, _)| *rect)
        .ok_or("no Files button")?;
    mouse(&mut app, MouseKind::Down, button.x, button.y);
    draw(&mut app, &mut terminal)?;
    let side = hits(&app)?.files.ok_or("no sidebar")?;
    let row_of = |terminal: &Terminal<TestBackend>, needle: &str| -> Option<u16> {
        (side.y.saturating_add(1)..side.bottom()).find(|y| row_text(terminal, *y).contains(needle))
    };
    let inner = row_of(&terminal, "inner").ok_or("no inner row")?;
    mouse(&mut app, MouseKind::Down, side.x.saturating_add(3), inner);
    draw(&mut app, &mut terminal)?;
    let before = app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    for name in ["a.rs", "b.rs", "c.rs"] {
        let y = row_of(&terminal, name).ok_or("no file row")?;
        mouse(&mut app, MouseKind::Down, side.x.saturating_add(6), y);
        draw(&mut app, &mut terminal)?;
    }
    let panes = |app: &App| app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    assert_eq!(
        panes(&app),
        before + 1,
        "three files made more than one panel"
    );
    let editor = |app: &App| {
        app.state
            .panes
            .iter()
            .find_map(|(id, pane)| match &pane.content {
                PaneContent::Editor(tabs) => Some((*id, tabs.len(), tabs.path.clone())),
                _ => None,
            })
    };
    let (group, count, shown) = editor(&app).ok_or("no editor panel")?;
    assert_eq!(count, 3, "the files did not stay as tabs");
    assert!(
        shown.ends_with("c.rs"),
        "the newest file is not showing: {shown}"
    );
    let strip = row_text(
        &terminal,
        hits(&app)?
            .panes
            .iter()
            .find(|(id, _)| *id == group)
            .map_or(0, |(_, r)| r.y.saturating_add(1)),
    );
    for name in ["a.rs", "b.rs", "c.rs"] {
        assert!(strip.contains(name), "no {name} tab in {strip:?}");
    }
    let rect_of = |app: &App, want: Hit| -> Option<Rect> {
        app.hits
            .as_ref()?
            .controls
            .iter()
            .find(|(_, hit)| *hit == want)
            .map(|(rect, _)| *rect)
    };
    let first = rect_of(&app, Hit::EditorTab(group, 0)).ok_or("no first tab")?;
    mouse(
        &mut app,
        MouseKind::Down,
        first.x.saturating_add(1),
        first.y,
    );
    mouse(&mut app, MouseKind::Up, first.x.saturating_add(1), first.y);
    assert!(
        editor(&app).ok_or("no editor")?.2.ends_with("a.rs"),
        "a click on a tab did not show it"
    );
    let close = rect_of(&app, Hit::EditorClose(group, 1)).ok_or("no ✕ on the second tab")?;
    mouse(
        &mut app,
        MouseKind::Down,
        close.x.saturating_add(1),
        close.y,
    );
    assert_eq!(
        editor(&app).ok_or("no editor")?.1,
        2,
        "the ✕ did not close a tab"
    );
    draw(&mut app, &mut terminal)?;
    let second = rect_of(&app, Hit::EditorTab(group, 1)).ok_or("no second tab")?;
    mouse(
        &mut app,
        MouseKind::Down,
        second.x.saturating_add(1),
        second.y,
    );
    let edge = hits(&app)?
        .panes
        .iter()
        .map(|(_, rect)| rect.right())
        .max()
        .ok_or("no panes")?
        .saturating_sub(3);
    mouse(&mut app, MouseKind::Drag, edge, 12);
    mouse(&mut app, MouseKind::Up, edge, 12);
    assert_eq!(
        panes(&app),
        before + 2,
        "a tab dragged to the edge made no panel"
    );
    let counts: Vec<usize> = app
        .state
        .panes
        .values()
        .filter_map(|pane| match &pane.content {
            PaneContent::Editor(tabs) => Some(tabs.len()),
            _ => None,
        })
        .collect();
    assert_eq!(
        counts.iter().sum::<usize>(),
        2,
        "a file was lost: {counts:?}"
    );
    assert!(
        counts.iter().all(|n| *n == 1),
        "the split left two tabs together: {counts:?}"
    );
    draw(&mut app, &mut terminal)?;
    let moved = rect_of(
        &app,
        Hit::EditorTab(
            app.state
                .panes
                .iter()
                .find_map(|(id, pane)| {
                    (*id != group && matches!(pane.content, PaneContent::Editor(_))).then_some(*id)
                })
                .ok_or("no second editor")?,
            0,
        ),
    )
    .ok_or("no tab in the new panel")?;
    let target = hits(&app)?
        .panes
        .iter()
        .find(|(id, _)| *id == group)
        .map(|(_, r)| *r)
        .ok_or("no target panel")?;
    mouse(
        &mut app,
        MouseKind::Down,
        moved.x.saturating_add(1),
        moved.y,
    );
    mouse(
        &mut app,
        MouseKind::Drag,
        target.x + target.width / 2,
        target.y + target.height / 2,
    );
    mouse(
        &mut app,
        MouseKind::Up,
        target.x + target.width / 2,
        target.y + target.height / 2,
    );
    assert_eq!(panes(&app), before + 1, "the merged panel did not close");
    assert_eq!(
        editor(&app).ok_or("no editor")?.1,
        2,
        "the dropped tab did not join the strip"
    );
    Ok(())
}

/// Dies when a drag to the bottom strip lights only a half, when the third under the pointer is not
/// the one lit and dropped, or when the thirds are not named before the pointer is over one.
#[test]
fn a_drag_to_the_bottom_strip_lights_and_drops_a_third() -> TestResult {
    let (mut app, _) = console()?;
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    app.state.sidebar = SidebarMode::Full;
    draw(&mut app, &mut terminal)?;
    let row = hits(&app)?.sidebar_rows.first().ok_or("no session row")?.0;
    let area = hits(&app)?
        .panes
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|all, rect| all.union(rect))
        .ok_or("no panes")?;
    let strip = area.bottom().saturating_sub(1);
    let panes = |app: &App| app.state.tab().map_or(0, |tab| tab.layout.pane_count());
    let before = panes(&app);
    for (third, label) in [
        (0_u16, "left third"),
        (1, "middle third"),
        (2, "right third"),
    ] {
        let x = area.x + area.width / 3 * third + area.width / 6;
        mouse(&mut app, MouseKind::Down, 3, row);
        mouse(&mut app, MouseKind::Drag, x, strip);
        draw(&mut app, &mut terminal)?;
        let lit = cell_at(&terminal, x, area.y + 3)?.bg;
        assert_eq!(lit, theme().selection_bg(), "the {label} is not lit");
        let words: String = (0..30)
            .map(|y| row_text(&terminal, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(words.contains(label), "no {label} label in {words}");
        for other in ["left third", "middle third", "right third"] {
            assert!(
                other == label || words.contains(other),
                "the {other} guide is not named while the {label} is lit"
            );
        }
        let far_half = if third == 0 {
            "right half"
        } else {
            "left half"
        };
        assert!(
            words.contains(far_half),
            "no {far_half} guide while dragging"
        );
        mouse(&mut app, MouseKind::Up, x, strip);
        assert_eq!(panes(&app), before + 1, "the {label} drop made no panel");
        draw(&mut app, &mut terminal)?;
        let dropped = app.state.focused_pane_id().ok_or("no focused panel")?;
        let width = hits(&app)?
            .panes
            .iter()
            .find(|(id, _)| *id == dropped)
            .map(|(_, rect)| rect.width)
            .ok_or("the dropped panel is not drawn")?;
        assert!(
            width.abs_diff(area.width / 3) <= 2,
            "the {label} panel is {width} wide, not a third of {}",
            area.width
        );
        app.state.tab_mut().ok_or("no tab")?.layout.close_focused();
        draw(&mut app, &mut terminal)?;
        assert_eq!(panes(&app), before, "the panel did not close");
    }
    Ok(())
}

/// Dies when a change of pointer shape waits in the queue for a frame: a move that changes nothing
/// on screen draws none, so the shape has to be written as the event is handled.
#[test]
fn a_pointer_shape_is_written_without_waiting_for_a_frame() -> TestResult {
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    let (mut app, _, _) = pair()?;
    draw(&mut app, &mut terminal)?;
    let edge = hits(&app)?.splits.first().ok_or("no split border")?.clone();
    app.osc_out.clear();
    mouse(
        &mut app,
        MouseKind::Moved,
        edge.pos,
        edge.area.y.saturating_add(5),
    );
    assert!(
        !app.dirty,
        "the move asked for a frame, so the test shows nothing"
    );
    let mut sent = Vec::new();
    app.flush_osc(&mut sent);
    let sent = String::from_utf8(sent)?;
    assert!(
        sent.contains("]22;ew-resize"),
        "the shape was not written: {sent:?}"
    );
    assert!(app.osc_out.is_empty(), "the queue kept what it wrote");
    Ok(())
}

fn sessions_in(app: &mut App, root: &str, names: impl Iterator<Item = String>) {
    for name in names {
        app.state.upsert_row(SessionRow {
            id: SessionId(format!("s-{root}-{name}")),
            root: root.to_owned(),
            name: Some(name),
            ..row_of("s-x", "x")
        });
    }
}

fn sidebar_text(terminal: &Terminal<TestBackend>, height: u16) -> Vec<String> {
    (0..height)
        .map(|y| row_text(terminal, y).chars().take(26).collect::<String>())
        .collect()
}

/// Dies when the wheel over the session sidebar does nothing, or drags the cursor along with it.
#[test]
fn the_wheel_slides_the_session_sidebar_and_leaves_the_cursor() -> TestResult {
    let (mut app, _) = console()?;
    app.state.sidebar = SidebarMode::Full;
    sessions_in(
        &mut app,
        "/repo",
        (0..60).map(|n| format!("session {n:02}")),
    );
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let before = sidebar_text(&terminal, 30);
    let cursor = app.state.selected;
    for _ in 0..4 {
        mouse(&mut app, MouseKind::ScrollDown, 3, 10);
    }
    draw(&mut app, &mut terminal)?;
    let after = sidebar_text(&terminal, 30);
    assert_ne!(before, after, "the wheel left the sidebar where it was");
    assert_eq!(app.state.selected, cursor, "the wheel moved the cursor");
    for _ in 0..40 {
        mouse(&mut app, MouseKind::ScrollUp, 3, 10);
    }
    draw(&mut app, &mut terminal)?;
    assert_eq!(
        sidebar_text(&terminal, 30),
        before,
        "the wheel did not come back"
    );
    Ok(())
}

/// Dies when a project cannot be moved: ⌥⇧↓ on its header, or a drag of its header over the next
/// one, must swap their places in the sidebar.
#[test]
fn a_project_moves_by_key_and_by_dragging_its_header() -> TestResult {
    use yi_console::render::Hit;
    let (mut app, _) = console()?;
    let (outbound, _threads) = no_daemon();
    app.state.sidebar = SidebarMode::Full;
    sessions_in(&mut app, "/aaa", ["one".to_owned()].into_iter());
    sessions_in(&mut app, "/bbb", ["two".to_owned()].into_iter());
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let roots = |app: &App| {
        app.state
            .groups()
            .into_iter()
            .filter_map(|group| group.root)
            .collect::<Vec<_>>()
    };
    let start = roots(&app);
    let (first, second) = (
        start.first().ok_or("no group")?.clone(),
        start.get(1).ok_or("one group")?.clone(),
    );
    app.state.header = Some(first.clone());
    press(
        &mut app,
        &outbound,
        KeyCode::Down,
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    );
    assert_eq!(
        roots(&app).get(1),
        Some(&first),
        "⌥⇧↓ did not move {first} down"
    );
    draw(&mut app, &mut terminal)?;
    let header = |app: &App, root: &str| {
        app.hits.as_ref().and_then(|hits| {
            hits.controls
                .iter()
                .find(|(_, hit)| matches!(hit, Hit::Fold(known) if known == root))
                .map(|(rect, _)| *rect)
        })
    };
    let from = header(&app, &first).ok_or("no header to drag")?;
    let over = header(&app, &second).ok_or("no header to drag over")?;
    mouse(&mut app, MouseKind::Down, from.x + 2, from.y);
    mouse(&mut app, MouseKind::Drag, from.x + 2, over.y);
    mouse(&mut app, MouseKind::Up, from.x + 2, over.y);
    assert_eq!(
        roots(&app).first(),
        Some(&first),
        "dragging the header did not move {first} back up"
    );
    Ok(())
}

/// Dies when the New session row starts a session in a directory nobody chose, though several
/// projects are listed: it must ask which one.
#[test]
fn the_new_session_row_asks_which_project_when_there_are_several() -> TestResult {
    use yi_console::model::Mode;
    use yi_console::render::Hit;
    let (mut app, _) = console()?;
    app.state.sidebar = SidebarMode::Full;
    sessions_in(&mut app, "/other", ["x".to_owned()].into_iter());
    let mut terminal = Terminal::new(TestBackend::new(140, 30))?;
    draw(&mut app, &mut terminal)?;
    let row = app
        .hits
        .as_ref()
        .and_then(|hits| {
            hits.controls
                .iter()
                .find(|(_, hit)| matches!(hit, Hit::New(None)))
                .map(|(rect, _)| *rect)
        })
        .ok_or("no New session row")?;
    mouse(&mut app, MouseKind::Down, row.x + 3, row.y);
    mouse(&mut app, MouseKind::Up, row.x + 3, row.y);
    let Mode::Menu(menu) = &app.state.mode else {
        return Err("the New session row opened no chooser".into());
    };
    let items = menu.levels.first().map_or(0, |level| level.items.len());
    assert!(items >= 2, "the chooser lists {items} directories");
    Ok(())
}

#[test]
fn files_never_stays_focused_where_it_has_no_width() -> TestResult {
    let (mut app, outbound) = crate::editor_view::console();
    crate::editor_view::key(
        &mut app,
        &outbound,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
    );
    assert!(app.state.files.is_some(), "the tree did not open");
    let mut terminal = Terminal::new(TestBackend::new(59, 30))?;
    draw(&mut app, &mut terminal)?;
    assert!(app.state.files.is_none(), "an invisible tree kept focus");
    crate::editor_view::key(
        &mut app,
        &outbound,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
    );
    assert!(app.state.files.is_none(), "an invisible tree opened");
    assert!(
        app.state
            .banner
            .as_ref()
            .is_some_and(|note| note.contains("60")),
        "the refusal does not name the minimum width"
    );
    outbound.shutdown();
    Ok(())
}

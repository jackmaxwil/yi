//! The Panes panel's list: the session's panes under the place each is in, a press that
//! goes to one, a cross that closes one, and the kinds with something to show not yet open.

use yd_core::grid::layout::Size;
use yd_core::grid::step::GridMsg;
use yd_core::ids::PaneId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_ui::grid_view::view::Grips;
use yd_ui::win::dispatch;

use super::panes::{PANEL, kinds, open};
use super::*;

/// Whether `key` is a row of the list: a pane's or a kind's, not a part of one.
fn is_row(key: &str) -> bool {
    let pane = key.strip_prefix("grid.panel.pane-");
    pane.is_some_and(|rest| !rest.contains('.')) || key.starts_with("grid.panel.kind.")
}

/// The key of the list's row for the pane called `name`.
pub(super) fn row_of(runtime: &Runtime<Win>, name: &str) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let said = format!("{name}, ");
    let named = |e: &&ElementInfo| {
        let label = e.label.as_deref().unwrap_or_default();
        !e.exiting && e.key.as_deref().is_some_and(is_row) && label.starts_with(&said)
    };
    let row = shown.iter().find(named).and_then(|e| e.key.clone());
    row.ok_or_else(|| format!("no row for {name}").into())
}

fn pane_of(row: &str) -> Result<PaneId, Box<dyn Error>> {
    let number = row
        .strip_prefix("grid.panel.pane-")
        .ok_or("not a pane's row")?;
    Ok(PaneId(number.parse()?))
}

/// The place a heading of the list names, "closed" for the kinds not open.
fn heading(key: &str) -> Option<&str> {
    let place = key.strip_prefix("grid.panel.place.");
    place.or((key == "grid.panel.closed").then_some("closed"))
}

/// Each heading of the list with what the rows under it say, top to bottom.
fn by_place(runtime: &Runtime<Win>) -> Vec<(String, Vec<String>)> {
    let shown = runtime.elements();
    let mut parts: Vec<&ElementInfo> = shown
        .iter()
        .filter(|e| !e.exiting)
        .filter(|e| {
            let key = e.key.as_deref().unwrap_or_default();
            heading(key).is_some() || is_row(key)
        })
        .collect();
    parts.sort_by(|a, b| {
        (a.rect[1], a.rect[0])
            .partial_cmp(&(b.rect[1], b.rect[0]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut list: Vec<(String, Vec<String>)> = Vec::new();
    for part in parts {
        let key = part.key.as_deref().unwrap_or_default();
        match (heading(key), list.last_mut()) {
            (Some(place), _) => list.push((place.to_owned(), Vec::new())),
            (None, Some((_, rows))) => rows.push(part.label.clone().unwrap_or_default()),
            (None, None) => {}
        }
    }
    list
}

fn under(list: &[(String, Vec<String>)], place: &str) -> Vec<String> {
    let found = list.iter().find(|(heading, _)| heading == place);
    found.map(|(_, rows)| rows.clone()).unwrap_or_default()
}

/// The pane with the keys in the main window.
fn focus(runtime: &Runtime<Win>) -> Result<Option<PaneId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grips = hub.panes.get::<Grips>(None);
    Ok(grips.and_then(|grips| grips.of(root::MAIN).focus))
}

/// The record's table of places (layout.md 4.3), read off the panel: chat, work beside it,
/// the side column, the shelf. A kind with nothing to show is nowhere in it.
#[test]
fn the_panel_lists_the_sessions_panes_under_the_place_each_is_in() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    let list = by_place(&runtime);
    let places: Vec<&str> = list.iter().map(|(place, _)| place.as_str()).collect();
    assert_eq!(places, ["chat", "work", "side", "shelf", "closed"]);
    assert_eq!(under(&list, "chat"), ["Chat, in front"]);
    let mut work = under(&list, "work");
    work.sort();
    assert_eq!(
        work,
        [
            "Changes, in front",
            "Code, in front",
            "Remote desktop, in front",
            "Report, in front",
            "Web app, in front",
        ]
    );
    // The session has child agents: Agents came by itself as a tab behind Files, not seen.
    assert_eq!(
        under(&list, "side"),
        ["Files, in front", "Agents, a tab behind, not seen yet"]
    );
    assert_eq!(under(&list, "shelf"), ["Terminal, in front"]);
    let closed = under(&list, "closed");
    for kind in ["Trace", "Logs", "Findings"] {
        let said = format!("{kind}, not open");
        assert!(closed.contains(&said), "{kind} can be opened: {closed:?}");
    }
    // No cell of this session drew a picture, Settings opens in a window, chat is listed.
    for kind in ["IPython", "Settings", "Themes", "Chat", "Files"] {
        let said = format!("{kind}, not open");
        assert!(!closed.contains(&said), "{kind} is not offered: {closed:?}");
    }
    Ok(())
}

/// A row is the way to its pane: a tab behind comes in front, the pane takes the keys, and
/// the panel gets out of the way. The row of a pane already drawn only moves the keys.
#[test]
fn a_press_on_a_row_goes_to_its_pane_and_closes_the_panel() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    let agents = row_of(&runtime, "Agents")?;
    tap(&mut runtime, &agents)?;
    assert!(!live(&runtime, PANEL), "the panel closed");
    let drawn = kinds(&runtime)?;
    assert!(drawn.contains(&PaneKind::Agents), "Agents is drawn");
    assert!(!drawn.contains(&PaneKind::Files), "in the slot Files had");
    assert_eq!(focus(&runtime)?, Some(pane_of(&agents)?), "with the keys");
    open(&mut runtime);
    assert_eq!(
        under(&by_place(&runtime), "side"),
        ["Files, a tab behind", "Agents, in front"],
        "in tab order, and seen now"
    );
    let terminal = row_of(&runtime, "Terminal")?;
    tap(&mut runtime, &terminal)?;
    assert!(!live(&runtime, PANEL));
    assert_eq!(
        kinds(&runtime)?,
        drawn,
        "a pane already drawn moves nothing"
    );
    assert_eq!(focus(&runtime)?, Some(pane_of(&terminal)?));
    Ok(())
}

/// A kind not open opens where its Show command puts it; a row's cross closes its pane as
/// the bar's does, with no hole, and leaves the panel open; the chat has no cross.
#[test]
fn a_kind_not_open_opens_as_show_does_and_a_cross_closes_with_no_hole() -> Check {
    let mut panel = settled("shell/mockup")?;
    let mut shown = settled("shell/mockup")?;
    shown.update(|win, tx| view::run(win, tx, CommandId("pane.show-trace"), Target::App));
    rest(&mut shown);
    open(&mut panel);
    tap(&mut panel, "grid.panel.kind.trace")?;
    assert!(!live(&panel, PANEL), "the panel closed");
    let grid = |runtime: &Runtime<Win>| -> Result<_, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        Ok(hub.model.grid.window(root::MAIN).map(|w| w.layout.clone()))
    };
    assert_eq!(grid(&panel)?, grid(&shown)?, "where Show Trace puts it");
    open(&mut panel);
    let list = by_place(&panel);
    assert_eq!(
        under(&list, "shelf"),
        ["Terminal, a tab behind", "Trace, in front"]
    );
    assert!(
        !under(&list, "closed").contains(&"Trace, not open".to_owned()),
        "and it is no longer offered"
    );
    let trace = row_of(&panel, "Trace")?;
    tap(&mut panel, &format!("{trace}.close"))?;
    assert!(live(&panel, PANEL), "a cross leaves the panel open");
    let list = by_place(&panel);
    assert_eq!(under(&list, "shelf"), ["Terminal, in front"]);
    assert!(
        under(&list, "closed").contains(&"Trace, not open".to_owned()),
        "Trace can be opened again"
    );
    let files = row_of(&panel, "Files")?;
    tap(&mut panel, &format!("{files}.close"))?;
    let side = under(&by_place(&panel), "side");
    assert_eq!(side, ["Agents, in front"], "its next tab shows");
    let agents = row_of(&panel, "Agents")?;
    tap(&mut panel, &format!("{agents}.close"))?;
    let list = by_place(&panel);
    let places: Vec<&str> = list.iter().map(|(place, _)| place.as_str()).collect();
    assert_eq!(
        places,
        ["chat", "work", "shelf", "closed"],
        "a place with no pane has no heading"
    );
    let hub = panel.state.hub.try_borrow()?;
    let size = Size {
        w: 1000.0,
        h: 700.0,
    };
    let placed = hub.model.grid.place(root::MAIN, size);
    let area: f32 = placed.iter().map(|(_, r)| r.w * r.h).sum();
    assert!((area - 700_000.0).abs() < 1.0, "no hole: {area}");
    drop(hub);
    let chat = row_of(&panel, "Chat")?;
    assert!(!live(&panel, &format!("{chat}.close")), "chat cannot close");
    Ok(())
}

/// A pane about one file is listed by the file's name, as its tab is, beside the viewer.
#[test]
fn a_pane_about_a_file_is_listed_by_the_files_name() -> Check {
    let mut runtime = settled("shell/everything")?;
    let thing = Address::new(Kind::File, "src/main.rs")?;
    runtime.update(move |win, _| {
        let (window, kind) = (root::MAIN, PaneKind::Code);
        let about = GridMsg::About {
            window,
            kind,
            thing,
        };
        dispatch(win, Msg::Grid(about));
    });
    rest(&mut runtime);
    open(&mut runtime);
    let work = under(&by_place(&runtime), "work");
    let named = |name: &str| work.iter().any(|row| row.starts_with(&format!("{name}, ")));
    assert!(named("main.rs"), "by its file: {work:?}");
    assert!(named("Code"), "and the viewer by its kind: {work:?}");
    Ok(())
}

/// A pane pulled out into a window of its own stays in the list of the window it left, and
/// a press on its row puts it back.
#[test]
fn a_pane_out_in_a_window_of_its_own_is_listed_and_a_press_puts_it_back() -> Check {
    let mut runtime = settled("shell/mockup")?;
    let shown = runtime.elements();
    let out = keys(&shown)
        .into_iter()
        .find(|key| key.starts_with("terminal-") && key.ends_with(".pane-out"));
    let out = out.ok_or("no pull-out icon on Terminal")?.to_owned();
    tap(&mut runtime, &out)?;
    assert!(!kinds(&runtime)?.contains(&PaneKind::Terminal), "it left");
    open(&mut runtime);
    assert_eq!(
        under(&by_place(&runtime), "shelf"),
        ["Terminal, in a window of its own"]
    );
    let row = row_of(&runtime, "Terminal")?;
    tap(&mut runtime, &row)?;
    assert!(kinds(&runtime)?.contains(&PaneKind::Terminal), "it is back");
    assert!(!live(&runtime, PANEL));
    Ok(())
}

/// The keyboard: the panel takes it when opened, the arrows move from item to item, Space
/// and Return press the one they are on.
#[test]
fn arrows_move_through_the_panel_and_space_or_return_presses() -> Check {
    let mut runtime = settled("shell/mockup")?;
    open(&mut runtime);
    let focused = |runtime: &Runtime<Win>| {
        let shown = runtime.elements();
        let found = shown.iter().find(|e| e.focused && !e.exiting);
        found.and_then(|e| e.key.clone()).unwrap_or_default()
    };
    let first = focused(&runtime);
    assert!(
        first.starts_with("grid.panel."),
        "the panel has the keyboard: {first:?}"
    );
    strike(&mut runtime, Key::Right, Mods::default());
    let next = focused(&runtime);
    assert!(
        next.starts_with("grid.panel.") && next != first,
        "{first} then {next}"
    );
    strike(&mut runtime, Key::Up, Mods::default());
    assert_eq!(focused(&runtime), first, "Up goes back");
    let walk = |runtime: &mut Runtime<Win>, to: &str| {
        let mut seen = vec![focused(runtime)];
        for _ in 0..80 {
            if focused(runtime) == to {
                break;
            }
            strike(runtime, Key::Down, Mods::default());
            seen.push(focused(runtime));
        }
        seen
    };
    let files = row_of(&runtime, "Files")?;
    let seen = walk(&mut runtime, "grid.panel.kind.trace");
    assert_eq!(focused(&runtime), "grid.panel.kind.trace", "after {seen:?}");
    for key in ["grid.panel.save", "grid.panel.side.1", files.as_str()] {
        assert!(
            seen.iter().any(|k| k == key),
            "{key} is passed on the way: {seen:?}"
        );
    }
    strike(&mut runtime, Key::Text(" ".into()), Mods::default());
    assert!(
        kinds(&runtime)?.contains(&PaneKind::Trace),
        "Space opens Trace"
    );
    open(&mut runtime);
    let agents = row_of(&runtime, "Agents")?;
    let seen = walk(&mut runtime, &agents);
    assert_eq!(focused(&runtime), agents, "after {seen:?}");
    strike(&mut runtime, Key::Enter, Mods::default());
    assert!(
        kinds(&runtime)?.contains(&PaneKind::Agents),
        "Return goes to Agents"
    );
    Ok(())
}

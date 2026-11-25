//! The command box, the run path and the shortcut map as a person meets them.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el};
use yd_core::command::table::{OPEN_COMMANDS, TOGGLE_SIDEBAR};
use yd_core::feed::{FILES, FeedEvent};
use yd_core::grid::layout::ChatSide as Side;
use yd_core::grid::step::GridMsg;
use yd_core::ids::{CommandId, WindowId};
use yd_core::msg::{Effect, Msg, step};
use yd_core::row::rows_from_json;
use yd_core::wire::replay::play;

use super::*;
use crate::win::Hub;

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;
type Asked = Rc<RefCell<Vec<Effect>>>;

const WINDOW: WindowId = WindowId(0);
const DT: f64 = 1.0 / 120.0;

/// Threads from a recorded server and the files tree the Files pane's mockup scene shows.
fn sample(hub: &mut Hub) {
    let _ = play(
        &mut hub.model,
        include_str!("../../../../../fixtures/wire/lists.jsonl"),
    );
    let tree = include_str!("../../../../panes/src/files/mockup.tree.json");
    let rows = rows_from_json(tree).unwrap_or_default();
    step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Reset(rows)));
    let show = GridMsg::Show {
        window: WINDOW,
        saved: "everything",
    };
    step(&mut hub.model, Msg::Grid(show));
}

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let opener = el().key("opener").size(40.0, 20.0).focusable(true);
    rune_kit::root(
        el().key("root.base").w_full().h_full().child(opener),
        layer(win, &hub, cx),
    )
}

fn window() -> Result<(Window, Asked), Box<dyn Error>> {
    let hub = Hub::new(&[], Vec::new());
    let asked: Asked = Rc::new(RefCell::new(Vec::new()));
    {
        let mut hub = hub.try_borrow_mut()?;
        sample(&mut hub);
        let log = Rc::clone(&asked);
        hub.set_sink(move |effect| log.borrow_mut().push(effect));
    }
    let mut window = Runtime::new(
        Win::new(hub, WINDOW),
        root,
        (1200.0, 750.0),
        Env::new(true, 1.0),
    );
    window.on_key = Some(keys);
    rest(&mut window);
    Ok((window, asked))
}

fn rest(window: &mut Window) {
    for _ in 0..120 {
        if !window.frame(DT) {
            break;
        }
    }
}

fn press(window: &mut Window, key: Key, mods: Mods) {
    window.key(KeyEvent { key, mods });
    rest(window);
}

fn cmd() -> Mods {
    Mods {
        cmd: cfg!(target_os = "macos"),
        logo: true,
        ..Mods::default()
    }
}

fn shown(window: &Window, key: &str) -> bool {
    let tree = window.elements();
    tree.iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn boxed(window: &Window) -> Result<Option<Boxed>, Box<dyn Error>> {
    let hub = window.state.hub.try_borrow()?;
    Ok(opened(&hub, WINDOW).cloned())
}

fn type_in(window: &mut Window, query: &str) -> Check {
    let query = query.to_owned();
    window.update(move |win: &mut Win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            open_in(&mut hub, WINDOW, &query);
        }
    });
    rest(window);
    Ok(())
}

#[test]
fn cmd_k_at_the_window_root_opens_the_box_and_esc_closes_it() -> Check {
    let (mut window, _) = window()?;
    assert!(!shown(&window, KEY));
    press(&mut window, Key::Text("k".into()), cmd());
    assert!(shown(&window, KEY), "cmd+k opens the box");
    assert!(shown(&window, FIELD));
    press(&mut window, Key::Escape, Mods::default());
    assert!(boxed(&window)?.is_none(), "esc closes it");
    assert!(!shown(&window, KEY));
    Ok(())
}

#[test]
fn the_box_finds_commands_threads_and_files_by_the_words_typed() -> Check {
    let (window, _) = window()?;
    let hub = window.state.hub.try_borrow()?;
    let layouts = search(&hub, "layout");
    let panel = Found::Command(
        CommandId("layout.pick"),
        "Panes and Layouts".into(),
        Some("alt+cmd+0".to_owned()),
    );
    assert!(layouts.contains(&panel), "{layouts:?}");
    let fork = search(&hub, "FORK");
    let thread = fork
        .iter()
        .any(|found| matches!(found, Found::Thread(_, t) if t == "something to fork"));
    assert!(thread, "a thread by its title, in any case: {fork:?}");
    let file = search(&hub, "rate limit");
    let path = Found::File("src/middleware/rate_limit.rs".to_owned());
    assert!(
        file.contains(&path),
        "a file by the words of its path: {file:?}"
    );
    assert!(search(&hub, "zzz nothing").is_empty());
    Ok(())
}

#[test]
fn return_runs_the_top_row_and_closes_the_box() -> Check {
    let (mut window, _) = window()?;
    type_in(&mut window, "other side")?;
    let side = |window: &Window| window.state.hub.try_borrow().map(|h| h.model.grid.side);
    assert_eq!(side(&window)?, Side::Left);
    window.update(|_, tx| tx.focus(FIELD, false));
    rest(&mut window);
    press(&mut window, Key::Enter, Mods::default());
    assert_eq!(side(&window)?, Side::Right, "the chat moved");
    assert!(boxed(&window)?.is_none());
    Ok(())
}

#[test]
fn a_command_that_is_not_built_says_so_and_runs_nothing() -> Check {
    let (mut window, asked) = window()?;
    type_in(&mut window, "notifications")?;
    window.update(|_, tx| tx.focus(FIELD, false));
    rest(&mut window);
    press(&mut window, Key::Enter, Mods::default());
    let said = boxed(&window)?.and_then(|b| b.said);
    assert_eq!(said.as_deref(), Some("Show Notifications is not built yet"));
    assert!(shown(&window, &format!("{KEY}.said")));
    assert!(
        asked.borrow().is_empty(),
        "the field kept Return: no approval was answered"
    );
    Ok(())
}

#[test]
fn the_menu_bar_greys_a_window_command_until_the_window_carries_it() -> Check {
    fn fold(_: &mut Win, _: &mut Tx, id: CommandId, _: &Target) -> bool {
        id == TOGGLE_SIDEBAR
    }
    let hub = Hub::new(&[], Vec::new());
    let sidebar = |hub: &Hub| {
        let menus = menu_bar(hub);
        let view = menus.into_iter().find(|menu| menu.title == "View");
        let items = view.map(|menu| menu.items).unwrap_or_default();
        items
            .into_iter()
            .find(|item| item.command == TOGGLE_SIDEBAR)
    };
    let before = sidebar(&*hub.try_borrow()?).ok_or("no sidebar item")?;
    assert_eq!((before.key, before.enabled), (Some("alt+cmd+s"), false));
    install(&mut *hub.try_borrow_mut()?, &[TOGGLE_SIDEBAR], fold);
    let after = sidebar(&*hub.try_borrow()?).ok_or("no sidebar item")?;
    assert!(after.enabled);
    let layouts = menu_bar(&*hub.try_borrow()?)
        .into_iter()
        .find(|m| m.title == "View");
    let everything = layouts
        .ok_or("no View")?
        .items
        .into_iter()
        .find(|i| i.title == "Everything");
    assert!(
        everything.is_some_and(|item| item.enabled),
        "the grid carries layouts"
    );
    Ok(())
}

#[test]
fn alt_cmd_s_reaches_the_command_the_window_carries() -> Check {
    fn fold(win: &mut Win, _: &mut Tx, id: CommandId, _: &Target) -> bool {
        if id == TOGGLE_SIDEBAR {
            change(win, |b| b.said = Some("folded".to_owned()));
            open_in_place(win);
        }
        id == TOGGLE_SIDEBAR
    }
    fn open_in_place(win: &mut Win) {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            open_in(&mut hub, WINDOW, "folded");
        }
    }
    let (mut window, asked) = window()?;
    install(
        &mut *window.state.hub.try_borrow_mut()?,
        &[TOGGLE_SIDEBAR],
        fold,
    );
    let held = Mods { alt: true, ..cmd() };
    press(&mut window, Key::Text("s".into()), held);
    assert_eq!(boxed(&window)?.map(|b| b.query).as_deref(), Some("folded"));
    assert!(
        asked.borrow().is_empty(),
        "the window carried it: nothing went to the model"
    );
    Ok(())
}

fn focused(window: &Window) -> Option<String> {
    let tree = window.elements();
    let found = tree.iter().find(|e| e.focused && !e.exiting);
    found.and_then(|e| e.key.clone())
}

#[test]
fn cmd_slash_lists_the_whole_map_with_each_key_as_the_map_has_it() -> Check {
    let (mut window, _) = window()?;
    press(&mut window, Key::Text("/".into()), cmd());
    let sheet = boxed(&window)?.ok_or("the sheet did not open")?;
    assert!(sheet.keys, "it lists shortcuts, not a search");
    let hub = window.state.hub.try_borrow()?;
    let map = keymap(&hub);
    let listed = super::sheet(&hub, "");
    // A kept layout's number holds its key with no layout under it: the sheet leaves it out.
    let unkept = |row: &&yd_core::command::keys::Row| {
        crate::grid_view::picker::own_of(row.command).is_some()
    };
    let keyed = map
        .rows()
        .iter()
        .filter(|row| row.key.is_some() && !unkept(row));
    assert_eq!(
        listed.len(),
        keyed.count(),
        "every command with a key, once"
    );
    let texts: Vec<String> = window
        .elements()
        .into_iter()
        .filter_map(|e| e.text)
        .collect();
    for (found, group) in &listed {
        let Found::Command(id, title, _) = found else {
            return Err("the sheet lists only commands".into());
        };
        assert!(texts.iter().any(|t| t == title), "{title} is drawn");
        let hint = map.hint(*id).ok_or("a listed command has no key")?;
        assert!(texts.contains(&hint), "{title}: {hint} is drawn: {texts:?}");
        assert!(
            texts.iter().any(|t| t == group),
            "{title}: its menu {group}"
        );
    }
    Ok(())
}

#[test]
fn the_sheet_filters_by_title_menu_or_key_and_esc_gives_the_keyboard_back() -> Check {
    let (mut window, _) = window()?;
    window.update(|_, tx| tx.focus("opener", true));
    rest(&mut window);
    assert_eq!(focused(&window).as_deref(), Some("opener"));
    press(&mut window, Key::Text("/".into()), cmd());
    assert_eq!(focused(&window).as_deref(), Some(FIELD));
    let hub = window.state.hub.try_borrow()?;
    let titles = |query: &str| -> Vec<String> {
        super::sheet(&hub, query)
            .into_iter()
            .filter_map(|(found, _)| match found {
                Found::Command(_, title, _) => Some(title.into_owned()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(titles("sidebar"), ["Toggle Sidebar"]);
    assert_eq!(titles("⌥⌘S"), ["Toggle Sidebar"], "by the key as drawn");
    assert!(
        titles("help")
            .iter()
            .any(|title| title == "Keyboard Shortcuts"),
        "by its menu"
    );
    drop(hub);
    press(&mut window, Key::Escape, Mods::default());
    assert!(boxed(&window)?.is_none());
    assert_eq!(
        focused(&window).as_deref(),
        Some("opener"),
        "closing hands the keyboard back to what opened the box"
    );
    Ok(())
}

#[test]
fn a_users_key_runs_the_command_and_every_hint_follows_it() -> Check {
    use yd_core::ids::SettingKey;
    use yd_core::settings::schema::SettingValue;
    use yd_core::settings::store::SettingsMsg;
    let (mut window, _) = window()?;
    {
        let mut hub = window.state.hub.try_borrow_mut()?;
        let own = yd_core::command::keys::settings(&table(&hub));
        step(&mut hub.model, Msg::Settings(SettingsMsg::Declare(own)));
        let key = SettingKey::new("keys.command.open");
        let value = Some(SettingValue::Text("cmd+shift+j".into()));
        step(&mut hub.model, Msg::Settings(SettingsMsg::Set(key, value)));
    }
    press(&mut window, Key::Text("k".into()), cmd());
    assert!(boxed(&window)?.is_none(), "the old key runs nothing now");
    let shifted = Mods {
        shift: true,
        ..cmd()
    };
    press(&mut window, Key::Text("j".into()), shifted);
    assert!(boxed(&window)?.is_some(), "the user's key opens the box");
    let hub = window.state.hub.try_borrow()?;
    let found = search(&hub, "search or run");
    let keyed = found
        .iter()
        .find_map(|f| match f {
            Found::Command(id, _, key) if *id == OPEN_COMMANDS => key.clone(),
            _ => None,
        })
        .ok_or("the box does not list itself")?;
    assert_eq!(keyed, "cmd+shift+j", "the row's key hint is the map's");
    Ok(())
}

/// Both surfaces of the box, the field's and the rows', and the help sheet that shares them.
fn surfaces(window: &Window) -> Vec<(String, f32)> {
    let tree = window.elements();
    let named = |key: &str| tree.iter().find(|e| e.key.as_deref() == Some(key));
    let keys = [KEY, "window.command-box.list-content"];
    keys.iter()
        .filter_map(|key| named(key).map(|e| (key.to_string(), e.fill.a)))
        .collect()
}

#[test]
fn the_command_box_and_the_help_sheet_are_opaque_so_no_page_text_shows_through() -> Check {
    for chord in ["k", "/"] {
        let (mut window, _) = window()?;
        press(&mut window, Key::Text(chord.into()), cmd());
        let found = surfaces(&window);
        assert_eq!(found.len(), 2, "{chord}: surfaces drawn: {found:?}");
        for (key, alpha) in found {
            assert!(alpha >= 0.999, "{chord}: {key} is see-through at {alpha}");
        }
    }
    Ok(())
}

//! The whole keyboard: every control has a name, and the app can be used with no pointer.

mod support;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime};
use support::scene;
use yd_core::address::uri::Address;
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{FILES, OPEN_FILES, SourceCmd};
use yd_core::ids::{CommandId, SettingKey, ThreadId};
use yd_core::kind::Kind;
use yd_core::msg::{Effect, Msg, step};
use yd_core::row::Rows;
use yd_core::settings::schema::SettingValue;
use yd_core::settings::store::SettingsMsg;
use yd_core::wire::event::WireEvent;
use yd_ui::kit::key_hint::KEYCAP;
use yd_ui::win::{Win, dispatch};
use yi_desktop::{root, scenes};

type Check = Result<(), Box<dyn Error>>;

const PRESSABLE: [&str; 9] = [
    "button", "link", "tab", "checkbox", "switch", "radio", "combobox", "textbox", "slider",
];

/// What a screen reader says for element `at`: its own label, else the words under it.
fn named(tree: &[ElementInfo], at: usize) -> bool {
    let mut below = vec![at];
    for (index, element) in tree.iter().enumerate() {
        let mut up = element.parent;
        while let Some(parent) = up {
            if parent == at {
                below.push(index);
                break;
            }
            up = tree.get(parent).and_then(|above| above.parent);
        }
    }
    let words = |index: &usize| {
        let element = tree.get(*index);
        let said = |text: &Option<String>| text.as_deref().is_some_and(|t| !t.trim().is_empty());
        let spoken = |e: &ElementInfo| said(&e.label) || said(&e.text) || said(&e.help);
        element.is_some_and(|e| !e.exiting && spoken(e))
    };
    below.iter().any(words)
}

#[test]
fn every_pressable_or_typed_into_element_of_every_scene_has_a_name() -> Check {
    let mut nameless = Vec::new();
    for def in scenes::all() {
        let tree = scene::elements(def.name)?;
        for (at, element) in tree.iter().enumerate() {
            let pressable = PRESSABLE.contains(&element.role) && element.focusable;
            if pressable && !element.exiting && !named(&tree, at) {
                let key = element.key.as_deref().unwrap_or(element.path.as_str());
                nameless.push(format!("{}: {} ({})", def.name, key, element.role));
            }
        }
    }
    nameless.sort();
    nameless.dedup();
    assert!(
        nameless.is_empty(),
        "{} unnamed:\n{}",
        nameless.len(),
        nameless.join("\n")
    );
    Ok(())
}

fn key(runtime: &mut Runtime<Win>, key: Key, mods: Mods) {
    runtime.key(KeyEvent { key, mods });
    for _ in 0..30 {
        if !runtime.frame(1.0 / 60.0) {
            break;
        }
    }
}

fn focused(runtime: &Runtime<Win>) -> Option<String> {
    let tree = runtime.elements();
    let found = tree.iter().find(|e| e.focused && !e.exiting);
    found.and_then(|e| e.key.clone())
}

/// The window as the app runs it: the poll and the key handler installed.
fn window(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut runtime = scene::runtime(name, scene::WINDOW)?;
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    Ok(runtime)
}

#[test]
fn arrows_move_between_the_tabs_of_a_strip_and_bring_each_in_front() -> Check {
    let mut runtime = window("shell/everything")?;
    let tree = runtime.elements();
    let tabs: Vec<String> = tree
        .iter()
        .filter_map(|e| e.key.clone())
        .filter(|key| key.starts_with("terminal-") && key.ends_with(".pane-tab"))
        .collect();
    let (first, second) = (tabs.first().ok_or("no tab")?, tabs.get(1).ok_or("one tab")?);
    let (first, second) = (first.clone(), second.clone());
    let fill = |runtime: &Runtime<Win>, key: &str| {
        let tree = runtime.elements();
        let found = tree.into_iter().find(|e| e.key.as_deref() == Some(key));
        found.map(|e| e.fill)
    };
    runtime.update(|_, tx| tx.focus(first.clone(), true));
    let before = fill(&runtime, &second);
    key(&mut runtime, Key::Right, Mods::default());
    assert_ne!(
        fill(&runtime, &second),
        before,
        "the tab it reached is the one in front"
    );
    assert_eq!(
        focused(&runtime),
        Some(second.clone()),
        "Right goes to the next tab"
    );
    key(&mut runtime, Key::Left, Mods::default());
    assert_eq!(focused(&runtime), Some(first), "Left comes back");
    Ok(())
}

type Asked = Rc<RefCell<Vec<Effect>>>;

fn asked(runtime: &Runtime<Win>) -> Result<Asked, Box<dyn Error>> {
    let log = Asked::default();
    let kept = Rc::clone(&log);
    runtime
        .state
        .hub
        .try_borrow_mut()?
        .set_sink(move |effect| kept.borrow_mut().push(effect));
    Ok(log)
}

fn typed(runtime: &mut Runtime<Win>, words: &str) {
    key(runtime, Key::Text(words.to_owned()), Mods::default());
}

fn live(runtime: &Runtime<Win>, key: &str) -> bool {
    let tree = runtime.elements();
    tree.iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// Tab until focus is on a key that `wanted` accepts: the keys it passed on the way.
fn tab_to(
    runtime: &mut Runtime<Win>,
    shift: bool,
    wanted: impl Fn(&str) -> bool,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mods = Mods {
        shift,
        ..Mods::default()
    };
    let mut passed = Vec::new();
    for _ in 0..200 {
        key(runtime, Key::Tab, mods);
        let here = focused(runtime).unwrap_or_default();
        passed.push(here.clone());
        if wanted(&here) {
            return Ok(passed);
        }
    }
    Err(format!("Tab never reached the place; it passed {passed:?}").into())
}

#[test]
fn with_only_the_keyboard_a_chat_is_started_sent_and_a_file_opened() -> Check {
    let mut runtime = window("shell/everything")?;
    let log = asked(&runtime)?;
    // Cmd+K, the words, Return: New Chat.
    key(
        &mut runtime,
        Key::Text("k".into()),
        Mods {
            cmd: true,
            ..Mods::default()
        },
    );
    assert!(live(&runtime, "window.command-box"), "cmd+k opens the box");
    typed(&mut runtime, "new chat");
    key(&mut runtime, Key::Enter, Mods::default());
    assert!(
        live(&runtime, "new-chat.chat-column"),
        "the page for a new chat shows"
    );
    // The words go to the composer the page gave the keyboard, and Return sends them.
    assert_eq!(
        focused(&runtime).as_deref(),
        Some("new-chat.composer.input")
    );
    typed(&mut runtime, "fix the flake");
    key(&mut runtime, Key::Enter, Mods::default());
    let lines: Vec<String> = log
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Effect::Wire(request) => Some(request.line.clone()),
            _ => None,
        })
        .collect();
    let made = lines
        .iter()
        .find(|line| line.contains("session/new"))
        .ok_or("no session/new")?;
    let id = made
        .split("\"id\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .ok_or("no id")?;
    let reply = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"fresh-thread"}}}}"#);
    runtime.update(move |win, _| {
        let line = WireEvent::Line(reply);
        dispatch(win, Msg::Wire(line));
    });
    key(&mut runtime, Key::Escape, Mods::default());
    assert!(
        !live(&runtime, "new-chat.chat-column"),
        "the thread shows now"
    );
    let said = |log: &Asked| {
        let sent = log.borrow();
        let words = sent.iter().filter_map(|effect| match effect {
            Effect::Wire(request) => Some(request.line.clone()),
            _ => None,
        });
        words.filter(|line| line.contains("fix the flake")).count()
    };
    assert!(said(&log) >= 1, "the first prompt carried the words");
    // A new thread opens blank: Everything comes back by its key.
    let layout = Mods {
        cmd: true,
        alt: true,
        ..Mods::default()
    };
    key(&mut runtime, Key::Text("1".into()), layout);
    // Tab on to the Files pane, a row of it, and open it.
    let passed = tab_to(&mut runtime, false, |key| {
        key.starts_with("files-") && key.contains(".row.")
    })?;
    let row = passed.last().cloned().unwrap_or_default();
    key(&mut runtime, Key::Enter, Mods::default());
    let path = row
        .split_once(".row.")
        .map(|(_, id)| id.trim_start_matches("change:").to_owned());
    let path = path.ok_or("no path in the row's key")?;
    let read = log.borrow().iter().any(|effect| {
        matches!(effect, Effect::Source(feed, SourceCmd::Read(file)) if *feed == FILES && file.ends_with(&path))
    });
    assert!(read, "Return opened {path}: {:?}", log.borrow().len());
    Ok(())
}

/// The words drawn inside the element keyed `key`, left to right.
fn words(runtime: &Runtime<Win>, key: &str) -> Vec<String> {
    let tree = runtime.elements();
    let Some(shell) = tree.iter().find(|e| e.key.as_deref() == Some(key)) else {
        return Vec::new();
    };
    let [x, y, w, h] = shell.rect;
    let mut inside: Vec<&ElementInfo> = tree
        .iter()
        .filter(|e| !e.exiting && e.text.is_some())
        .filter(|e| e.rect[0] >= x && e.rect[1] >= y && e.rect[0] < x + w && e.rect[1] < y + h)
        .collect();
    inside.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    inside.into_iter().filter_map(|e| e.text.clone()).collect()
}

fn own_keys(runtime: &Runtime<Win>, keys: &[(&str, &str)]) -> Check {
    let mut hub = runtime.state.hub.try_borrow_mut()?;
    let rows = yd_core::command::keys::settings(&yd_ui::menu::command_box::table(&hub));
    step(&mut hub.model, Msg::Settings(SettingsMsg::Declare(rows)));
    for (id, chord) in keys {
        let text = SettingValue::Text((*chord).to_owned().into());
        let set = SettingsMsg::Set(SettingKey::new(format!("keys.{id}")), Some(text));
        step(&mut hub.model, Msg::Settings(set));
    }
    Ok(())
}

#[test]
fn every_key_hint_the_window_draws_is_the_maps_even_after_the_user_changes_a_key() -> Check {
    let mut runtime = window("shell/everything")?;
    own_keys(
        &runtime,
        &[
            ("command.open", "cmd+shift+j"),
            ("layout.review", "ctrl+alt+cmd+2"),
        ],
    )?;
    runtime.update(|win, tx| {
        let run = Run::new(yd_ui::grid_view::picker::PICK);
        yd_ui::menu::command_box::run(win, tx, run);
    });
    for _ in 0..10 {
        runtime.frame(1.0 / 60.0);
    }
    let hint = |id: &str| {
        let hub = runtime.state.hub.try_borrow().ok()?;
        yd_ui::menu::command_box::keymap(&hub).hint(CommandId(id.to_owned().leak()))
    };
    assert_eq!(
        hint("command.open").as_deref(),
        Some("⌘⇧J"),
        "the map follows the user"
    );
    let top = words(&runtime, "shell.command-box");
    assert_eq!(
        top.last(),
        hint("command.open").as_ref(),
        "the top bar's box: {top:?}"
    );
    for (id, slug) in [
        ("layout.everything", "everything"),
        ("layout.review", "review"),
        ("layout.just-chat", "just-chat"),
    ] {
        let shown = runtime.elements();
        let key = format!("grid.panel.layout.{slug}");
        let picture = shown
            .iter()
            .find(|e| e.key.as_deref() == Some(key.as_str()));
        let said = picture.and_then(|e| e.help.clone()).unwrap_or_default();
        let hint = hint(id).ok_or("no hint")?;
        assert!(said.ends_with(&hint), "{id} in the Panes panel: {said:?}");
    }
    let mut settings = window("settings/mockup")?;
    own_keys(&settings, &[("pane.show-settings", "cmd+shift+,")])?;
    settings.frame(1.0 / 60.0);
    let search = words(&settings, "settings-0.search");
    let want = hint("pane.show-settings");
    assert_eq!(
        search.last(),
        want.as_ref(),
        "Settings' search box: {search:?}"
    );
    Ok(())
}

#[test]
fn the_plan_keys_each_run_one_command_and_no_key_of_any_pane_is_bound_twice() -> Check {
    let runtime = window("shell/everything")?;
    let hub = runtime.state.hub.try_borrow()?;
    assert!(
        !hub.defs.is_empty(),
        "the window's own panes are in the table"
    );
    let keys = yd_ui::menu::command_box::keymap(&hub);
    assert_eq!(keys.problems, []);
    let plan = [
        ("cmd+k", "command.open"),
        ("alt+cmd+s", "sidebar.toggle"),
        ("alt+cmd+1", "layout.everything"),
        ("alt+cmd+5", "layout.just-chat"),
        ("cmd+w", "tab.close"),
        ("cmd+shift+[", "tab.previous"),
        ("cmd+shift+]", "tab.next"),
        ("cmd+,", "pane.show-settings"),
        ("return", "approval.allow"),
        ("cmd+return", "approval.always"),
        ("escape", "approval.deny"),
    ];
    for (key, command) in plan {
        let found = keys.command(key).map(|id| id.0);
        assert_eq!(found, Some(command), "{key}");
    }
    Ok(())
}

#[test]
fn every_keycap_any_scene_draws_is_a_key_the_map_holds() -> Check {
    let mut wrong = Vec::new();
    for def in scenes::all() {
        let runtime = window(def.name)?;
        let map = yd_ui::menu::command_box::keymap(&*runtime.state.hub.try_borrow()?);
        let hints: Vec<String> = map
            .rows()
            .iter()
            .filter_map(|r| map.hint(r.command))
            .collect();
        let tree = runtime.elements();
        for (at, element) in tree.iter().enumerate() {
            if element.role != KEYCAP || element.exiting {
                continue;
            }
            let child = |e: &&ElementInfo| e.parent == Some(at) && e.text.is_some();
            let Some(cap) = tree.iter().find(child).and_then(|e| e.text.clone()) else {
                continue;
            };
            let fits = hints.contains(&cap);
            if !fits {
                let key = element.key.as_deref().unwrap_or(element.path.as_str());
                wrong.push(format!("{}: {cap} at {key}", def.name));
            }
        }
    }
    wrong.sort();
    wrong.dedup();
    assert!(
        wrong.is_empty(),
        "{} keycaps not in the map:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    Ok(())
}

fn tap(runtime: &mut Runtime<Win>, key: &str) -> Check {
    let tree = runtime.elements();
    let found = tree
        .iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting);
    let e = found.ok_or(format!("no {key}"))?;
    // Near the start: a shell's row is wider than its pane, and with chat on the left the
    // middle of it lies over the next pane.
    let (x, y) = (
        e.rect[0] + (e.rect[2] / 2.0).min(8.0),
        e.rect[1] + e.rect[3] / 2.0,
    );
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    for _ in 0..30 {
        if !runtime.frame(1.0 / 60.0) {
            break;
        }
    }
    Ok(())
}

fn terminal_tab(runtime: &Runtime<Win>, suffix: &str) -> Option<String> {
    let tree = runtime.elements();
    let keys = tree.iter().filter_map(|e| e.key.clone());
    keys.filter(|key| key.starts_with("terminal-"))
        .find(|key| key.ends_with(suffix))
}

fn shell_tabs(runtime: &Runtime<Win>) -> usize {
    runtime
        .elements()
        .iter()
        .filter(|e| {
            let key = e.key.as_deref().unwrap_or_default();
            !e.exiting && key.starts_with("terminal-") && key.ends_with(".pane-tab")
        })
        .count()
}

/// Dies with a window whose layout or sidebar a person changes by typing words after a click
/// on a button: the button held the keyboard and Space and Return pressed it again.
#[test]
fn typing_after_a_click_on_a_button_changes_nothing() -> Check {
    let mut runtime = window("shell/everything")?;
    tap(&mut runtime, "shell.sidebar-toggle")?;
    let folded = live(&runtime, "shell.sidebar");
    let shape = |runtime: &Runtime<Win>| -> Result<String, Box<dyn Error>> {
        Ok(format!("{:?}", runtime.state.hub.try_borrow()?.model.grid))
    };
    let before = shape(&runtime)?;
    assert_eq!(
        focused(&runtime),
        None,
        "a click leaves the keyboard on a button"
    );
    for words in [" ", "the quick brown fox 0123456789", " ", "jumps over"] {
        typed(&mut runtime, words);
        key(&mut runtime, Key::Enter, Mods::default());
    }
    assert_eq!(live(&runtime, "shell.sidebar"), folded, "the sidebar moved");
    assert_eq!(shape(&runtime)?, before, "the layout moved");
    Ok(())
}

/// Dies with a "+" that leaves the keyboard on itself: the words typed for the new shell are
/// lost and Return makes one more tab.
#[test]
fn a_new_shell_tab_has_the_keyboard() -> Check {
    let mut runtime = window("shell/everything")?;
    let tree = runtime.elements();
    let mut keys = tree.iter().filter_map(|e| e.key.clone());
    let first = keys
        .find(|k| k.starts_with("terminal-") && k.contains(".screen.") && !k.ends_with(".body"));
    let first = first.ok_or("no screen")?;
    tap(&mut runtime, &first)?;
    assert_eq!(
        focused(&runtime),
        Some(first),
        "the person's keys were in the first shell"
    );
    let plus = terminal_tab(&runtime, ".pane-tab-new").ok_or("no plus")?;
    tap(&mut runtime, &plus)?;
    let screen = focused(&runtime).ok_or("nothing has the keyboard")?;
    assert!(
        screen.starts_with("terminal-") && screen.contains(".screen."),
        "the keyboard is on {screen}"
    );
    key(&mut runtime, Key::Enter, Mods::default());
    assert_eq!(
        shell_tabs(&runtime),
        3,
        "two shells and the agent's, no more"
    );
    Ok(())
}

/// Dies with a second thread that shows the first one's file tabs, "Opening the file" for
/// ever, and shells whose screens went blank: the panes did not follow the thread.
#[test]
fn another_thread_has_its_own_file_tabs_and_shells() -> Check {
    let mut runtime = window("shell/everything")?;
    let log = asked(&runtime)?;
    let plus = terminal_tab(&runtime, ".pane-tab-new").ok_or("no plus")?;
    tap(&mut runtime, &plus)?;
    let other = {
        let hub = runtime.state.hub.try_borrow()?;
        let shown = yd_ui::shown::thread(&hub, runtime.state.window).map(|t| t.id.clone());
        let all = hub.model.wire.listed();
        let others = all.into_iter().map(|t| t.id.clone());
        let other: Option<ThreadId> = others.into_iter().find(|id| Some(id) != shown.as_ref());
        other.ok_or("the scene lists one thread")?
    };
    let file = Address::new(Kind::File, "src/lib.rs")?;
    runtime.update(move |win, _| {
        let run = Run::new(OPEN_FILE).on(Target::Thing(file));
        yd_ui::win::dispatch(win, Msg::Command(Route::Model, run));
    });
    let tabs = |runtime: &Runtime<Win>| -> Result<usize, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        Ok(hub.model.tables.get(OPEN_FILES).map_or(0, Rows::len))
    };
    assert!(tabs(&runtime)? > 0, "files are open before the switch");
    log.borrow_mut().clear();
    runtime.update(move |win, _| yd_ui::shown::show(win, other));
    for _ in 0..5 {
        runtime.frame(1.0 / 60.0);
    }
    assert_eq!(tabs(&runtime)?, 0, "the first thread's file tabs stayed");
    let hung_up = log
        .borrow()
        .iter()
        .filter(|e| matches!(e, Effect::Source(_, SourceCmd::Close)))
        .count();
    assert_eq!(
        hung_up, 0,
        "a shell was hung up by looking at another thread"
    );
    assert_eq!(shell_tabs(&runtime), 1, "the new thread has one shell");
    Ok(())
}

/// Dies with a dev server started in one thread's shell that is gone, and a fresh blank
/// shell in its place, when the person looks at another thread and comes back.
#[test]
fn a_shell_lives_on_while_another_thread_is_shown() -> Check {
    use yd_core::pane::PaneKind;
    use yd_ui::term::view::state::Shells;
    let mut runtime = window("shell/everything")?;
    for _ in 0..5 {
        runtime.frame(1.0 / 60.0);
    }
    let (pane, first) = {
        let hub = runtime.state.hub.try_borrow()?;
        let pane = hub
            .model
            .grid
            .panes
            .iter()
            .find(|p| p.kind == PaneKind::Terminal);
        let pane = pane.map(|p| p.id).ok_or("no terminal pane")?;
        let shells = hub.panes.get::<Shells>(Some(pane)).ok_or("no shells")?;
        let shell = shells.tabs.first().ok_or("no shell")?;
        if let Ok(mut parser) = shell.surface.parser.lock() {
            parser.process(b"Serving HTTP on :: port 8771");
        }
        (pane, shell.feed)
    };
    let (home, other) = {
        let hub = runtime.state.hub.try_borrow()?;
        let shown = yd_ui::shown::thread(&hub, runtime.state.window).map(|t| t.id.clone());
        let others = hub.model.wire.listed().into_iter().map(|t| t.id.clone());
        let other = others.into_iter().find(|id| Some(id) != shown.as_ref());
        (shown.ok_or("no thread shown")?, other.ok_or("one thread")?)
    };
    let log = asked(&runtime)?;
    log.borrow_mut().clear();
    for id in [other, home] {
        runtime.update(move |win, _| yd_ui::shown::show(win, id));
        for _ in 0..5 {
            runtime.frame(1.0 / 60.0);
        }
    }
    let hung_up = log
        .borrow()
        .iter()
        .any(|e| matches!(e, Effect::Source(_, SourceCmd::Close)));
    assert!(!hung_up, "a shell was hung up by a switch away and back");
    let hub = runtime.state.hub.try_borrow()?;
    let shells = hub.panes.get::<Shells>(Some(pane)).ok_or("no shells")?;
    let shell = shells
        .shell(first)
        .ok_or("the thread's own shell is gone")?;
    let screen = shell.surface.parser.lock().map(|p| p.screen().contents());
    assert!(
        screen.is_ok_and(|s| s.contains("Serving HTTP")),
        "the shell's output is gone"
    );
    Ok(())
}

#[test]
fn the_keyboard_help_says_which_always_cmd_return_picks() -> Check {
    let mut runtime = window("shell/everything")?;
    let cmd = Mods {
        cmd: true,
        ..Mods::default()
    };
    key(&mut runtime, Key::Text("/".into()), cmd);
    let tree = runtime.elements();
    let said = tree.iter().filter_map(|e| e.text.as_deref());
    let rule = said.into_iter().find(|t| t.starts_with("Always Allow"));
    assert_eq!(
        rule,
        Some("Always Allow the Narrowest Scope Yi Offers"),
        "the help sheet lists cmd+return's rule"
    );
    Ok(())
}

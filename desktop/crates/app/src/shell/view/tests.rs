//! The shell drawn from its scenes, read back from the element tree.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use yd_core::address::attach::ATTACH;
use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::msg::Effect;
use yd_core::wire::replay::play;
use yd_ui::pane::show_scene;
use yd_ui::win::Win;

use crate::shell::view;
use crate::{root, scenes};

mod attached;
mod bar;
mod beside;
mod children;
mod colour;
mod design;
mod docking;
mod dragged;
mod environment;
mod fill;
mod first_look;
mod ipython;
mod joined;
mod kept;
mod narrow;
mod opens;
mod outside;
mod panes;
mod peek;
mod places;
mod pointed;
mod project;
mod replay;
mod sidebar;
mod steps;
mod tabs;
mod tools;

type Check = Result<(), Box<dyn Error>>;

fn runtime(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut runtime = unframed(name)?;
    runtime.frame(1.0 / 60.0);
    Ok(runtime)
}

/// The root as the app runs it: its view, its poll and its key hook.
fn unframed(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut win = root::init();
    let index = scenes::all()
        .iter()
        .position(|scene| scene.name == name)
        .ok_or_else(|| format!("no scene {name:?}"))?;
    show_scene(&mut win, index);
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    Ok(runtime)
}

/// The person's words on what opens by itself, as the settings file would hold them. The
/// scenes' session has child agents, which would bring Agents beside a chat meant to be alone.
fn say(runtime: &mut Runtime<Win>, words: &'static str) {
    use yd_core::settings::schema::SettingValue;
    use yd_core::settings::store::{OPENS, SettingsMsg};
    runtime.update(move |win, _| {
        let key = yd_core::ids::SettingKey::fixed(OPENS);
        let set = SettingsMsg::Set(key, Some(SettingValue::Text(words.into())));
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Settings(set));
    });
}

fn rest(runtime: &mut Runtime<Win>) {
    for _ in 0..120 {
        if !runtime.frame(1.0 / 60.0) {
            break;
        }
    }
}

fn lines(recording: &str, from: usize, to: usize) -> String {
    let lines = recording.lines().skip(from).take(to.saturating_sub(from));
    lines.collect::<Vec<_>>().join("\n")
}

/// Plays more of the recording into the running window, as its reader thread would.
fn feed(runtime: &mut Runtime<Win>, recording: &str, from: usize, to: usize) {
    let part = lines(recording, from, to);
    runtime.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let _ = play(&mut hub.model, &part);
        }
    });
    rest(runtime);
}

fn centre(runtime: &Runtime<Win>, key: &str) -> Result<(f32, f32), Box<dyn Error>> {
    let shown = runtime.elements();
    let found = by_key(&shown, key).ok_or_else(|| format!("no element {key}"))?;
    let [x, y, w, h] = found.rect;
    Ok((x + w / 2.0, y + h / 2.0))
}

fn tap(runtime: &mut Runtime<Win>, key: &str) -> Check {
    let (x, y) = centre(runtime, key)?;
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    rest(runtime);
    Ok(())
}

fn strike(runtime: &mut Runtime<Win>, key: Key, mods: Mods) {
    runtime.key(KeyEvent { key, mods });
    rest(runtime);
}

fn cmd() -> Mods {
    Mods {
        cmd: true,
        ..Mods::default()
    }
}

fn live(runtime: &Runtime<Win>, key: &str) -> bool {
    let shown = runtime.elements();
    shown
        .iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// Every request line the window sends from here on.
fn sent(runtime: &Runtime<Win>) -> Result<Rc<RefCell<Vec<String>>>, Box<dyn Error>> {
    let lines = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&lines);
    runtime.state.hub.try_borrow_mut()?.set_sink(move |effect| {
        if let Effect::Wire(request) = effect {
            log.borrow_mut().push(request.line);
        }
    });
    Ok(lines)
}

#[test]
fn cmd_k_and_the_top_bars_search_box_each_open_the_command_box() -> Check {
    let mut runtime = runtime("shell/everything")?;
    strike(&mut runtime, Key::Text("k".into()), cmd());
    assert!(live(&runtime, "window.command-box"), "cmd+k opens it");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, "window.command-box"), "esc closes it");
    tap(&mut runtime, "shell.command-box")?;
    assert!(
        live(&runtime, "window.command-box"),
        "and so does the search box"
    );
    Ok(())
}

/// The sidebar's width, or none while it is hidden, once a toggle has settled.
fn sidebar_width(runtime: &mut Runtime<Win>) -> Option<f32> {
    for _ in 0..60 {
        runtime.frame(1.0 / 60.0);
    }
    let shown = runtime.elements();
    let live = shown.iter().filter(|e| !e.exiting);
    live.into_iter()
        .find(|e| e.key.as_deref() == Some("shell.sidebar"))
        .map(|e| e.rect[2])
}

#[test]
fn the_tables_key_hides_the_sidebar_and_shows_it_again() -> Check {
    let mut runtime = runtime("shell/everything")?;
    assert_eq!(sidebar_width(&mut runtime), Some(196.0), "open");
    let held = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("s".into()), held);
    assert_eq!(
        sidebar_width(&mut runtime),
        None,
        "alt+cmd+s hides it whole"
    );
    strike(&mut runtime, Key::Text("s".into()), held);
    assert_eq!(sidebar_width(&mut runtime), Some(196.0), "and opens it");
    Ok(())
}

#[test]
fn a_right_click_on_a_thread_row_opens_the_menu_for_that_thread() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let (x, y) = centre(
        &runtime,
        "shell.thread.migrate-sessions-to-uuids.sidebar-row",
    )?;
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    rest(&mut runtime);
    let head = row_text(&runtime.elements(), "window.context-menu-head")?;
    assert!(
        head.contains("Migrate sessions to UUIDs"),
        "the menu is about the thread, not a pane: {head:?}"
    );
    Ok(())
}

#[test]
fn what_is_attached_shows_above_the_composer_goes_with_the_prompt_and_is_cleared() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let lines = sent(&runtime)?;
    let thread = Address::new(Kind::Thread, "migrate-sessions-to-uuids")?;
    runtime.update(move |win, tx| {
        let run = Run::new(ATTACH).on(Target::Thing(thread));
        yd_ui::menu::command_box::run(win, tx, run);
    });
    rest(&mut runtime);
    let tray = row_text(&runtime.elements(), "chat-8.context-tray")?;
    assert!(tray.contains("1 attached"), "{tray:?}");
    tap(&mut runtime, "chat-8.composer.input")?;
    strike(&mut runtime, Key::Text("see this".into()), Mods::default());
    strike(&mut runtime, Key::Enter, Mods::default());
    // The scene's thread is mid-turn: the prompt, with what was attached, goes as it ends.
    runtime.update(|win, _| {
        let shown = win.hub.try_borrow().ok().and_then(|hub| {
            yd_ui::shown::thread(&hub, win.window).map(|t| t.id.clone())
        });
        let Some(id) = shown else {
            return;
        };
        let idle = format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{id}","update":{{"sessionUpdate":"state_update","state":"idle"}}}}}}"#
        );
        let line = yd_core::wire::event::WireEvent::Line(idle);
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Wire(line));
    });
    let lines = lines.borrow();
    let prompt = lines.iter().find(|line| line.contains("session/prompt"));
    let prompt = prompt.ok_or("no prompt was sent")?;
    assert!(
        prompt.contains("Attached: thread · Migrate sessions to UUIDs"),
        "the prompt names what was attached: {prompt}"
    );
    assert!(
        !live(&runtime, "chat-8.context-tray"),
        "and the tray is empty"
    );
    Ok(())
}

#[test]
fn the_pick_key_starts_picking_and_escape_stops() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let held = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("p".into()), held);
    assert!(live(&runtime, "overlay.pick"), "alt+cmd+p starts picking");
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(!live(&runtime, "overlay.pick"), "esc stops");
    Ok(())
}

#[test]
fn the_bell_counts_what_is_unread_and_opens_a_panel_that_escape_closes() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let unread = {
        let hub = runtime.state.hub.try_borrow()?;
        yd_panes::notifications::view::unread(&hub)
    };
    assert!(unread > 0, "the scene has news");
    assert_eq!(
        unread, 3,
        "three threads have news, as the mockup counts them"
    );
    let count = row_text(&runtime.elements(), "shell.bell.count")?;
    assert_eq!(count, "3");
    tap(&mut runtime, "shell.bell")?;
    assert!(
        live(&runtime, "notifications.notification-panel"),
        "the bell opens the panel"
    );
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(
        !live(&runtime, "notifications.notification-panel"),
        "esc closes it"
    );
    Ok(())
}

fn by_key<'a>(elements: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    elements
        .iter()
        .find(|element| element.key.as_deref() == Some(key))
}

/// The words inside the element `key`, left to right.
fn row_text(elements: &[ElementInfo], key: &str) -> Result<String, Box<dyn Error>> {
    let outer = by_key(elements, key).ok_or_else(|| format!("no element {key}"))?;
    let [x, y, w, h] = outer.rect;
    let mut inside: Vec<&ElementInfo> = elements
        .iter()
        .filter(|element| element.text.is_some())
        .filter(|element| {
            let [ex, ey, ew, eh] = element.rect;
            ex >= x - 0.5 && ey >= y - 0.5 && ex + ew <= x + w + 0.5 && ey + eh <= y + h + 0.5
        })
        .collect();
    inside.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    let words: Vec<&str> = inside.iter().filter_map(|e| e.text.as_deref()).collect();
    Ok(words.join(" "))
}

fn keys(elements: &[ElementInfo]) -> Vec<&str> {
    elements.iter().filter_map(|e| e.key.as_deref()).collect()
}

#[test]
fn the_status_bar_reads_the_thread_the_git_row_and_who_needs_you() -> Check {
    let shown = runtime("shell/mockup")?.elements();
    let bar = row_text(&shown, "shell.status-bar")?;
    for part in [
        "rate-limit-uploads ↑2 ↓0 · a1b9f3e",
        "Supervised",
        "Opus 5.5 · High",
        "41.2K of 200K 21%",
        "$0.38",
        "turn 2 · 14m",
        "2 need you",
        "2 working",
        "6 agents",
        "yi serve",
    ] {
        assert!(
            bar.contains(part),
            "{part:?} is not in the status bar: {bar:?}"
        );
    }
    Ok(())
}

#[test]
fn the_top_bar_names_the_thread_with_its_facts_and_the_bell_count() -> Check {
    let shown = runtime("shell/mockup")?.elements();
    let bar = row_text(&shown, "shell.top-bar")?;
    for part in [
        "atlas-api / Rate-limit the upload endpoint turn 2 · 14m · 6 agents · +54 −1",
        "Search or run a command ⌘K",
        "3",
    ] {
        assert!(
            bar.contains(part),
            "{part:?} is not in the top bar: {bar:?}"
        );
    }
    Ok(())
}

#[test]
fn the_tree_groups_threads_by_project_with_children_under_their_root() -> Check {
    let shown = runtime("shell/mockup")?.elements();
    let label = row_text(&shown, "shell.group.atlas-api.sidebar-label")?;
    assert_eq!(label, "atlas-api 6 threads · main");
    assert_eq!(
        row_text(&shown, "shell.group.docs.sidebar-label")?,
        "docs 2 threads"
    );
    let rows: Vec<&str> = keys(&shown)
        .into_iter()
        .filter(|key| key.starts_with("shell.thread."))
        .collect();
    let first: Vec<&str> = rows.iter().take(4).copied().collect();
    assert_eq!(
        first,
        [
            "shell.thread.rate-limit-the-upload-endpoint.sidebar-row",
            "shell.thread.turn-1-found-the-route.sidebar-row",
            "shell.thread.fork-sliding-window.sidebar-row",
            "shell.thread.turn-2-add-the-limit.sidebar-row",
        ]
    );
    assert_eq!(rows.len(), 19, "every mockup thread has a row");
    let cold = row_text(&shown, "shell.thread.why-is-cold-start-900-ms.sidebar-row")?;
    assert_eq!(cold, "Why is cold start 900 ms? 11m");
    Ok(())
}

#[test]
fn the_bars_and_the_sidebar_have_the_record_sizes() -> Check {
    let shown = runtime("shell/mockup")?.elements();
    let size = |key: &str| by_key(&shown, key).map(|e| (e.rect[2], e.rect[3]));
    assert_eq!(size("shell.top-bar").map(|s| s.1), Some(25.0));
    assert_eq!(size("shell.status-bar").map(|s| s.1), Some(17.0));
    assert_eq!(size("shell.sidebar").map(|s| s.0), Some(196.0));
    Ok(())
}

#[test]
fn the_sidebar_button_hides_the_tree_and_back() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let toggle = by_key(&runtime.elements(), "shell.sidebar-toggle")
        .map(|e| (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
        .ok_or("no sidebar toggle")?;
    runtime.pointer_move(toggle.0, toggle.1);
    runtime.pointer_down(toggle.0, toggle.1);
    runtime.pointer_up(toggle.0, toggle.1);
    assert_eq!(sidebar_width(&mut runtime), None);
    runtime.pointer_down(toggle.0, toggle.1);
    runtime.pointer_up(toggle.0, toggle.1);
    assert_eq!(sidebar_width(&mut runtime), Some(196.0));
    Ok(())
}

#[test]
fn picking_a_thread_shows_it_and_opens_it() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let key = "shell.thread.migrate-sessions-to-uuids.sidebar-row";
    // The bell holds open each thread that needs the user, so views are counted as they move.
    let total = |runtime: &Runtime<Win>| -> Result<u32, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        Ok(hub.model.wire.threads.values().map(|t| t.views).sum())
    };
    let before = total(&runtime)?;
    let at = by_key(&runtime.elements(), key)
        .map(|e| (e.rect[0] + 40.0, e.rect[1] + e.rect[3] / 2.0))
        .ok_or("no Migrate row")?;
    runtime.pointer_move(at.0, at.1);
    runtime.pointer_down(at.0, at.1);
    runtime.pointer_up(at.0, at.1);
    runtime.frame(1.0 / 60.0);
    let title = row_text(&runtime.elements(), "shell.title")?;
    assert!(
        title.starts_with("atlas-api / Migrate sessions to UUIDs"),
        "{title:?}"
    );
    let hub = runtime.state.hub.try_borrow()?;
    let views = hub
        .model
        .wire
        .threads
        .values()
        .find(|thread| thread.id.as_str() == "migrate-sessions-to-uuids")
        .map(|thread| thread.views);
    assert_eq!(views, Some(1), "picking a thread runs thread.open on it");
    drop(hub);
    assert_eq!(
        total(&runtime)?,
        before,
        "and thread.close on the one shown before"
    );
    Ok(())
}

#[test]
fn just_chat_is_the_document_alone_under_its_title_row() -> Check {
    let mut runtime = runtime("shell/just-chat")?;
    rest(&mut runtime);
    let shown = runtime.elements();
    let present = keys(&shown);
    for gone in [
        "shell.sidebar",
        "shell.status-bar",
        "shell.top-bar",
        "shell.command-box",
        "chat-8.pane-tab-bar",
    ] {
        assert!(!present.contains(&gone), "Just chat has no {gone}");
    }
    let title = row_text(&shown, "window.title")?;
    assert!(
        title.starts_with("Rate-limit the upload endpoint turn 2"),
        "{title:?}"
    );
    assert!(present.contains(&"chat-8.doc"), "the document is drawn");
    // The draft is the pane's own: what is typed here is there in Everything too.
    tap(&mut runtime, "chat-8.composer.input")?;
    strike(
        &mut runtime,
        Key::Text("still here".into()),
        Mods::default(),
    );
    runtime.update(|win, tx| view::run(win, tx, CommandId("layout.everything"), Target::App));
    rest(&mut runtime);
    let field = by_key(&runtime.elements(), "chat-8.composer.input").and_then(|e| e.text.clone());
    assert_eq!(field.as_deref(), Some("still here"));
    Ok(())
}

#[test]
fn with_no_threads_the_window_is_new_chat() -> Check {
    let shown = runtime("shell/empty")?.elements();
    let present = keys(&shown);
    assert!(present.contains(&"new-chat.chat-column"), "New chat shows");
    assert!(present.contains(&"shell.sidebar"), "beside the sidebar");
    Ok(())
}

#[test]
fn cmd_n_opens_new_chat_and_its_first_words_make_the_thread_it_then_shows() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let lines = sent(&runtime)?;
    rest(&mut runtime);
    // A pane opened in the thread shown first: it is that session's.
    runtime.update(|win, tx| view::run(win, tx, CommandId("pane.show-trace"), Target::App));
    rest(&mut runtime);
    let trace = |runtime: &Runtime<Win>| -> Result<bool, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        let window = hub.model.grid.window(root::MAIN).ok_or("no window")?;
        let mut docked = window.layout.docked();
        let trace = Some(yd_core::pane::PaneKind::Trace);
        Ok(docked.any(|(pane, _)| hub.model.grid.kind(pane) == trace))
    };
    assert!(trace(&runtime)?, "Trace is open in the first thread");
    let first = {
        let hub = runtime.state.hub.try_borrow()?;
        let shown = yd_ui::shown::thread(&hub, root::MAIN);
        shown.map(|thread| thread.id.clone()).ok_or("no thread")?
    };
    strike(&mut runtime, Key::Text("n".into()), cmd());
    assert!(live(&runtime, "new-chat.chat-column"), "the page shows");
    assert!(live(&runtime, "shell.sidebar"), "beside the sidebar");
    assert!(
        lines.borrow().is_empty(),
        "nothing is made until words are sent"
    );
    tap(&mut runtime, "new-chat.composer.input")?;
    strike(
        &mut runtime,
        Key::Text("fix the flake".into()),
        Mods::default(),
    );
    strike(&mut runtime, Key::Enter, Mods::default());
    let asked = lines
        .borrow()
        .iter()
        .find(|line| line.contains("session/new"))
        .cloned();
    let asked = asked.ok_or("no session/new")?;
    let id = asked.split("\"id\":").nth(1);
    let id = id
        .and_then(|rest| rest.split([',', '}']).next())
        .ok_or("no id")?;
    let made = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"fresh-thread"}}}}"#);
    runtime.update(move |win, _| {
        let line = yd_core::wire::event::WireEvent::Line(made);
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Wire(line));
    });
    rest(&mut runtime);
    assert!(!live(&runtime, "new-chat.chat-column"), "the page is gone");
    let hub = runtime.state.hub.try_borrow()?;
    let shown = yd_ui::shown::thread(&hub, root::MAIN).map(|thread| thread.id.as_str());
    assert_eq!(
        shown,
        Some("fresh-thread"),
        "and the window shows the new thread"
    );
    drop(hub);
    assert!(!trace(&runtime)?, "with none of the first thread's panes");
    let prompted = lines
        .borrow()
        .iter()
        .any(|line| line.contains("fix the flake"));
    assert!(prompted, "which got the words as its first prompt");
    runtime.update(move |win, _| yd_ui::shown::show(win, first));
    rest(&mut runtime);
    assert!(trace(&runtime)?, "which the first thread still has");
    Ok(())
}

#[test]
fn a_card_in_new_chat_shows_its_thread_and_escape_just_leaves() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    strike(&mut runtime, Key::Text("n".into()), cmd());
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(
        !live(&runtime, "new-chat.chat-column"),
        "esc leaves New chat"
    );
    strike(&mut runtime, Key::Text("n".into()), cmd());
    let card = {
        let shown = runtime.elements();
        let mut cards = shown.iter().filter_map(|e| e.key.clone());
        cards
            .find(|key| key.starts_with("new-chat.card."))
            .ok_or("no card")?
    };
    tap(&mut runtime, &card)?;
    assert!(!live(&runtime, "new-chat.chat-column"));
    let hub = runtime.state.hub.try_borrow()?;
    let shown = yd_ui::shown::thread(&hub, root::MAIN).map(|thread| thread.id.to_string());
    let wanted = card.strip_prefix("new-chat.card.").map(str::to_owned);
    assert_eq!(shown, wanted, "the card's thread shows");
    Ok(())
}

/// Dies with Return on New chat answering another thread's pending ask, with no card in view.
#[test]
fn return_on_new_chat_answers_no_approval_it_cannot_show() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let lines = sent(&runtime)?;
    let waiting = runtime.state.hub.try_borrow()?.model.wire.asks.len();
    assert!(waiting > 0, "the scene has a pending ask");
    strike(&mut runtime, Key::Text("n".into()), cmd());
    lines.borrow_mut().clear();
    strike(&mut runtime, Key::Enter, Mods::default());
    assert_eq!(
        *lines.borrow(),
        Vec::<String>::new(),
        "an ask was answered blind"
    );
    Ok(())
}

/// The scene after the frames a layout read needs to land.
fn settled(name: &str) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut runtime = runtime(name)?;
    for _ in 0..3 {
        runtime.frame(1.0 / 60.0);
    }
    Ok(runtime)
}

#[test]
fn the_grid_fills_the_shells_slot_with_every_pane_of_the_layout() -> Check {
    let shown = settled("shell/everything")?.elements();
    let [x, y, w, h] = by_key(&shown, "shell.grid").ok_or("no grid slot")?.rect;
    assert_eq!((x, y, w, h), (196.0, 25.0, 1004.0, 708.0));
    let frames: Vec<[f32; 4]> = shown
        .iter()
        .filter(|e| e.key.as_deref().is_some_and(|k| k.ends_with(".pane-frame")))
        .map(|e| e.rect)
        .collect();
    assert_eq!(frames.len(), 8, "Everything has eight panes");
    let inside =
        |r: &[f32; 4]| r[0] >= x && r[1] >= y && r[0] + r[2] <= x + w && r[1] + r[3] <= y + h;
    assert!(
        frames.iter().all(inside),
        "a pane leaves the slot: {frames:?}"
    );
    let covered: f32 = frames.iter().map(|r| r[2] * r[3]).sum();
    assert!(
        (covered - w * h).abs() < 1.0,
        "the panes tile the slot: {covered}"
    );
    Ok(())
}

#[test]
fn every_built_pane_of_everything_draws_its_own_mockup_state() -> Check {
    let shown = settled("shell/everything")?.elements();
    let drawn = |slug: &str, part: &str| {
        shown.iter().any(|e| {
            let key = e.key.as_deref().unwrap_or_default();
            !e.exiting && key.starts_with(slug) && key.ends_with(part)
        })
    };
    for (slug, part, what) in [
        ("chat-", ".asks.row.perm-1", "the chat's waiting ask"),
        ("code-", ".L1", "the code viewer's open file"),
        ("changes-", ".L20", "the changes pane's diff"),
        ("files-", "upload.rs", "the files pane's tree"),
        ("terminal-", ".tab-0.pane-tab", "the terminal's agent tab"),
    ] {
        assert!(drawn(slug, part), "{what} is not drawn");
    }
    let screen = shown.iter().filter_map(|e| e.label.as_deref());
    assert!(
        screen
            .clone()
            .any(|label| label.contains("src/routes/upload.rs")),
        "the terminal's shell shows nothing"
    );
    let bars = shown.iter().filter(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.ends_with(".pane-tab-bar")
    });
    assert_eq!(bars.count(), 8, "one tab bar a pane");
    Ok(())
}

#[test]
fn the_status_bar_says_how_long_the_server_took_to_answer() -> Check {
    let shown = runtime("shell/mockup")?.elements();
    let bar = row_text(&shown, "shell.status-bar")?;
    assert!(bar.ends_with("yi serve 12 ms"), "{bar:?}");
    Ok(())
}

#[test]
fn the_window_shows_the_thread_the_list_puts_first_and_points_the_sources_at_it() -> Check {
    use std::cell::RefCell;
    use std::rc::Rc;
    use yd_core::feed::{CHANGES, FILES, LSP, SourceCmd};
    use yd_core::msg::Effect;

    let mut win = root::init();
    let asked = Rc::new(RefCell::new(Vec::new()));
    let index = scenes::all()
        .iter()
        .position(|scene| scene.name == "shell/mockup");
    show_scene(&mut win, index.ok_or("no scene")?);
    let log = Rc::clone(&asked);
    win.hub
        .try_borrow_mut()?
        .set_sink(move |effect| log.borrow_mut().push(effect));
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.frame(1.0 / 60.0);
    let roots: Vec<Effect> = asked
        .borrow()
        .iter()
        .filter(|effect| matches!(effect, Effect::Source(_, SourceCmd::Root(_))))
        .cloned()
        .collect();
    let folder = || SourceCmd::Root("/work/atlas-api".into());
    assert_eq!(
        roots,
        [
            Effect::Source(FILES, folder()),
            Effect::Source(CHANGES, folder()),
            Effect::Source(LSP, folder()),
        ],
        "nobody pressed a thread: the one shown is still opened"
    );
    let hub = runtime.state.hub.try_borrow()?;
    let shown = hub
        .model
        .wire
        .threads
        .values()
        .find(|thread| thread.views > 0);
    assert_eq!(
        shown.and_then(|thread| thread.title.as_deref()),
        Some("Rate-limit the upload endpoint")
    );
    Ok(())
}

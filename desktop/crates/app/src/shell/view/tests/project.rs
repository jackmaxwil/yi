//! Pointing a new thread at a project: the default folder, the chip's menu, the folder
//! chooser, Open Folder and a dropped folder, through the window as the app runs it.

use std::path::{Path, PathBuf};

use rune::{Env, FileDialogKind, Key, Mods, Runtime};
use yd_core::ids::SettingKey;
use yd_core::msg::{Msg, step};
use yd_core::settings::schema::SettingValue;
use yd_core::settings::store::{PROJECTS, SettingsMsg};
use yd_panes::new_chat::view::{CHOOSE, PROJECT, StartedIn};
use yd_ui::win::Win;

use super::{Check, by_key, cmd, live, rest, runtime, sent, strike, tap};
use crate::root;

const EMPTY: &str = include_str!("../../../../../../fixtures/wire/empty-list.jsonl");

/// A folder that exists, under the lane's target, named `name`.
fn folder(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/project-tests");
    let dir = dir.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.canonicalize()?)
}

fn text(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(path.to_str().ok_or("not UTF-8")?.to_owned())
}

/// The app's window on a server that has no thread at all, started in `started`.
fn empty(started: Option<String>) -> Result<Runtime<Win>, Box<dyn std::error::Error>> {
    let win = root::init();
    {
        let mut hub = win.hub.try_borrow_mut()?;
        yd_core::wire::replay::play(&mut hub.model, EMPTY)?;
        if let Some(kept) = hub.panes.get_mut::<StartedIn>(None) {
            kept.0 = started;
        }
    }
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    Ok(runtime)
}

fn known(runtime: &mut Runtime<Win>, folders: &[&Path]) -> Check {
    let lines: Vec<&str> = folders.iter().filter_map(|f| f.to_str()).collect();
    let value = {
        let quoted: Vec<String> = lines.iter().map(|line| format!("{line:?}")).collect();
        SettingValue::Json(format!("[{}]", quoted.join(",")).into())
    };
    let set = SettingsMsg::Set(SettingKey::new(PROJECTS), Some(value));
    let hub = runtime.state.hub.clone();
    runtime.update(move |_, _| {
        if let Ok(mut hub) = hub.try_borrow_mut() {
            step(&mut hub.model, Msg::Settings(set));
        }
    });
    rest(runtime);
    Ok(())
}

/// What the project chip says it is, to a person and to a screen reader.
fn chip(runtime: &Runtime<Win>) -> Result<String, Box<dyn std::error::Error>> {
    let shown = runtime.elements();
    let chip = by_key(&shown, PROJECT).ok_or("no project chip")?;
    Ok(chip.label.clone().unwrap_or_default())
}

#[test]
fn new_chat_starts_where_the_app_was_started_else_in_the_newest_project() -> Check {
    let started = folder("started")?;
    let mut runtime = empty(Some(text(&started)?))?;
    assert!(
        live(&runtime, "new-chat.chat-column"),
        "no thread: New chat"
    );
    let tally = folder("tally")?;
    known(&mut runtime, &[&tally])?;
    assert_eq!(
        chip(&runtime)?,
        "Project: started",
        "the launch folder first"
    );
    let mut runtime = empty(None)?;
    known(&mut runtime, &[&tally])?;
    assert_eq!(chip(&runtime)?, "Project: tally", "then the newest project");
    Ok(())
}

#[test]
fn the_project_chip_lists_recent_folders_and_a_pick_moves_new_chat_there() -> Check {
    let mut runtime = empty(None)?;
    let (tally, atlas) = (folder("tally")?, folder("atlas")?);
    let gone = Path::new("/no/such/folder");
    known(&mut runtime, &[&tally, gone, &atlas])?;
    tap(&mut runtime, PROJECT)?;
    let menu = runtime.popup_menu().ok_or("the chip opened no menu")?;
    let title = |id: &str| menu.find(id).map(|item| item.title().to_owned());
    assert_eq!(
        title(&text(&tally)?).as_deref(),
        Some("tally"),
        "named by its last part"
    );
    assert_eq!(title(&text(&atlas)?).as_deref(), Some("atlas"));
    assert_eq!(title(CHOOSE).as_deref(), Some("Choose a folder…"));
    assert_eq!(
        title("/no/such/folder"),
        None,
        "a folder that is gone is not listed"
    );
    runtime.resolve_popup(Some(&text(&atlas)?))?;
    rest(&mut runtime);
    assert_eq!(chip(&runtime)?, "Project: atlas");
    Ok(())
}

#[test]
fn choose_a_folder_opens_the_chooser_and_the_thread_starts_in_the_folder_chosen() -> Check {
    let mut runtime = empty(None)?;
    let lines = sent(&runtime)?;
    let chosen = folder("chosen")?;
    tap(&mut runtime, PROJECT)?;
    runtime.resolve_popup(Some(CHOOSE))?;
    rest(&mut runtime);
    let kind = runtime.file_dialog().map(|(kind, _)| kind);
    assert_eq!(
        kind,
        Some(FileDialogKind::Folder),
        "the Mac's folder chooser"
    );
    runtime.resolve_file_dialog(Some(chosen.clone()))?;
    rest(&mut runtime);
    assert_eq!(chip(&runtime)?, "Project: chosen");
    strike(&mut runtime, Key::Text("make it".into()), Mods::default());
    strike(&mut runtime, Key::Enter, Mods::default());
    let new = lines
        .borrow()
        .iter()
        .find(|l| l.contains("session/new"))
        .cloned();
    let new = new.ok_or("no session/new")?;
    let cwd = format!(r#""cwd":"{}""#, text(&chosen)?);
    assert!(
        new.contains(&cwd),
        "the thread starts in the folder chosen: {new}"
    );
    let hub = runtime.state.hub.try_borrow()?;
    let projects = hub.model.settings.projects();
    assert_eq!(
        projects.first(),
        Some(&text(&chosen)?),
        "kept as a known project"
    );
    Ok(())
}

#[test]
fn open_folder_on_cmd_o_shows_new_chat_on_the_folder_chosen() -> Check {
    let mut runtime = runtime("shell/everything")?;
    assert!(!live(&runtime, "new-chat.chat-column"), "a thread shows");
    strike(&mut runtime, Key::Text("o".into()), cmd());
    let kind = runtime.file_dialog().map(|(kind, _)| kind);
    assert_eq!(kind, Some(FileDialogKind::Folder));
    runtime.resolve_file_dialog(Some(folder("opened")?))?;
    rest(&mut runtime);
    assert!(live(&runtime, "new-chat.chat-column"), "New chat shows");
    assert_eq!(chip(&runtime)?, "Project: opened");
    Ok(())
}

#[test]
fn a_folder_dropped_on_the_window_shows_new_chat_on_it() -> Check {
    let mut runtime = runtime("shell/everything")?;
    let dropped = folder("dropped")?;
    let file = dropped.join("README");
    std::fs::write(&file, "seed\n")?;
    runtime.drag_files(vec![file.clone()], Some((600.0, 400.0)));
    runtime.drop_files(vec![file], Some((600.0, 400.0)));
    rest(&mut runtime);
    assert!(
        !live(&runtime, "new-chat.chat-column"),
        "a file is no project"
    );
    runtime.drag_files(vec![dropped.clone()], Some((600.0, 400.0)));
    runtime.drop_files(vec![dropped], Some((600.0, 400.0)));
    rest(&mut runtime);
    assert!(live(&runtime, "new-chat.chat-column"), "New chat shows");
    assert_eq!(chip(&runtime)?, "Project: dropped");
    Ok(())
}

/// Dogfood: a project whose folder was deleted was still asked for its threads every poll.
#[test]
fn a_known_project_whose_folder_is_gone_is_forgotten_and_its_threads_never_asked_for() -> Check {
    use yd_core::ids::Millis;
    use yd_core::wire::event::WireEvent;
    let mut runtime = empty(None)?;
    let (tally, gone) = (folder("tally")?, folder("gone")?);
    std::fs::remove_dir(&gone)?;
    known(&mut runtime, &[&gone, &tally])?;
    let projects = runtime.state.hub.try_borrow()?.model.settings.projects();
    assert_eq!(
        projects,
        [text(&tally)?],
        "the folder that is gone is dropped"
    );
    let lines = sent(&runtime)?;
    runtime.update(|win, _| {
        let poll = WireEvent::Poll { at: Millis(1) };
        yd_ui::win::dispatch(win, Msg::Wire(poll));
    });
    rest(&mut runtime);
    let lists: Vec<String> = lines
        .borrow()
        .iter()
        .filter(|l| l.contains("session/list"))
        .cloned()
        .collect();
    let asks = |folder: &Path| {
        lists
            .iter()
            .any(|l| l.contains(&format!("{}\"", folder.display())))
    };
    assert!(
        asks(&tally),
        "the folder that is there is asked for: {lists:?}"
    );
    assert!(!asks(&gone), "the one that is gone is not: {lists:?}");
    Ok(())
}

/// Dies with a New chat whose chip says "0" and whose menu lists the thread's checkout, a
/// folder named for a counter, beside the project it came from.
#[test]
fn a_thread_in_a_checkout_leaves_new_chat_on_its_project() -> Check {
    use yd_core::ids::ThreadId;
    use yd_core::reduce::threads::{Thread, Workdir};
    let mut runtime = empty(None)?;
    let (project, lane) = (folder("tally")?, folder("lane-home/5e1f0c2a/0")?);
    {
        let mut hub = runtime.state.hub.try_borrow_mut()?;
        let id = ThreadId::new("5e1f0c2a");
        let mut thread = Thread::new(id.clone());
        thread.cwd = text(&project)?;
        thread.workdir = Some(Workdir {
            cwd: text(&lane)?,
            lane: Some(text(&lane)?),
        });
        hub.model.wire.threads.insert(id, thread);
    }
    runtime.update(|win, _| yd_ui::shown::start(win));
    rest(&mut runtime);
    assert_eq!(
        chip(&runtime)?,
        "Project: tally",
        "not the checkout's last part"
    );
    tap(&mut runtime, PROJECT)?;
    let menu = runtime.popup_menu().ok_or("the chip opened no menu")?;
    assert!(
        menu.find(&text(&project)?).is_some(),
        "the project is listed"
    );
    assert!(
        menu.find(&text(&lane)?).is_none(),
        "a checkout is not a project"
    );
    Ok(())
}

/// A window with no thread whose one shell started in `/started`, counting the shells hung up.
/// The window and how many shells it hung up.
type Hung = (Runtime<Win>, std::rc::Rc<std::cell::Cell<usize>>);

fn started_shell() -> Result<Hung, Box<dyn std::error::Error>> {
    use yd_core::feed::SourceCmd;
    use yd_core::msg::Effect;
    use yd_core::pane::PaneKind;
    let runtime = empty(None)?;
    let closed = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let kept = std::rc::Rc::clone(&closed);
    let mut hub = runtime.state.hub.try_borrow_mut()?;
    hub.set_sink(move |effect| {
        if matches!(effect, Effect::Source(_, SourceCmd::Close)) {
            kept.set(kept.get() + 1);
        }
    });
    let pane = hub
        .model
        .grid
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Terminal);
    let pane = pane.map(|pane| pane.id).ok_or("no terminal pane")?;
    let shells = hub
        .panes
        .get_mut::<yd_ui::term::view::state::Shells>(Some(pane));
    for shell in shells.ok_or("no shells")?.tabs.iter_mut() {
        shell.cwd = "/started".to_owned();
    }
    drop(hub);
    Ok((runtime, closed))
}

/// Dies with a shell that goes on in the folder the app started in after the person chose
/// another project for New chat: the terminal's next shell must start where they pointed.
#[test]
fn choosing_a_project_hangs_up_the_shells_so_the_next_starts_there() -> Check {
    let (mut runtime, closed) = started_shell()?;
    let chosen = folder("chosen-shell")?;
    tap(&mut runtime, PROJECT)?;
    runtime.resolve_popup(Some(CHOOSE))?;
    rest(&mut runtime);
    runtime.resolve_file_dialog(Some(chosen))?;
    rest(&mut runtime);
    assert_eq!(closed.get(), 1, "the window's one shell is hung up");
    Ok(())
}

/// Dies with the first shell left in the folder the app started in, `projects`, when the
/// thread the list brings works in `tally`: it started before the server had said anything.
#[test]
fn a_shell_started_before_the_first_thread_moves_to_the_thread_s_folder() -> Check {
    use yd_core::ids::ThreadId;
    use yd_core::reduce::threads::Thread;
    let (mut runtime, closed) = started_shell()?;
    {
        let mut hub = runtime.state.hub.try_borrow_mut()?;
        let id = ThreadId::new("5e1f0c2a");
        let mut thread = Thread::new(id.clone());
        thread.cwd = text(&folder("tally")?)?;
        hub.model.wire.threads.insert(id, thread);
    }
    rest(&mut runtime);
    assert_eq!(
        closed.get(),
        1,
        "the shell in /started is hung up for one in tally"
    );
    Ok(())
}

/// Dies with a recent-folders list that offers a Yi checkout, a folder named "0", as a project.
#[test]
fn a_stored_checkout_is_never_offered_as_a_project() -> Check {
    let mut runtime = empty(None)?;
    let (tally, lane) = (folder("tally")?, folder("home/.yi/lanes/5e1f0c2a/0")?);
    known(&mut runtime, &[&lane, &tally])?;
    tap(&mut runtime, PROJECT)?;
    let menu = runtime.popup_menu().ok_or("the chip opened no menu")?;
    assert!(menu.find(&text(&tally)?).is_some(), "the project is listed");
    assert!(
        menu.find(&text(&lane)?).is_none(),
        "the checkout is offered"
    );
    assert_eq!(
        chip(&runtime)?,
        "Project: tally",
        "New chat starts in a project, not a lane"
    );
    Ok(())
}

/// Dies with the first shell in the home folder: with no thread shown yet it asked for none.
#[test]
fn a_shell_with_no_thread_yet_starts_in_the_project_new_chat_names() -> Check {
    use std::rc::Rc;
    use yd_core::ids::Millis;
    use yd_core::inbox::inbox;
    use yd_core::pane::PaneKind;
    use yd_core::wire::client::Clock;
    use yd_ui::term::view::state::Shells;
    let project = folder("shell-tally")?;
    let mut runtime = empty(None)?;
    known(&mut runtime, &[&project])?;
    let hub = runtime.state.hub.clone();
    let (pane, feed) = {
        let mut hub = hub.try_borrow_mut()?;
        let found = hub
            .model
            .grid
            .panes
            .iter()
            .find(|p| p.kind == PaneKind::Terminal);
        let pane = found.map(|pane| pane.id).ok_or("no terminal pane")?;
        let shells = hub.panes.get_mut::<Shells>(Some(pane)).ok_or("no shells")?;
        (
            pane,
            shells
                .tabs
                .first()
                .map(|shell| shell.feed)
                .ok_or("no shell")?,
        )
    };
    let (sender, _inbox) = inbox(|| {});
    let clock: Clock = std::sync::Arc::new(|| Millis(0));
    let running = crate::start::Running::new(Rc::downgrade(&hub), sender, clock);
    let (_, spawn) = running.shell(feed).ok_or("no spawn")?;
    assert_eq!(spawn.cwd, project);
    let hub = hub.try_borrow()?;
    let first = hub
        .panes
        .get::<Shells>(Some(pane))
        .and_then(|s| s.tabs.first());
    assert_eq!(
        first.map(|shell| shell.cwd.as_str()),
        project.to_str(),
        "the tab keeps it"
    );
    Ok(())
}

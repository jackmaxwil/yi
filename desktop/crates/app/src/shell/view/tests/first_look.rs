//! The owner's first look: the zoom, chat on the left, a blank first window and no false age.

use std::path::{Path, PathBuf};

use yd_core::grid::layout::ChatSide;
use yd_core::ids::ThreadId;
use yd_core::pane::PaneKind;
use yd_core::reduce::threads::Thread;

use super::*;
use crate::start::{self, Files};
use crate::windows::places::Places;

/// An empty folder of this test's own under the lane's target.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/df13-first-look")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("themes"))?;
    Ok(dir)
}

/// The window as the native app starts it over `folder`'s files, the zoom declared as it is.
fn started(folder: &Path) -> Result<Runtime<Win>, Box<dyn Error>> {
    let win = root::init();
    {
        let mut hub = win.hub.try_borrow_mut()?;
        hub.model.settings.rows.push(root::ZOOM);
        start::files(&mut hub, Files::under(folder));
    }
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    Ok(runtime)
}

#[test]
fn command_equals_minus_and_zero_zoom_the_window_and_a_restart_keeps_it() -> Check {
    let dir = folder("zoom")?;
    let mut runtime = started(&dir)?;
    assert_eq!(
        runtime.zoom(),
        1.25,
        "larger than the Mac's points at first"
    );
    strike(&mut runtime, Key::Text("=".into()), cmd());
    assert_eq!(runtime.zoom(), 1.5);
    let shifted = Mods {
        shift: true,
        ..cmd()
    };
    strike(&mut runtime, Key::Text("+".into()), shifted);
    assert_eq!(runtime.zoom(), 1.75, "⌘+ zooms in as ⌘= does");
    strike(&mut runtime, Key::Text("-".into()), cmd());
    assert_eq!(runtime.zoom(), 1.5);
    strike(&mut runtime, Key::Text("0".into()), cmd());
    assert_eq!(runtime.zoom(), 1.0, "actual size");
    strike(&mut runtime, Key::Text("-".into()), cmd());
    start::quit(&mut runtime.state);
    assert_eq!(started(&dir)?.zoom(), 0.9, "kept across a restart");
    Ok(())
}

/// The x of the pane of `kind` the main window draws.
fn pane_x(runtime: &Runtime<Win>, kind: PaneKind) -> Result<f32, Box<dyn Error>> {
    let pane = {
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let docked = grid.window(root::MAIN).ok_or("no window")?.layout.docked();
        let mut panes = docked.map(|(pane, _)| pane);
        panes
            .find(|pane| grid.kind(*pane) == Some(kind))
            .ok_or("not drawn")?
    };
    let key = format!("{}-{}", kind.slug(), pane.0);
    let shown = runtime.elements();
    Ok(by_key(&shown, &key).ok_or("no pane element")?.rect[0])
}

#[test]
fn chat_opens_on_the_left_and_a_side_chosen_since_is_kept() -> Check {
    let runtime = runtime("shell/everything")?;
    let (chat, files) = (
        pane_x(&runtime, PaneKind::Chat)?,
        pane_x(&runtime, PaneKind::Files)?,
    );
    assert!(chat < files, "chat at {chat}, Files at {files}");
    let grid = runtime.state.hub.try_borrow()?.model.grid.clone();
    let dir = folder("side")?;
    let file = dir.join("places.json");
    Places::at(&file).keep(&grid)?;
    // A file an older build wrote names its default, the right, under the old key.
    let old = std::fs::read_to_string(&file)?.replace(r#""chat":"left""#, r#""side":"right""#);
    std::fs::write(&file, old)?;
    let read = Places::at(&file).read(root::MAIN, &[]).ok_or("not read")?;
    assert_eq!(read.side, ChatSide::Left, "the old default is not a choice");
    let chosen = yd_core::grid::windows::Windows {
        side: ChatSide::Right,
        ..grid
    };
    Places::at(&file).keep(&chosen)?;
    let read = Places::at(&file).read(root::MAIN, &[]).ok_or("not read")?;
    assert_eq!(read.side, ChatSide::Right, "a choice made since is kept");
    Ok(())
}

/// The pane kinds whose root the window draws.
fn drawn(runtime: &Runtime<Win>) -> Result<Vec<PaneKind>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let shown = runtime.elements();
    let grid = &hub.model.grid;
    let panes = grid.panes.iter().filter(|pane| {
        let key = format!("{}-{}", pane.kind.slug(), pane.id.0);
        by_key(&shown, &key).is_some()
    });
    Ok(panes.map(|pane| pane.kind).collect())
}

#[test]
fn a_first_launch_shows_the_chat_alone_until_a_pane_is_opened() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    say(&mut runtime, "agents never");
    runtime.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            root::blank(&mut hub, root::MAIN);
        }
    });
    rest(&mut runtime);
    assert_eq!(drawn(&runtime)?, [PaneKind::Chat], "a blank window");
    assert!(live(&runtime, "shell.sidebar"), "beside the sidebar");
    runtime.update(|win, tx| {
        yd_ui::grid_view::view::show_pane(win, tx, PaneKind::Files);
    });
    rest(&mut runtime);
    let mut kinds = drawn(&runtime)?;
    kinds.sort_by_key(|kind| kind.slug());
    assert_eq!(
        kinds,
        [PaneKind::Chat, PaneKind::Files],
        "opened when asked"
    );
    Ok(())
}

#[test]
fn a_thread_the_list_gave_no_time_shows_no_age() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    runtime.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let id = ThreadId::new("01a070-no-time");
            let mut thread = Thread::new(id.clone());
            thread.cwd = "/work/atlas-api".to_owned();
            thread.title = Some("Stored, never listed live".to_owned());
            hub.model.wire.threads.insert(id, thread);
        }
    });
    rest(&mut runtime);
    let row = row_text(
        &runtime.elements(),
        "shell.thread.01a070-no-time.sidebar-row",
    )?;
    assert_eq!(row, "Stored, never listed live", "no age, not 2967w");
    Ok(())
}

/// A file pressed in Files opens in the Code pane at once, the pane opened when the layout
/// has none: Review has Files and no Code, and a press there did nothing.
#[test]
fn a_file_pressed_where_the_layout_has_no_code_pane_opens_one_with_the_file() -> Check {
    let mut runtime = runtime("shell/everything")?;
    runtime.update(|win, tx| {
        yd_ui::grid_view::view::act(win, tx, yd_ui::grid_view::view::Act::Show("review"));
    });
    rest(&mut runtime);
    assert!(
        !drawn(&runtime)?.contains(&PaneKind::Code),
        "Review has none"
    );
    let row = keys(&runtime.elements())
        .into_iter()
        .find(|key| key.starts_with("files-") && key.ends_with(".row.src/main.rs"))
        .ok_or("no main.rs row in Files")?
        .to_owned();
    tap(&mut runtime, &row)?;
    assert!(
        drawn(&runtime)?.contains(&PaneKind::Code),
        "the Code pane opened"
    );
    let tabs: Vec<String> = keys(&runtime.elements())
        .into_iter()
        .filter(|key| key.starts_with("code-") && key.ends_with(".pane-tab"))
        .map(|key| row_text(&runtime.elements(), key).unwrap_or_default())
        .collect();
    assert!(tabs.iter().any(|tab| tab.contains("main.rs")), "{tabs:?}");
    Ok(())
}

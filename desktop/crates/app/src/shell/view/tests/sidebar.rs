//! The sidebar's own controls: its edge resizes it, and New chat and New project head it.

use rune::{Cursor, FileDialogKind};
use yd_core::command::table::OPEN_FOLDER;
use yd_core::reduce::step::commands::NEW;
use yd_ui::menu::command_box;

use super::*;

const EDGE: &str = "shell.sidebar.edge";

fn width(runtime: &Runtime<Win>) -> Option<f32> {
    let shown = runtime.elements();
    let live = shown.iter().filter(|e| !e.exiting);
    let sidebar = live
        .into_iter()
        .find(|e| e.key.as_deref() == Some("shell.sidebar"));
    sidebar.map(|e| e.rect[2])
}

/// Presses the sidebar's edge and lets go `by` points along, in steps as a hand moves.
fn drag(runtime: &mut Runtime<Win>, by: f32) -> Check {
    let (x, y) = centre(runtime, EDGE)?;
    let to = x + by;
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.frame(1.0 / 60.0);
    for step in 1..=8u8 {
        let at = x + (to - x) * f32::from(step) / 8.0;
        runtime.pointer_move(at, y);
        runtime.frame(1.0 / 60.0);
    }
    runtime.pointer_up(to, y);
    rest(runtime);
    Ok(())
}

#[test]
fn the_sidebar_edge_resizes_it_within_its_bounds_and_a_double_click_resets_it() -> Check {
    let mut runtime = runtime("shell/everything")?;
    assert_eq!(width(&runtime), Some(196.0), "the default");
    let (x, y) = centre(&runtime, EDGE)?;
    runtime.pointer_move(x, y);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.cursor(), Cursor::ResizeColumn, "on the edge");
    drag(&mut runtime, 100.0)?;
    assert_eq!(width(&runtime), Some(296.0), "follows the pointer");
    drag(&mut runtime, 500.0)?;
    assert_eq!(
        width(&runtime),
        Some(400.0),
        "a third of the window at most"
    );
    drag(&mut runtime, -300.0)?;
    assert_eq!(width(&runtime), Some(160.0), "never so narrow names go");
    let (x, y) = centre(&runtime, EDGE)?;
    for _ in 0..2 {
        runtime.pointer_down(x, y);
        runtime.frame(1.0 / 60.0);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
    }
    rest(&mut runtime);
    assert_eq!(width(&runtime), Some(196.0), "a double-click resets it");
    drag(&mut runtime, 64.0)?;
    let held = Mods { alt: true, ..cmd() };
    strike(&mut runtime, Key::Text("s".into()), held);
    assert_eq!(width(&runtime), None, "hidden stays hidden");
    strike(&mut runtime, Key::Text("s".into()), held);
    assert_eq!(width(&runtime), Some(260.0), "and comes back as wide");
    Ok(())
}

#[test]
fn new_chat_and_new_project_head_the_sidebar_with_their_keys() -> Check {
    let mut runtime = runtime("shell/mockup")?;
    let keys = {
        let hub = runtime.state.hub.try_borrow()?;
        let map = command_box::keymap(&hub);
        (
            map.hint(NEW).ok_or("no key")?,
            map.hint(OPEN_FOLDER).ok_or("no key")?,
        )
    };
    let shown = runtime.elements();
    let chat = by_key(&shown, "shell.sidebar.new-chat").ok_or("no New chat")?;
    assert_eq!(chat.label.as_deref(), Some("New chat"));
    assert_eq!(
        chat.help.as_deref(),
        Some(format!("New chat {}", keys.0).as_str())
    );
    let project = by_key(&shown, "shell.sidebar.new-project").ok_or("no New project")?;
    assert_eq!(project.label.as_deref(), Some("New project"));
    assert_eq!(
        project.help.as_deref(),
        Some(format!("New project {}", keys.1).as_str())
    );
    tap(&mut runtime, "shell.sidebar.new-chat")?;
    assert!(live(&runtime, "new-chat.chat-column"), "New chat shows");
    let label = by_key(&runtime.elements(), "new-chat.project").and_then(|e| e.label.clone());
    assert_eq!(
        label.as_deref(),
        Some("Project: atlas-api"),
        "on the shown project"
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/df15/tally");
    std::fs::create_dir_all(&dir)?;
    tap(&mut runtime, "shell.sidebar.new-project")?;
    let kind = runtime.file_dialog().map(|(kind, _)| kind);
    assert_eq!(kind, Some(FileDialogKind::Folder), "the folder chooser");
    runtime.resolve_file_dialog(Some(dir.canonicalize()?))?;
    rest(&mut runtime);
    let label = by_key(&runtime.elements(), "new-chat.project").and_then(|e| e.label.clone());
    assert_eq!(label.as_deref(), Some("Project: tally"), "New chat on it");
    assert!(
        live(&runtime, "shell.group.tally.sidebar-label"),
        "and the sidebar lists it as a project"
    );
    Ok(())
}

//! A file dragged out of the Files pane in the real window: carried by the pointer, shown
//! where it would land, and opened there when let go.

use yd_core::ids::PaneId;
use yd_core::pane::PaneKind;
use yd_ui::grid_view::view::Grips;

use super::docking::{carry, near, part, tiles};
use super::beside::about;
use super::*;

const MAIN_RS: &str = "files-1.row.src/main.rs";
const SESSIONS: &str = "files-1.row.src/routes/sessions.rs";
const CHIP: &str = "grid.carried";
const ZONE: &str = "grid.drop-zone";

/// The kinds of the tabs in the slot `pane` is in, in tab order.
fn tabs(runtime: &Runtime<Win>, pane: PaneId) -> Result<Vec<PaneKind>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let tabs = grid.tabs(root::MAIN, pane);
    Ok(tabs.iter().filter_map(|pane| grid.kind(*pane)).collect())
}

fn focus(runtime: &Runtime<Win>) -> Result<Option<PaneId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let grips = hub.panes.get::<Grips>(None);
    Ok(grips.and_then(|grips| grips.of(root::MAIN).focus))
}

fn frame(runtime: &Runtime<Win>, pane: PaneId) -> Result<[f32; 4], Box<dyn Error>> {
    let key = format!("code-{}.pane-frame", pane.0);
    let found = by_key(&runtime.elements(), &key).map(|frame| frame.rect);
    Ok(found.ok_or(format!("no {key}"))?)
}

fn middle(rect: [f32; 4]) -> (f32, f32) {
    (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
}

/// Held over the terminal's middle, the file's name is by the pointer, the slot is marked
/// and nothing has opened; let go, it is the tab in front of that slot, with the keys.
#[test]
fn a_file_dragged_onto_a_panes_middle_is_the_tab_in_front_there() -> Check {
    let mut runtime = settled("shell/everything")?;
    let shown = runtime.elements();
    let shell = part(&shown, "terminal", ".pane-frame")?;
    let (from, to) = (centre(&runtime, MAIN_RS)?, middle(shell));
    carry(&mut runtime, from, to);
    let held = runtime.elements();
    let chip = by_key(&held, CHIP).ok_or("nothing by the pointer")?;
    assert_eq!(chip.label.as_deref(), Some("Carrying main.rs"));
    let [x, y, ..] = chip.rect;
    assert!(
        x > to.0 && x < to.0 + 30.0 && y > to.1 && y < to.1 + 30.0,
        "by the pointer: {:?} for {to:?}",
        chip.rect
    );
    let zone = by_key(&held, ZONE).ok_or("no preview")?.rect;
    assert!(near(zone, shell), "the terminal's slot: {zone:?}");
    assert_eq!(about(&runtime, "src/main.rs")?, None, "not before the drop");
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    let pinned = about(&runtime, "src/main.rs")?.ok_or("no pane about the file")?;
    assert!(near(frame(&runtime, pinned)?, shell), "where the shell was");
    assert_eq!(
        tabs(&runtime, pinned)?,
        [PaneKind::Terminal, PaneKind::Code],
        "a tab of that slot"
    );
    assert_eq!(focus(&runtime)?, Some(pinned), "with the keys");
    let after = runtime.elements();
    assert!(by_key(&after, CHIP).is_none() && by_key(&after, ZONE).is_none());
    tiles(&after)
}

/// Let go on the top edge of the chat the file takes the upper half of the chat's slot; on
/// the window's right edge another takes a column of its own.
#[test]
fn a_file_dragged_to_an_edge_takes_that_half_or_a_column_of_the_window() -> Check {
    let mut runtime = settled("shell/everything")?;
    let shown = runtime.elements();
    let chat = part(&shown, "chat", ".pane-frame")?;
    let to = (chat[0] + chat[2] / 2.0, chat[1] + 20.0);
    let from = centre(&runtime, MAIN_RS)?;
    carry(&mut runtime, from, to);
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    let pinned = about(&runtime, "src/main.rs")?.ok_or("no pane about the file")?;
    let upper = frame(&runtime, pinned)?;
    let lower = part(&runtime.elements(), "chat", ".pane-frame")?;
    assert!(
        (upper[1] - chat[1]).abs() < 0.6,
        "at the chat's top: {upper:?}"
    );
    assert!(
        (lower[1] - (upper[1] + upper[3])).abs() < 0.6,
        "chat under it"
    );
    assert_eq!(
        tabs(&runtime, pinned)?,
        [PaneKind::Code],
        "alone in its half"
    );
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, "shell.grid").ok_or("no grid")?.rect;
    let to = (x + w - 4.0, y + h / 2.0);
    let from = centre(&runtime, SESSIONS)?;
    carry(&mut runtime, from, to);
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    let column = about(&runtime, "src/routes/sessions.rs")?.ok_or("no second pane")?;
    let column = frame(&runtime, column)?;
    assert!((column[0] + column[2] - (x + w)).abs() < 0.6, "{column:?}");
    assert!((column[1] - y).abs() < 0.6 && (column[3] - h).abs() < 0.6);
    tiles(&runtime.elements())
}

/// A press that barely moves is a click and carries nothing. A file let go over the pane
/// it came from, or after Escape, opens nothing; a folder is not carried at all.
#[test]
fn a_click_escape_or_a_drop_on_its_own_pane_opens_nothing() -> Check {
    let mut runtime = settled("shell/everything")?;
    let shown = runtime.elements();
    let (files, shell) = (
        part(&shown, "files", ".pane-frame")?,
        part(&shown, "terminal", ".pane-frame")?,
    );
    let from = centre(&runtime, MAIN_RS)?;
    runtime.pointer_move(from.0, from.1);
    runtime.pointer_down(from.0, from.1);
    runtime.pointer_move(from.0 + 2.0, from.1 + 1.0);
    runtime.frame(1.0 / 60.0);
    assert!(by_key(&runtime.elements(), CHIP).is_none(), "a click");
    runtime.pointer_up(from.0 + 2.0, from.1 + 1.0);
    rest(&mut runtime);
    assert_eq!(about(&runtime, "src/main.rs")?, None);
    let home = (files[0] + files[2] / 2.0, files[1] + files[3] - 30.0);
    carry(&mut runtime, from, home);
    let held = runtime.elements();
    assert!(by_key(&held, CHIP).is_some(), "it is carried");
    assert!(by_key(&held, ZONE).is_none(), "its own pane is no place");
    runtime.pointer_up(home.0, home.1);
    rest(&mut runtime);
    assert_eq!(about(&runtime, "src/main.rs")?, None);
    let to = middle(shell);
    carry(&mut runtime, from, to);
    strike(&mut runtime, Key::Escape, Mods::default());
    assert!(
        by_key(&runtime.elements(), CHIP).is_none(),
        "Escape lets go"
    );
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    assert_eq!(about(&runtime, "src/main.rs")?, None, "and nothing opened");
    let folder = centre(&runtime, "files-1.row.src")?;
    carry(&mut runtime, folder, to);
    assert!(by_key(&runtime.elements(), CHIP).is_none(), "a folder");
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    Ok(())
}

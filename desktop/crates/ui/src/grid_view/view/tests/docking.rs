//! Docking by hand: a pane carried to an edge of another or of the window, the preview of
//! where it goes, a refusal, Escape, and the keys that move a pane.

use rune::{Mods, PhysicalInput, PhysicalKeyCode};
use yd_core::pane::PaneKind::{self, Changes, Chat, Code, Files, Terminal};

use super::{
    Check, DT, Key, Window, assert_at, assert_tiled, carry, find, frames, middle, part, press_key,
    rest, window,
};

/// A point on the bar of the pane of `kind`, clear of its tabs and icons.
fn bar_of(window: &Window, kind: PaneKind) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    let [x, y, w, _] = part(window, kind, "pane-tab-bar")?.rect;
    Ok((x + w - 60.0, y + 10.0))
}

fn shown(window: &Window, key: &str) -> bool {
    find(&window.elements(), key).is_ok()
}

/// The terminal carried to the top of chat: the preview shows the top of chat's slot and
/// nothing moves until the drop, which gives the terminal that half.
#[test]
fn a_pane_dropped_on_an_edge_of_another_splits_its_slot() -> Check {
    let (mut window, _) = window("review")?;
    let from = bar_of(&window, Terminal)?;
    carry(&mut window, from, (980.0, 60.0));
    rest(&mut window);
    let zone = find(&window.elements(), "grid.drop-zone")?.rect;
    assert!(
        super::near(zone, [734.0, 0.0, 300.0, 526.0]),
        "preview at {zone:?}"
    );
    assert_at(&window, Chat, [734.0, 0.0, 300.0, 676.0])?;
    window.pointer_up(980.0, 60.0);
    rest(&mut window);
    assert_at(&window, Terminal, [734.0, 0.0, 300.0, 526.0])?;
    assert_at(&window, Chat, [734.0, 526.0, 300.0, 150.0])?;
    assert_at(&window, Changes, [220.0, 0.0, 514.0, 676.0])?;
    assert!(
        !shown(&window, "grid.drop-zone"),
        "the preview goes with the drop"
    );
    assert_tiled(&window)
}

/// Files carried to the window's right edge becomes a full-height column there.
#[test]
fn a_pane_dropped_on_the_windows_edge_becomes_a_column() -> Check {
    let (mut window, _) = window("review")?;
    let from = bar_of(&window, Files)?;
    carry(&mut window, from, (1030.0, 338.0));
    rest(&mut window);
    let zone = find(&window.elements(), "grid.drop-zone")?.rect;
    assert!(
        super::near(zone, [734.0, 0.0, 300.0, 676.0]),
        "preview at {zone:?}"
    );
    window.pointer_up(1030.0, 338.0);
    rest(&mut window);
    assert_at(&window, Files, [734.0, 0.0, 300.0, 676.0])?;
    assert_at(&window, Chat, [434.0, 0.0, 300.0, 676.0])?;
    assert_tiled(&window)
}

/// Everything 704 wide has no room for a fifth column: the zone says so instead of a
/// preview, and the drop changes nothing.
#[test]
fn a_drop_with_no_room_says_so_and_changes_nothing() -> Check {
    let (mut window, _) = window("everything")?;
    window.set_viewport((704.0, 676.0), 1.0);
    rest(&mut window);
    let before = frames(&window);
    let [x, y, _, h] = super::frame(&window, Code)?;
    let to = (x + 4.0, y + h / 2.0);
    let from = bar_of(&window, Terminal)?;
    carry(&mut window, from, to);
    rest(&mut window);
    let refused = find(&window.elements(), "grid.drop-refused")?.clone();
    assert!(refused.rect[0] <= to.0 && to.0 <= refused.rect[0] + refused.rect[2]);
    assert!(
        !shown(&window, "grid.drop-zone"),
        "no target where it cannot go"
    );
    let said = window
        .elements()
        .into_iter()
        .any(|e| e.text.as_deref() == Some("Not enough room for a pane here"));
    assert!(said, "the zone says why");
    window.pointer_up(to.0, to.1);
    rest(&mut window);
    assert_eq!(frames(&window), before);
    Ok(())
}

/// Escape while a pane is carried over a zone puts it back: nothing changes.
#[test]
fn escape_during_a_carry_changes_nothing() -> Check {
    let (mut window, _) = window("review")?;
    let before = frames(&window);
    let from = bar_of(&window, Terminal)?;
    carry(&mut window, from, (980.0, 60.0));
    let mods = Mods::default();
    let (key, pressed, repeat) = (PhysicalKeyCode::Escape, true, false);
    window.physical_input(PhysicalInput::Key {
        key,
        mods,
        pressed,
        repeat,
    });
    window.frame(DT);
    window.pointer_up(980.0, 60.0);
    rest(&mut window);
    assert_eq!(frames(&window), before);
    Ok(())
}

/// The Move Pane keys: chat, in focus, goes onto the left edge of Changes, then back right.
#[test]
fn the_move_pane_keys_move_the_focused_pane_to_its_neighbours_edge() -> Check {
    let (mut window, _) = window("review")?;
    let at = middle(super::frame(&window, Chat)?);
    super::click(&mut window, at);
    let chord = Mods {
        cmd: true,
        shift: true,
        ctrl: true,
        ..Mods::default()
    };
    press_key(&mut window, Key::Left, chord);
    assert_at(&window, Chat, [220.0, 0.0, 407.0, 526.0])?;
    assert_at(&window, Changes, [627.0, 0.0, 407.0, 526.0])?;
    assert_tiled(&window)?;
    press_key(&mut window, Key::Right, chord);
    assert_at(&window, Changes, [220.0, 0.0, 407.0, 526.0])?;
    assert_at(&window, Chat, [627.0, 0.0, 407.0, 526.0])?;
    assert_tiled(&window)?;
    Ok(())
}

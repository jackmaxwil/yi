//! Docking in the real window: a pane carried by its bar to an edge of another pane and to
//! an edge of the window, through the root's own view, poll and keys.

use super::{Check, by_key, rest, settled};
use rune::{ElementInfo, Runtime};
use yd_ui::win::Win;

fn part(shown: &[ElementInfo], slug: &str, part: &str) -> Result<[f32; 4], String> {
    let found = shown.iter().find(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.starts_with(&format!("{slug}-")) && key.ends_with(part)
    });
    found.map(|e| e.rect).ok_or(format!("no {slug} {part}"))
}

/// Presses at `from`, moves to `to` in eight steps, and lets the preview settle.
fn carry(runtime: &mut Runtime<Win>, from: (f32, f32), to: (f32, f32)) {
    runtime.pointer_move(from.0, from.1);
    runtime.pointer_down(from.0, from.1);
    runtime.frame(1.0 / 60.0);
    for step in 1..=8u8 {
        let part = f32::from(step) / 8.0;
        let x = from.0 + (to.0 - from.0) * part;
        runtime.pointer_move(x, from.1 + (to.1 - from.1) * part);
        runtime.frame(1.0 / 60.0);
    }
    rest(runtime);
}

fn bar(shown: &[ElementInfo], slug: &str) -> Result<(f32, f32), String> {
    let [x, y, w, _] = part(shown, slug, ".pane-tab-bar")?;
    Ok((x + w - 60.0, y + 10.0))
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.6)
}

/// Everything's terminal carried to the top of chat: the preview is the top of chat's slot,
/// and the drop puts the terminal there with chat under it and the panes still tiling.
#[test]
fn a_pane_carried_to_the_top_of_chat_docks_above_it() -> Check {
    let mut runtime = settled("shell/everything")?;
    let shown = runtime.elements();
    let chat = part(&shown, "chat", ".pane-frame")?;
    let to = (chat[0] + chat[2] / 2.0, chat[1] + 20.0);
    carry(&mut runtime, bar(&shown, "terminal")?, to);
    let held = runtime.elements();
    let zone = by_key(&held, "grid.drop-zone").ok_or("no preview")?.rect;
    assert!(
        (zone[0] - chat[0]).abs() < 0.6 && (zone[1] - chat[1]).abs() < 0.6,
        "{zone:?}"
    );
    assert!(
        near(part(&held, "chat", ".pane-frame")?, chat),
        "chat moved before the drop"
    );
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    let after = runtime.elements();
    let terminal = part(&after, "terminal", ".pane-frame")?;
    let below = part(&after, "chat", ".pane-frame")?;
    assert!(
        near(terminal, zone),
        "the terminal is at {terminal:?}, not {zone:?}"
    );
    assert!(
        (below[1] - (terminal[1] + terminal[3])).abs() < 0.6,
        "chat is under it"
    );
    assert!(by_key(&after, "grid.drop-zone").is_none());
    tiles(&after)
}

/// Files carried to the window's right edge becomes the grid's last, full-height column.
#[test]
fn a_pane_carried_to_the_windows_edge_becomes_a_column_there() -> Check {
    let mut runtime = settled("shell/everything")?;
    let shown = runtime.elements();
    let [x, y, w, h] = by_key(&shown, "shell.grid").ok_or("no grid")?.rect;
    let to = (x + w - 4.0, y + h / 2.0);
    carry(&mut runtime, bar(&shown, "files")?, to);
    by_key(&runtime.elements(), "grid.drop-zone").ok_or("no preview")?;
    runtime.pointer_up(to.0, to.1);
    rest(&mut runtime);
    let after = runtime.elements();
    let files = part(&after, "files", ".pane-frame")?;
    assert!(
        (files[0] + files[2] - (x + w)).abs() < 0.6,
        "files ends at {files:?}"
    );
    assert!(
        (files[1] - y).abs() < 0.6 && (files[3] - h).abs() < 0.6,
        "full height"
    );
    tiles(&after)
}

fn tiles(shown: &[ElementInfo]) -> Check {
    let [x, y, w, h] = by_key(shown, "shell.grid").ok_or("no grid")?.rect;
    let frames = shown
        .iter()
        .filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.ends_with(".pane-frame")));
    let rects: Vec<[f32; 4]> = frames.map(|e| e.rect).collect();
    let covered: f32 = rects.iter().map(|r| r[2] * r[3]).sum();
    let inside = |r: &[f32; 4]| r[0] >= x - 0.5 && r[1] >= y - 0.5 && r[0] + r[2] <= x + w + 0.5;
    assert!(
        rects
            .iter()
            .all(|r| inside(r) && r[1] + r[3] <= y + h + 0.5),
        "{rects:?}"
    );
    assert!(
        (covered - w * h).abs() < 2.0,
        "the panes cover {covered} of {}",
        w * h
    );
    Ok(())
}

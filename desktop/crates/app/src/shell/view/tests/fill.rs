//! Filling the window with one pane, by every hand that can: the key, a double press on the
//! bar, a pinch, and a two-finger double tap. Each said again gives the grid back.

use rune::{GesturePhase, MagnifyEvent};

use super::*;

/// The live pane frames, by key.
fn frames(runtime: &Runtime<Win>) -> Vec<(String, [f32; 4])> {
    let live = runtime.elements().into_iter().filter(|e| !e.exiting);
    let frame = |e: ElementInfo| Some((e.key.filter(|k| k.ends_with(".pane-frame"))?, e.rect));
    live.filter_map(frame).collect()
}

fn rect(runtime: &Runtime<Win>, slug: &str, end: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let shown = runtime.elements();
    let found = shown.iter().find(|e| {
        let key = e.key.as_deref().unwrap_or_default();
        !e.exiting && key.starts_with(&format!("{slug}-")) && key.ends_with(end)
    });
    Ok(found.ok_or_else(|| format!("no {slug} {end}"))?.rect)
}

/// The one frame drawn is `slug`'s, over the room the whole grid had.
fn fills(runtime: &Runtime<Win>, slug: &str, grid: [f32; 4]) -> Check {
    let now = frames(runtime);
    let [(key, rect)] = &now[..] else {
        return Err(format!("{} frames drawn: {now:?}", now.len()).into());
    };
    assert!(
        key.starts_with(&format!("{slug}-")),
        "{key} fills, not {slug}"
    );
    let near = rect.iter().zip(grid).all(|(a, b)| (a - b).abs() < 0.6);
    assert!(near, "{rect:?} is not the grid {grid:?}");
    Ok(())
}

/// The room the grid's panes cover between them.
fn grid(frames: &[(String, [f32; 4])]) -> [f32; 4] {
    let low = |at: usize| frames.iter().map(|(_, r)| r[at]).fold(f32::MAX, f32::min);
    let high = |at: usize| {
        let far = frames.iter().map(|(_, r)| r[at] + r[at + 2]);
        far.fold(f32::MIN, f32::max)
    };
    [low(0), low(1), high(0) - low(0), high(1) - low(1)]
}

fn pinch(runtime: &mut Runtime<Win>, at: (f32, f32), deltas: &[f32]) {
    for (n, delta) in deltas.iter().enumerate() {
        let phase = match n {
            0 => GesturePhase::Began,
            n if n + 1 == deltas.len() => GesturePhase::Ended,
            _ => GesturePhase::Moved,
        };
        let (x, y, delta) = (at.0, at.1, *delta);
        runtime.pointer_move(x, y);
        let mut event = MagnifyEvent::default();
        (event.delta, event.x, event.y, event.phase) = (delta, x, y, phase);
        runtime.magnify_event(event);
        runtime.frame(1.0 / 60.0);
    }
    rest(runtime);
}

#[test]
fn the_key_and_a_double_press_on_the_bar_fill_the_window_and_give_it_back() -> Check {
    let mut runtime = settled("shell/everything")?;
    let before = frames(&runtime);
    let room = grid(&before);
    assert!(before.len() > 4, "Everything draws its panes");
    let shift = Mods {
        shift: true,
        ..cmd()
    };
    strike(&mut runtime, Key::Enter, shift);
    fills(&runtime, "chat", room)?;
    strike(&mut runtime, Key::Enter, shift);
    assert_eq!(frames(&runtime), before, "every pane is back where it was");
    let twice = |runtime: &mut Runtime<Win>, end: &str, along: f32| -> Check {
        let [x, y, w, h] = rect(runtime, "changes", end)?;
        let at = (x + w * along, y + h / 2.0);
        for _ in 0..2 {
            runtime.pointer_down(at.0, at.1);
            runtime.pointer_up(at.0, at.1);
            runtime.frame(1.0 / 60.0);
        }
        rest(runtime);
        Ok(())
    };
    // A tab is pressed twice to keep what it shows: that is no fill.
    twice(&mut runtime, ".pane-tab", 0.5)?;
    assert_eq!(frames(&runtime), before, "a double press on a tab");
    // The bare stretch of the bar, between its one tab and its buttons.
    twice(&mut runtime, ".pane-tab-bar", 0.55)?;
    fills(&runtime, "changes", room)?;
    twice(&mut runtime, ".pane-tab-bar", 0.55)?;
    assert_eq!(frames(&runtime), before);
    Ok(())
}

#[test]
fn a_pinch_out_fills_once_a_pinch_in_gives_back_and_a_double_tap_does_either() -> Check {
    let mut runtime = settled("shell/everything")?;
    let before = frames(&runtime);
    let room = grid(&before);
    let [x, y, w, h] = rect(&runtime, "changes", ".pane-frame")?;
    let at = (x + w / 2.0, y + h / 2.0);
    pinch(&mut runtime, at, &[0.05, 0.05]);
    assert_eq!(frames(&runtime), before, "a small pinch does nothing");
    // Fingers held apart go on reporting: the pane fills once and stays.
    pinch(&mut runtime, at, &[0.1, 0.15, 0.1, 0.3, 0.3, 0.0]);
    fills(&runtime, "changes", room)?;
    let middle = (room[0] + room[2] / 2.0, room[1] + room[3] / 2.0);
    pinch(&mut runtime, middle, &[-0.1, -0.2, -0.3, 0.0]);
    assert_eq!(frames(&runtime), before, "pinched in, the grid is back");
    runtime.pointer_move(at.0, at.1);
    runtime.smart_magnify(at.0, at.1);
    rest(&mut runtime);
    fills(&runtime, "changes", room)?;
    runtime.smart_magnify(middle.0, middle.1);
    rest(&mut runtime);
    assert_eq!(frames(&runtime), before);
    Ok(())
}

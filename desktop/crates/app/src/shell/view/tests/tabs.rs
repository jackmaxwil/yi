//! A bar's tabs under two fingers and the middle button: a sideways swipe turns them, a
//! middle press closes the one under it.

use rune::{GesturePhase, PointerButton, WheelEvent};
use yd_core::grid::step::GridMsg;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_ui::win::dispatch;

use super::fill::rect;
use super::*;

/// Everything with the debugger and IPython opened: Changes, the debugger and IPython are
/// the tabs of one slot, in that order, IPython in front.
fn stacked() -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut runtime = settled("shell/everything")?;
    for kind in [PaneKind::Debugger, PaneKind::IPython] {
        runtime.update(move |win, _| {
            let window = root::MAIN;
            dispatch(win, Msg::Grid(GridMsg::Open { window, kind }));
        });
        rest(&mut runtime);
    }
    Ok(runtime)
}

/// The slug of the pane drawn in the slot the three share.
fn front(runtime: &Runtime<Win>) -> Result<&'static str, Box<dyn Error>> {
    let shown = runtime.elements();
    let drawn = |slug: &&str| {
        let frame = |e: &&ElementInfo| {
            let key = e.key.as_deref().unwrap_or_default();
            !e.exiting && key.starts_with(&format!("{slug}-")) && key.ends_with(".pane-frame")
        };
        shown.iter().any(|e| frame(&e))
    };
    let found: Vec<&str> = ["changes", "debugger", "ipython"]
        .into_iter()
        .filter(drawn)
        .collect();
    match found[..] {
        [one] => Ok(one),
        _ => Err(format!("drawn in the slot: {found:?}").into()),
    }
}

/// The middle of the bar of the pane drawn in that slot.
fn bar(runtime: &Runtime<Win>) -> Result<(f32, f32), Box<dyn Error>> {
    let [x, y, w, h] = rect(runtime, front(runtime)?, ".pane-tab-bar")?;
    Ok((x + w * 0.6, y + h / 2.0))
}

/// One wheel event for each delta at `at`, the first a touch down, then the last lifting
/// when `lifts`. A touch down right after a lift is the coasting of the swipe before it.
fn wheel(runtime: &mut Runtime<Win>, at: (f32, f32), deltas: &[(f32, f32)], lifts: bool) {
    for (n, (dx, dy)) in deltas.iter().enumerate() {
        let phase = match n {
            0 => GesturePhase::Began,
            n if lifts && n + 1 == deltas.len() => GesturePhase::Ended,
            _ => GesturePhase::Moved,
        };
        runtime.pointer_move(at.0, at.1);
        runtime.wheel_event(WheelEvent::new(*dx, *dy).at(at.0, at.1).phase(phase));
        runtime.frame(1.0 / 60.0);
    }
}

#[test]
fn two_fingers_along_a_bar_turn_its_tabs_and_coasting_turns_none() -> Check {
    let mut runtime = stacked()?;
    assert_eq!(front(&runtime)?, "ipython", "opened last, it is in front");
    let swiped = |runtime: &mut Runtime<Win>, deltas: &[(f32, f32)]| -> Check {
        let at = bar(runtime)?;
        wheel(runtime, at, deltas, false);
        rest(runtime);
        Ok(())
    };
    swiped(&mut runtime, &[(-10.0, 0.0), (-10.0, 1.0)])?;
    assert_eq!(front(&runtime)?, "ipython", "a short swipe turns none");
    // Fingers going right bring the tab before, one for each stretch.
    swiped(&mut runtime, &[(20.0, 0.0), (20.0, 2.0), (20.0, 0.0)])?;
    assert_eq!(front(&runtime)?, "debugger");
    swiped(&mut runtime, &[(60.0, 0.0)])?;
    assert_eq!(front(&runtime)?, "changes");
    // A short swipe that lifts, and the long coast the system sends after it.
    let at = bar(&runtime)?;
    wheel(
        &mut runtime,
        at,
        &[(-5.0, 0.0), (-5.0, 0.0), (0.0, 0.0)],
        true,
    );
    wheel(
        &mut runtime,
        at,
        &[(-300.0, 0.0), (-300.0, 0.0), (-300.0, 0.0)],
        true,
    );
    rest(&mut runtime);
    assert_eq!(front(&runtime)?, "changes", "coasting turns none");
    swiped(&mut runtime, &[(60.0, -80.0), (60.0, -80.0)])?;
    assert_eq!(front(&runtime)?, "changes", "up and down is no swipe");
    // Fingers going left bring the next. Among stacked panes a swipe turns one tab however
    // long it is: the bar under the fingers is the next pane's from then on.
    swiped(&mut runtime, &[(-30.0, 0.0), (-30.0, 0.0)])?;
    assert_eq!(front(&runtime)?, "debugger");
    swiped(&mut runtime, &[(-50.0, 0.0), (-50.0, 0.0), (-50.0, 0.0)])?;
    assert_eq!(front(&runtime)?, "ipython");
    swiped(&mut runtime, &[(-60.0, 0.0)])?;
    assert_eq!(
        front(&runtime)?,
        "changes",
        "past the last, round to the first"
    );
    Ok(())
}

#[test]
fn a_middle_press_closes_the_tab_under_it_and_spares_one_with_no_cross() -> Check {
    let mut runtime = stacked()?;
    let middle = |runtime: &mut Runtime<Win>, end: &str| -> Check {
        let shown = runtime.elements();
        let found = shown.iter().find(|e| {
            let key = e.key.as_deref().unwrap_or_default();
            !e.exiting && key.ends_with(end)
        });
        let [x, y, w, h] = found.ok_or_else(|| format!("no element {end}"))?.rect;
        let at = (x + w / 2.0, y + h / 2.0);
        runtime.pointer_move(at.0, at.1);
        runtime.button_down(at.0, at.1, PointerButton::Middle);
        runtime.button_up(at.0, at.1, PointerButton::Middle);
        rest(runtime);
        Ok(())
    };
    let kinds = |runtime: &Runtime<Win>| -> Result<Vec<PaneKind>, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let layout = &grid.window(root::MAIN).ok_or("no window")?.layout;
        let held = grid.panes.iter().filter(|pane| layout.holds(pane.id));
        Ok(held.map(|pane| pane.kind).collect())
    };
    let before = kinds(&runtime)?;
    // The chat's one tab has no cross: a middle press leaves it.
    middle(&mut runtime, "chat-8.tab-0.pane-tab")?;
    assert_eq!(kinds(&runtime)?, before);
    // A tab that stands for another pane of the stack, behind the one in front.
    let behind = runtime.elements().into_iter().filter_map(|e| e.key);
    let mut behind = behind.filter(|key| key.contains(".tab-pane-") && key.ends_with(".pane-tab"));
    let tab = behind.next().ok_or("no tab for a pane behind")?;
    let id = tab
        .rsplit_once(".tab-pane-")
        .and_then(|(_, rest)| rest.split('.').next());
    let pane = yd_core::ids::PaneId(id.ok_or("no pane in the key")?.parse()?);
    let kind = {
        let hub = runtime.state.hub.try_borrow()?;
        hub.model.grid.kind(pane).ok_or("no such pane")?
    };
    middle(&mut runtime, &tab)?;
    let after = kinds(&runtime)?;
    assert!(!after.contains(&kind), "{kind:?} is closed: {after:?}");
    assert_eq!(after.len() + 1, before.len(), "and nothing else");
    assert_eq!(front(&runtime)?, "ipython", "the pane in front stays");
    Ok(())
}

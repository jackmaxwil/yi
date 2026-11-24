//! A window apart from the main one is a real Mac window: opaque, cycled with ⌘`, placed
//! where it was last and never shrunk below what its pane can draw.

use yd_core::grid::layout::MIN_PANE;

use super::*;

const SHOW: &str = r#"{"menu":"pane.show-settings","t":0.3}"#;
const END: &str = r#"{"t":2.0,"type":"mark","name":"end"}"#;

/// Incident: under the 1.25 zoom the window's glass covered 608 by 448 of its 760 by 560; the
/// rest was clear, so a click there went to the window behind and Settings looked a pop-up.
#[test]
fn a_panes_window_is_opaque_edge_to_edge() -> Result<(), Box<dyn Error>> {
    let (shared, _, _specs) = started(two)?;
    let mut runtime = window(&shared, OWN);
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    let shown = runtime.elements();
    let whole = |each: &&ElementInfo| {
        let [x, y, w, h] = each.rect;
        let covers = (x, y, w, h) == (0.0, 0.0, 312.0, 296.0);
        covers && each.key.as_deref() != Some(view::KEY) && each.fill.a >= 1.0
    };
    assert!(
        shown.iter().any(|each| whole(&each)),
        "nothing opaque under the pane but the root, whose fill the glass hides"
    );
    Ok(())
}

#[test]
fn command_backtick_brings_the_next_window_forward() -> Result<(), Box<dyn Error>> {
    let cycle = r#"{"t":1.0,"type":"key","name":"`","cmd":true,"code":"`"}"#;
    let events = format!("{SHOW},{cycle},{END}");
    let ran = desk("cycle", ONE, docked, &events)?;
    assert!(ran.window("main")?.focused, "the main window is in front");
    assert!(
        !ran.window("window-1")?.focused,
        "Settings handed it the keys"
    );
    Ok(())
}

#[test]
fn settings_closed_with_command_w_is_remembered_where_it_was() -> Result<(), Box<dyn Error>> {
    let close = r#"{"t":1.0,"type":"key","name":"w","cmd":true,"code":"w"}"#;
    let events = format!("{SHOW},{close},{END}");
    let ran = desk("settings-kept", ONE, docked, &events)?;
    let at = ran.window("window-1")?.frame;
    let hub = ran.shared.try_borrow()?;
    let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
    let places = registry.places.as_ref().ok_or("no places")?;
    let kept = places.read_apart(&registry.screens);
    let frame = Frame {
        x: at[0],
        y: at[1],
        w: at[2],
        h: at[3],
    };
    assert_eq!(kept, vec![("settings".to_owned(), frame)]);
    Ok(())
}

#[test]
fn settings_opens_where_it_was_last() -> Result<(), Box<dyn Error>> {
    let mut places = Places::memory();
    let there = Frame {
        x: 300.0,
        y: 60.0,
        w: 700.0,
        h: 500.0,
    };
    places.keep_apart(&vec![("settings".to_owned(), there)])?;
    let ran = desk_in(
        "settings-again",
        ONE,
        docked,
        &format!("{SHOW},{END}"),
        places,
    )?;
    let own = ran.window("window-1")?;
    assert_eq!(
        own.frame,
        place(there),
        "not in the middle of the app again"
    );
    Ok(())
}

#[test]
fn a_panes_window_is_never_smaller_than_its_pane_can_draw() {
    let zoomed = least(None, 1.25);
    assert_eq!(
        zoomed,
        (MIN_PANE.w * 1.25, MIN_PANE.h * 1.25),
        "the pane minimum, zoomed"
    );
    let settings = least(Some(PaneKind::Settings), 1.25);
    assert_eq!(
        settings,
        (MIN_WINDOW.w, MIN_WINDOW.h),
        "Settings is not crushed below the window minimum"
    );
}

/// Incident: ⌘, pressed in a pane's own window split that window for Settings, so closing it
/// later sent its pane home and left a stray window holding Settings' place.
#[test]
fn settings_asked_from_a_panes_window_leaves_the_app_window_not_that_one()
-> Result<(), Box<dyn Error>> {
    let focus = r#"{"t":0.3,"type":"tap","x":1000,"y":150}"#;
    let ask = r#"{"t":0.8,"type":"key","name":",","cmd":true,"code":","}"#;
    let ran = desk(
        "settings-from-pane",
        ONE,
        two,
        &format!("{focus},{ask},{END}"),
    )?;
    let hub = ran.shared.try_borrow()?;
    let grid = &hub.model.grid;
    let settings = grid.panes.iter().find(|p| p.kind == PaneKind::Settings);
    let settings = settings.ok_or("no Settings pane")?.id;
    assert_eq!(
        grid.origin_of(settings),
        Some(MAIN),
        "its place is kept in the app"
    );
    assert_eq!(
        lone(grid, OWN),
        Some(WEB_PANE),
        "the web pane's window is its alone"
    );
    Ok(())
}

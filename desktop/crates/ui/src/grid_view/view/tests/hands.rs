//! What a hand does to the grid: carrying a pane, dragging a line, the three pane icons,
//! tabs, and the layout keys and menu.

use super::{
    Act, Changes, Chat, Check, Code, Effect, FLIP_CHAT, Files, Gesture, Key, LIFT, MIN_PANE, Mods,
    Msg, PaneKind, Route, Run, SIZE, Terminal, WINDOW, WindowOp, act, assert_at, assert_tiled,
    carry, click, drag, frame, frames, front_of, key, middle, pane_of, part, press_key, read, rest,
    step, tabs_of, window,
};
use yd_core::grid::swap::Spot;

const CMD: Mods = Mods {
    cmd: true,
    shift: false,
    alt: false,
    ctrl: false,
    logo: false,
};

/// A point on the bar of the pane of `kind`, clear of its tabs and icons.
fn bar_of(
    window: &super::Window,
    kind: PaneKind,
) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    let [x, y, w, _] = part(window, kind, "pane-tab-bar")?.rect;
    Ok((x + w - 60.0, y + 10.0))
}

/// Record 2.7.9: the two panes exchange slots, and spans travel with the slot.
#[test]
fn a_pane_dropped_on_another_swaps_with_it_and_the_grid_still_tiles() -> Check {
    let (mut window, _) = window("review")?;
    let (from, to) = (bar_of(&window, Terminal)?, middle(frame(&window, Chat)?));
    drag(&mut window, from, to);
    assert_at(&window, Files, [0.0, 0.0, 220.0, 676.0])?;
    assert_at(&window, Changes, [220.0, 0.0, 514.0, 526.0])?;
    assert_at(&window, Chat, [220.0, 526.0, 514.0, 150.0])?;
    assert_at(&window, Terminal, [734.0, 0.0, 300.0, 676.0])?;
    assert_tiled(&window)?;
    // A pane with one tab is carried by that tab too.
    let (from, to) = (
        middle(part(&window, Changes, "tab-0.pane-tab")?.rect),
        middle(frame(&window, Files)?),
    );
    drag(&mut window, from, to);
    assert_at(&window, Changes, [0.0, 0.0, 220.0, 676.0])?;
    assert_at(&window, Files, [220.0, 0.0, 514.0, 526.0])?;
    assert_tiled(&window)
}

/// Record 2.11.1: a tab and a pane icon press in while the button is down and spring back.
#[test]
fn tabs_and_pane_icons_press_in_and_spring_back() -> Check {
    let (mut window, _) = window("everything")?;
    for name in ["tab-1.pane-tab", "pane-split"] {
        let at = middle(part(&window, Terminal, name)?.rect);
        window.pointer_move(at.0, at.1);
        window.pointer_down(at.0, at.1);
        for _ in 0..12 {
            window.frame(super::DT);
        }
        let pressed = part(&window, Terminal, name)?.scale;
        assert!(pressed < 0.97, "{name} is drawn at {pressed} while pressed");
        window.pointer_up(at.0, at.1);
        rest(&mut window);
        let name = if name == "pane-split" {
            "pane-out"
        } else {
            name
        };
        let after = part(&window, Terminal, name)?.scale;
        assert!((after - 1.0).abs() < 0.001, "{name} rests at {after}");
    }
    Ok(())
}

/// Record 2.11.2: the pane in the hand lifts and follows, the pane under the pointer is the
/// target, every other pane dims to .7; let go over nothing, it comes home.
#[test]
fn a_held_pane_lifts_and_follows_while_the_others_dim() -> Check {
    let (mut window, _) = window("review")?;
    let home = frame(&window, Terminal)?;
    let (from, to) = (bar_of(&window, Terminal)?, middle(frame(&window, Chat)?));
    carry(&mut window, from, to);
    rest(&mut window);
    let (terminal, chat) = (pane_of(&window, Terminal)?, pane_of(&window, Chat)?);
    let Gesture::Pane(held) = read(&window.state).gesture else {
        return Err("no pane is held".into());
    };
    let size = yd_core::grid::layout::Size {
        w: SIZE.0,
        h: SIZE.1,
    };
    let zone = held.zone(&*window.state.hub.try_borrow()?, super::WINDOW, size);
    let onto = zone.map(|zone| zone.spot);
    assert_eq!((held.pane, onto), (terminal, Some(Spot::Onto(chat))));
    let lifted = part(&window, Terminal, "pane-frame")?;
    assert!(
        (lifted.scale - LIFT).abs() < 0.001,
        "scale {}",
        lifted.scale
    );
    let moved = middle(lifted.rect);
    let want = (
        middle(home).0 + to.0 - from.0,
        middle(home).1 + to.1 - from.1,
    );
    assert!((moved.0 - want.0).abs() < 1.0 && (moved.1 - want.1).abs() < 1.0);
    assert!((part(&window, Files, "pane-frame")?.opacity - 0.7).abs() < 0.01);
    assert!((lifted.opacity - 1.0).abs() < 0.01);
    // Back over its own slot there is no target: released, it springs home.
    window.pointer_move(from.0, from.1);
    window.pointer_up(from.0, from.1);
    rest(&mut window);
    assert_at(&window, Terminal, home)?;
    assert!((part(&window, Files, "pane-frame")?.opacity - 1.0).abs() < 0.01);
    assert_tiled(&window)
}

/// Reduced motion turns the carry into a plain move: no lift, and the pane is where the
/// pointer is on the very next frame.
#[test]
fn reduced_motion_carries_a_pane_plainly() -> Check {
    let (mut window, _) = window("review")?;
    window.set_system_reduced_motion(true);
    rest(&mut window);
    let home = frame(&window, Terminal)?;
    let from = bar_of(&window, Terminal)?;
    let to = (from.0 + 90.0, from.1 - 120.0);
    carry(&mut window, from, to);
    window.frame(super::DT);
    let held = part(&window, Terminal, "pane-frame")?;
    assert!((held.scale - 1.0).abs() < 0.001, "scale {}", held.scale);
    assert!(
        (held.rect[0] - (home[0] + 90.0)).abs() < 0.5,
        "{:?}",
        held.rect
    );
    assert!(
        (held.rect[1] - (home[1] - 120.0)).abs() < 0.5,
        "{:?}",
        held.rect
    );
    Ok(())
}

/// Record 2.7.8 and plan default 6: the line follows the pointer, stops at the pane
/// minimum, and a double-click gives the two sides equal room.
#[test]
fn a_dragged_line_resizes_keeps_the_minimum_and_evens_on_a_double_click() -> Check {
    let (mut window, _) = window("review")?;
    // The line between Files (220 wide) and Changes.
    drag(&mut window, (220.0, 300.0), (300.0, 300.0));
    assert_at(&window, Files, [0.0, 0.0, 300.0, 676.0])?;
    assert_at(&window, Changes, [300.0, 0.0, 434.0, 526.0])?;
    assert_tiled(&window)?;
    drag(&mut window, (300.0, 300.0), (20.0, 300.0));
    let files = frame(&window, Files)?;
    assert!(
        (files[2] - MIN_PANE.w).abs() < 0.51,
        "Files is {} wide",
        files[2]
    );
    assert_tiled(&window)?;
    // The row line between Changes and the terminal under it.
    let line = frame(&window, Terminal)?[1];
    drag(&mut window, (500.0, line), (500.0, line - 100.0));
    let terminal = frame(&window, Terminal)?;
    assert!((terminal[1] - (line - 100.0)).abs() < 0.51, "{terminal:?}");
    assert_tiled(&window)?;
    let at = (500.0, terminal[1]);
    click(&mut window, at);
    click(&mut window, at);
    let (changes, terminal) = (frame(&window, Changes)?, frame(&window, Terminal)?);
    assert!(
        (changes[3] - terminal[3]).abs() < 0.51,
        "{changes:?} {terminal:?}"
    );
    assert_tiled(&window)
}

/// Record 2.7.8: the pointer over a line between panes shows the resize cursor.
#[test]
fn a_line_between_panes_shows_the_resize_cursor() -> Check {
    let (mut window, _) = window("review")?;
    window.pointer_move(220.0, 300.0);
    window.frame(super::DT);
    assert_eq!(window.cursor(), rune::Cursor::ResizeColumn);
    window.pointer_move(500.0, 526.0);
    window.frame(super::DT);
    assert_eq!(window.cursor(), rune::Cursor::ResizeRow);
    Ok(())
}

/// Plan defaults 4 and 5: split puts a second pane of the kind to the right, close removes
/// a pane and its neighbours fill, pop out asks for a window; never a hole.
#[test]
fn split_close_and_pop_out_never_leave_a_hole() -> Check {
    let (mut window, asked) = window("review")?;
    let at = middle(part(&window, Changes, "pane-split")?.rect);
    click(&mut window, at);
    assert_eq!(frames(&window).len(), 5, "split adds a pane");
    let changes = frame(&window, Changes)?;
    assert!(changes[2] < 514.0 && changes[0] == 220.0, "{changes:?}");
    assert_tiled(&window)?;
    let at = middle(part(&window, Terminal, "pane-close")?.rect);
    click(&mut window, at);
    assert_eq!(frames(&window).len(), 4, "close removes a pane");
    assert_tiled(&window)?;
    assert!(
        asked.borrow().is_empty(),
        "nothing asked of the windows yet"
    );
    let at = middle(part(&window, Files, "pane-out")?.rect);
    click(&mut window, at);
    assert_eq!(
        frames(&window).len(),
        3,
        "a pulled-out pane leaves the grid"
    );
    assert_tiled(&window)?;
    let opened = asked
        .borrow()
        .iter()
        .any(|effect| matches!(effect, Effect::Window(WindowOp::Open(id)) if *id != WINDOW));
    assert!(opened, "pop out asks the window layer for a window");
    Ok(())
}

/// Plan default 3: a click brings a tab in front; cmd+shift+[ and ] go round the tabs;
/// cmd+W closes the one in front, and the pane with its last tab.
#[test]
fn a_click_switches_tabs_and_keys_turn_and_close_them() -> Check {
    let (mut window, _) = window("everything")?;
    assert_eq!(front_of(&window, Terminal)?, "zsh");
    let at = middle(part(&window, Terminal, "tab-1.pane-tab")?.rect);
    click(&mut window, at);
    assert_eq!(front_of(&window, Terminal)?, "agent: cargo test · waiting");
    let shifted = Mods { shift: true, ..CMD };
    press_key(&mut window, Key::Command('['), shifted);
    assert_eq!(front_of(&window, Terminal)?, "zsh");
    press_key(&mut window, Key::Command('['), shifted);
    assert_eq!(front_of(&window, Terminal)?, "agent: cargo test · waiting");
    press_key(&mut window, Key::Command(']'), shifted);
    assert_eq!(front_of(&window, Terminal)?, "zsh");
    press_key(&mut window, Key::Command('w'), CMD);
    assert_eq!(tabs_of(&window, Terminal)?, ["agent: cargo test · waiting"]);
    assert_eq!(frames(&window).len(), 8);
    press_key(&mut window, Key::Command('w'), CMD);
    assert_eq!(frames(&window).len(), 7, "the pane goes with its last tab");
    assert_tiled(&window)
}

/// Plan default 3: a tab dragged along its strip changes places; dragged out of the strip
/// it becomes a pane the grid asks the window layer to open, and the grid stays whole.
#[test]
fn a_tab_dragged_along_reorders_and_dragged_out_asks_for_a_window() -> Check {
    let (mut window, asked) = window("everything")?;
    let first = middle(part(&window, Code, "tab-0.pane-tab")?.rect);
    let second = part(&window, Code, "tab-1.pane-tab")?.rect;
    drag(&mut window, first, (second[0] + second[2] - 4.0, first.1));
    assert_eq!(tabs_of(&window, Code)?, ["upload.rs", "rate_limit.rs"]);
    assert!(asked.borrow().is_empty());
    let grab = middle(part(&window, Code, "tab-0.pane-tab")?.rect);
    drag(&mut window, grab, (grab.0 + 10.0, grab.1 + 120.0));
    assert_eq!(tabs_of(&window, Code)?, ["upload.rs"]);
    let opened = asked
        .borrow()
        .iter()
        .any(|effect| matches!(effect, Effect::Window(WindowOp::Open(id)) if *id != WINDOW));
    assert!(opened, "the torn tab asks for a window");
    assert_eq!(frames(&window).len(), 8, "the grid closed up behind it");
    assert_tiled(&window)
}

/// Plan default 1: alt+cmd+1 to 5 show the five layouts, and the chat side flips the grid.
/// The Panes panel that also lists them is the window's, tested with the app.
#[test]
fn the_layout_keys_switch_layouts() -> Check {
    let (mut window, _) = window("everything")?;
    let at = middle(frame(&window, Files)?);
    click(&mut window, at);
    let chord = Mods { alt: true, ..CMD };
    press_key(&mut window, Key::Command('2'), chord);
    assert_at(&window, Chat, [734.0, 0.0, 300.0, 676.0])?;
    assert_eq!(frames(&window).len(), 4);
    press_key(&mut window, Key::Command('4'), chord);
    assert_at(&window, Code, [220.0, 0.0, 407.0, 506.0])?;
    window.update(|win, tx| act(win, tx, Act::FlipChat));
    rest(&mut window);
    assert_at(&window, Files, [814.0, 0.0, 220.0, 676.0])?;
    assert_tiled(&window)?;
    press_key(&mut window, Key::Command('5'), chord);
    assert_eq!(frames(&window).len(), 1);
    assert!(
        part(&window, Chat, "pane-tab-bar").is_err(),
        "just chat has no bar"
    );
    Ok(())
}

/// A layout that has chat in it puts the focus on chat again (record 2.7.5).
#[test]
fn showing_a_layout_with_chat_focuses_chat() -> Check {
    let (mut window, _) = window("everything")?;
    let at = middle(frame(&window, Files)?);
    click(&mut window, at);
    let files = pane_of(&window, Files)?;
    assert_eq!(read(&window.state).focus, Some(files));
    window.update(|win, tx| act(win, tx, Act::Show("review")));
    rest(&mut window);
    let chat = pane_of(&window, Chat)?;
    let hub = window.state.hub.try_borrow()?;
    assert_eq!(super::super::focused(&hub, WINDOW), Some(chat));
    Ok(())
}

/// The chat-side command the app's table already has still flips the grid the view draws.
#[test]
fn the_models_own_flip_command_mirrors_what_is_drawn() -> Check {
    let (mut window, _) = window("review")?;
    window.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            step(
                &mut hub.model,
                Msg::Command(Route::Model, Run::new(FLIP_CHAT)),
            );
        }
    });
    rest(&mut window);
    assert_at(&window, Chat, [0.0, 0.0, 300.0, 676.0])?;
    let _ = key(&window, Chat, "pane-frame")?;
    assert_tiled(&window)
}

/// Brief item 4: a carried pane tilts the way it is flung and trails the pointer on the
/// hand's own spring.
#[test]
fn a_flung_pane_tilts_its_way_and_lags_the_pointer() -> Check {
    for (kind, sideways) in [(Terminal, -300.0_f32), (Files, 300.0)] {
        let (mut window, _) = window("review")?;
        let from = bar_of(&window, kind)?;
        carry(&mut window, from, (from.0 + sideways, from.1));
        window.frame(super::DT);
        let pose = window
            .presented_pose(&key(&window, kind, "pane-frame")?)
            .ok_or("no pose")?;
        assert!(
            pose.rotation() * sideways > 0.0,
            "{kind:?} flung {sideways} turned {}",
            pose.rotation()
        );
        rest(&mut window);
        let before = middle(frame(&window, kind)?).0;
        window.pointer_move(from.0 + sideways + 100.0, from.1);
        for _ in 0..12 {
            window.frame(super::DT);
        }
        let moved = middle(frame(&window, kind)?).0 - before;
        let (want, _) = crate::grid_view::drag::HELD.sample(0.0, 0.0, 100.0, 0.1);
        assert!(
            (moved - want).abs() < want * 0.1,
            "{kind:?} trailed {moved} of 100 in 0.1 s, the hand's spring says {want}"
        );
    }
    Ok(())
}

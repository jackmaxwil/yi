//! What a person watching a remote screen would notice if it broke, drawn in process.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col};
use yd_core::command::table::Target;
use yd_core::command::tools::remote::{NO_HOST, SCREENSHOT};
use yd_core::feed::{Pointer, ScreenInput, SourceCmd};
use yd_core::ids::{PaneId, WindowId};
use yd_core::msg::Effect;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE};
use yd_ui::win::{Hub, Win};

use super::scenes::SCENES;
use super::{DEF, FEED, scale, thing_at};

type Check = Result<(), Box<dyn Error>>;
type Sent = Rc<RefCell<Vec<Effect>>>;

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

/// The scene `name`, drawn alone at `size` points.
fn draw(name: &str, size: (f32, f32)) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    let into = sent.clone();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.set_sink(move |effect| into.borrow_mut().push(effect));
        let scene = SCENES.iter().find(|scene| scene.name == name);
        (scene.ok_or("no such scene")?.setup)(&mut hub);
    }
    let win = Win::new(hub, WindowId(0));
    let mut rt = Runtime::new(win, root, size, Env::new(true, 1.0));
    for _ in 0..4 {
        rt.frame(0.016);
    }
    Ok((rt, sent))
}

fn rect(rt: &Runtime<Win>, key: &str) -> Result<[f32; 4], Box<dyn Error>> {
    let all = rt.elements();
    let hit = all
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key));
    Ok(hit.ok_or(format!("no {key}"))?.rect)
}

fn tap_at(rt: &mut Runtime<Win>, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(0.016);
    }
}

fn screen_asks(sent: &Sent) -> Vec<ScreenInput> {
    let all = sent.borrow();
    let asks = all.iter().filter_map(|effect| match effect {
        Effect::Source(feed, SourceCmd::Screen(input)) if *feed == FEED => Some(input.clone()),
        _ => None,
    });
    asks.collect()
}

#[test]
fn the_screen_is_fitted_by_the_records_rule_at_two_pane_sizes() -> Check {
    // Record 4.6: scale = min((w - 10) / 860, (h - 10) / 484). Frame 1, 286 by 148: about .285,
    // 245 by 138 points. Frame 23, 415 by 378: .4709, 405 by 228.
    for (size, body, want) in [
        ((286.0, 170.0), 148.0, (245.0, 138.0)),
        ((415.0, 400.0), 378.0, (405.0, 227.9)),
    ] {
        let (rt, _) = draw("desktop/mockup", size)?;
        let [_, _, w, h] = rect(&rt, "remote-0.screen")?;
        assert!(
            (w - want.0).abs() < 1.0 && (h - want.1).abs() < 1.0,
            "{size:?}: drew {w} by {h}, the record says {want:?}"
        );
        let held = rect(&rt, "remote-0.letterbox")?;
        assert!((held[3] - body).abs() < 1.0, "the body is {} high", held[3]);
    }
    assert_eq!(scale(20.0, 20.0), 0.08, "never below .08");
    Ok(())
}

#[test]
fn a_press_in_control_mode_reaches_the_screen_point_under_it_and_in_watch_mode_nothing() -> Check {
    let (mut rt, sent) = draw("desktop/control", (420.0, 300.0))?;
    let [x, y, w, h] = rect(&rt, "remote-0.surface")?;
    // A quarter across and three quarters down the 860 by 484 screen: pixel (215, 363).
    tap_at(&mut rt, (x + w * 0.25, y + h * 0.75));
    let pressed = screen_asks(&sent)
        .into_iter()
        .find_map(|input| match input {
            ScreenInput::Pointer {
                phase: Pointer::Press,
                x,
                y,
            } => Some((x, y)),
            _ => None,
        });
    let (px, py) = pressed.ok_or("no press reached the source")?;
    assert!(
        px.abs_diff(215) <= 1 && py.abs_diff(363) <= 1,
        "({px}, {py})"
    );

    let (mut rt, sent) = draw("desktop/mockup", (420.0, 300.0))?;
    let [x, y, w, h] = rect(&rt, "remote-0.surface")?;
    tap_at(&mut rt, (x + w * 0.25, y + h * 0.75));
    assert_eq!(screen_asks(&sent), [], "Watch sends nothing");
    Ok(())
}

#[test]
fn windows_and_rows_of_the_screen_are_things_with_a_screen_address_and_a_time() -> Check {
    let (rt, _) = draw("desktop/mockup", (286.0, 170.0))?;
    let hub = rt.state.hub.try_borrow()?;
    let at = |key: &str| thing_at(&hub, PaneId(0), key);
    let terminal =
        at("remote-0.screen.window:terminal").ok_or("the Terminal window is not a thing")?;
    assert_eq!(terminal.label, "ci-ubuntu · Terminal window");
    assert_eq!(
        terminal.address.to_string(),
        "yi://screen/ci-ubuntu/terminal#t=17:58"
    );
    let row = at("remote-0.screen.item:detail-front").ok_or("the retry row is not a thing")?;
    assert_eq!(
        row.address.to_string(),
        "yi://screen/ci-ubuntu/atlas-uploader/detail-front#t=17:58"
    );
    assert_eq!(row.excerpt.as_deref(), Some("detail-front.png"));
    assert!(at("remote-0.surface").is_none(), "the picture is no window");
    assert!(
        at("remote-0.screen.line:1").is_none(),
        "a log line is no row"
    );
    drop(hub);
    for key in [
        "remote-0.screen.window:terminal",
        "remote-0.screen.item:detail-front",
    ] {
        rect(&rt, key)?;
    }
    Ok(())
}

#[test]
fn with_no_host_the_pane_says_so_and_draws_no_screen() -> Check {
    let (rt, _) = draw("desktop/none", (420.0, 300.0))?;
    let said = rt.elements();
    let words: Vec<_> = said.iter().filter_map(|e| e.text.as_deref()).collect();
    assert!(words.contains(&NO_HOST), "{words:?}");
    assert!(rect(&rt, "remote-0.screen").is_err(), "a screen is drawn");
    Ok(())
}

fn key(rt: &mut Runtime<Win>, key: rune::Key) {
    rt.key(rune::KeyEvent {
        key,
        mods: rune::Mods::default(),
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
}

/// Record 4.9.2: in Mark up a drag boxes a region and nothing reaches the far side; the note
/// typed beside it goes with the prompt, and the box stays while the region is attached.
#[test]
fn mark_up_boxes_a_dragged_region_and_attaches_it_with_its_typed_note() -> Check {
    let (mut rt, sent) = draw("desktop/mockup", (420.0, 300.0))?;
    let [x, y, w, h] = rect(&rt, "remote-0.mode.2")?;
    tap_at(&mut rt, (x + w / 2.0, y + h / 2.0));
    let said = rt.elements();
    let hint = said.iter().filter_map(|e| e.text.as_deref());
    assert!(
        hint.clone().any(|t| t == super::bar::MARK_UP_HINT),
        "no hint"
    );
    let before = sent.borrow().len();
    let [sx, sy, sw, sh] = rect(&rt, "remote-0.surface")?;
    let (from, to) = (
        (sx + sw * 0.25, sy + sh * 0.25),
        (sx + sw * 0.5, sy + sh * 0.5),
    );
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    rt.pointer_move(to.0, to.1);
    rt.pointer_up(to.0, to.1);
    for _ in 0..3 {
        rt.frame(0.016);
    }
    let boxed = rect(&rt, "remote-0.surface.mark.0")?;
    assert!((boxed[2] - sw * 0.25).abs() < 8.0, "{boxed:?} of {sw}");
    let [fx, fy, fw, fh] = rect(&rt, "remote-0.surface.mark.0.note.field")?;
    tap_at(&mut rt, (fx + fw / 2.0, fy + fh / 2.0));
    key(&mut rt, rune::Key::Text("Too dark here".to_owned()));
    key(&mut rt, rune::Key::Enter);
    assert_eq!(sent.borrow().len(), before, "mark-up reached the far side");
    {
        let hub = rt.state.hub.try_borrow()?;
        let thing = hub.model.attached.things.first().ok_or("not attached")?;
        let address = thing.address.to_string();
        assert!(
            address.starts_with("yi://screen/ci-ubuntu#215,121,"),
            "{address}"
        );
        assert_eq!(thing.excerpt.as_deref(), Some("Too dark here"));
    }
    let [x, y, w, h] = rect(&rt, "remote-0.mode.0")?;
    tap_at(&mut rt, (x + w / 2.0, y + h / 2.0));
    assert!(
        rect(&rt, "remote-0.surface.mark.0").is_ok(),
        "the box went with the mode"
    );
    Ok(())
}

#[test]
fn the_screenshot_button_attaches_the_screen_as_a_thing_once() -> Check {
    let (mut rt, _) = draw("desktop/mockup", (420.0, 300.0))?;
    for _ in 0..2 {
        rt.update(|win, tx| {
            let target = Target::Pane(SCENE_PANE);
            assert!(super::carry(win, tx, SCREENSHOT, &target));
        });
    }
    let hub = rt.state.hub.try_borrow()?;
    let attached: Vec<String> = hub
        .model
        .attached
        .things
        .iter()
        .map(|thing| thing.address.to_string())
        .collect();
    assert_eq!(attached, ["yi://screen/ci-ubuntu#t=17:58"]);
    Ok(())
}

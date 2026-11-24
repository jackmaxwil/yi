//! What a person at the grid would notice if it broke, on Rune's in-process runtime: where
//! the panes are, what a bar holds, and what a drag, a click and a key do to them.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Color, Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::grid::layout::MIN_PANE;
use yd_core::grid::saved::Slug;
use yd_core::grid::windows::WindowOp;
use yd_core::ids::PaneId;
use yd_core::msg::{Effect, Msg, step};
use yd_core::pane::PaneKind;
use yd_core::pane::PaneKind::{Changes, Chat, Code, Files, Remote, Report, Terminal, Web};

use super::scenes::{SCENES, WINDOW, show};
use super::{Act, act, grid, read};
use crate::chrome::tabs::Tabs;
use crate::grid_view::drag::{Gesture, HELD, LIFT, LOOSE};
use crate::pane::{PaneCx, PaneDef, placeholder};
use crate::theme::look::colour;
use crate::win::{Hub, Win};

mod docking;
mod hands;
mod own;

type Check = Result<(), Box<dyn Error>>;
type Window = Runtime<Win>;
/// Every effect a window's changes asked of the world.
type Asked = Rc<RefCell<Vec<Effect>>>;
/// Where the record measures a pane.
type Measured = (PaneKind, [f32; 4]);

/// The grid of the mockup's first frame, where record 2.7.4 measures every pane.
const SIZE: (f32, f32) = (1034.0, 676.0);
/// One frame of a 120 Hz display, in seconds.
const DT: f64 = 1.0 / 120.0;

fn body(pane: &PaneCx<'_>, _: &mut Cx<Win>) -> El {
    let kind = pane.model().grid.kind(pane.pane).unwrap_or(Chat);
    placeholder(pane, kind, "its own")
}

const fn def(kind: PaneKind) -> PaneDef {
    PaneDef {
        kind,
        view: body,
        scenes: &[],
        commands: &[],
        settings: &[],
        tabs: None,
        on_tab: None,
        finder: None,
    }
}

static DEFS: &[&PaneDef] = &[
    &def(Files),
    &def(Changes),
    &def(Terminal),
    &def(Web),
    &def(Remote),
    &def(Report),
    &def(Code),
    &def(Chat),
];

fn view(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    col()
        .key("root")
        .w_full()
        .h_full()
        .child(grid(win, &hub, cx))
}

/// Runs frames until nothing moves any more, or for two seconds.
fn rest(window: &mut Window) {
    for _ in 0..240 {
        if !window.frame(DT) {
            break;
        }
    }
}

/// A window showing a saved layout with the mockup's tabs, and every effect it asks for.
fn window(saved: Slug) -> Result<(Window, Asked), Box<dyn Error>> {
    window_of(DEFS, saved)
}

fn window_of(
    defs: &'static [&'static PaneDef],
    saved: Slug,
) -> Result<(Window, Asked), Box<dyn Error>> {
    let hub = Hub::new(defs, Vec::new());
    let asked = Rc::new(RefCell::new(Vec::new()));
    {
        let mut hub = hub.try_borrow_mut()?;
        show(&mut hub, saved);
        let log = Rc::clone(&asked);
        hub.set_sink(move |effect| log.borrow_mut().push(effect));
    }
    let mut window = Runtime::new(Win::new(hub, WINDOW), view, SIZE, Env::new(true, 1.0));
    rest(&mut window);
    Ok((window, asked))
}

fn pane_of(window: &Window, kind: PaneKind) -> Result<PaneId, Box<dyn Error>> {
    let hub = window.state.hub.try_borrow()?;
    let found = hub.model.grid.panes.iter().find(|pane| pane.kind == kind);
    Ok(found.ok_or_else(|| format!("no {kind:?} pane"))?.id)
}

/// The key of a part of the pane of `kind`: `files-1.pane-frame`.
fn key(window: &Window, kind: PaneKind, part: &str) -> Result<String, Box<dyn Error>> {
    Ok(format!(
        "{}-{}.{part}",
        kind.slug(),
        pane_of(window, kind)?.0
    ))
}

fn find<'a>(tree: &'a [ElementInfo], key: &str) -> Result<&'a ElementInfo, Box<dyn Error>> {
    let found = tree
        .iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key));
    found.ok_or_else(|| format!("no element keyed {key:?}").into())
}

fn part(window: &Window, kind: PaneKind, part: &str) -> Result<ElementInfo, Box<dyn Error>> {
    let wanted = key(window, kind, part)?;
    Ok(find(&window.elements(), &wanted)?.clone())
}

fn frame(window: &Window, kind: PaneKind) -> Result<[f32; 4], Box<dyn Error>> {
    Ok(part(window, kind, "pane-frame")?.rect)
}

fn near(got: [f32; 4], want: [f32; 4]) -> bool {
    got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.51)
}

fn assert_at(window: &Window, kind: PaneKind, want: [f32; 4]) -> Check {
    let got = frame(window, kind)?;
    if near(got, want) {
        return Ok(());
    }
    Err(format!("{kind:?} is at {got:?}, want {want:?}").into())
}

/// Every pane frame drawn, as rectangles.
fn frames(window: &Window) -> Vec<[f32; 4]> {
    let drawn = window.elements().into_iter().filter(|element| {
        let key = element.key.as_deref();
        !element.exiting && key.is_some_and(|key| key.ends_with(".pane-frame"))
    });
    drawn.map(|element| element.rect).collect()
}

/// No hole and no overlap: the frames cover the window exactly.
fn assert_tiled(window: &Window) -> Check {
    let all = frames(window);
    let area: f32 = all.iter().map(|[_, _, w, h]| w * h).sum();
    if (area - SIZE.0 * SIZE.1).abs() > 1.0 {
        return Err(format!("panes cover {area} of {}: {all:?}", SIZE.0 * SIZE.1).into());
    }
    for (index, a) in all.iter().enumerate() {
        for b in all.iter().skip(index.saturating_add(1)) {
            let apart = a[0] + a[2] <= b[0] + 0.01
                || b[0] + b[2] <= a[0] + 0.01
                || a[1] + a[3] <= b[1] + 0.01
                || b[1] + b[3] <= a[1] + 0.01;
            if !apart {
                return Err(format!("{a:?} overlaps {b:?}").into());
            }
        }
    }
    let inside = |[x, y, w, h]: &[f32; 4]| {
        *x >= -0.01 && *y >= -0.01 && x + w <= SIZE.0 + 0.01 && y + h <= SIZE.1 + 0.01
    };
    match all.iter().find(|rect| !inside(rect)) {
        Some(rect) => Err(format!("{rect:?} leaves the window").into()),
        None => Ok(()),
    }
}

fn middle([x, y, w, h]: [f32; 4]) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

fn click(window: &mut Window, (x, y): (f32, f32)) {
    window.pointer_move(x, y);
    window.pointer_down(x, y);
    window.frame(DT);
    window.pointer_up(x, y);
    rest(window);
}

/// Presses at `from`, moves to `to` in steps, and holds there with the button down.
fn carry(window: &mut Window, from: (f32, f32), to: (f32, f32)) {
    window.pointer_move(from.0, from.1);
    window.pointer_down(from.0, from.1);
    window.frame(DT);
    for step in 1..=8u8 {
        let part = f32::from(step) / 8.0;
        let (x, y) = (
            from.0 + (to.0 - from.0) * part,
            from.1 + (to.1 - from.1) * part,
        );
        window.pointer_move(x, y);
        window.frame(DT);
    }
}

fn drag(window: &mut Window, from: (f32, f32), to: (f32, f32)) {
    carry(window, from, to);
    window.pointer_up(to.0, to.1);
    rest(window);
}

fn press_key(window: &mut Window, key: Key, mods: Mods) {
    window.key(KeyEvent { key, mods });
    rest(window);
}

fn tabs_of(window: &Window, kind: PaneKind) -> Result<Vec<String>, Box<dyn Error>> {
    let pane = pane_of(window, kind)?;
    let hub = window.state.hub.try_borrow()?;
    let kept = hub
        .panes
        .get::<Tabs>(Some(pane))
        .ok_or("the pane keeps no tabs")?;
    Ok(kept.tabs.iter().map(|tab| tab.title.clone()).collect())
}

fn front_of(window: &Window, kind: PaneKind) -> Result<String, Box<dyn Error>> {
    let pane = pane_of(window, kind)?;
    let hub = window.state.hub.try_borrow()?;
    let kept = hub
        .panes
        .get::<Tabs>(Some(pane))
        .ok_or("the pane keeps no tabs")?;
    Ok(kept.active().ok_or("no tab in front")?.title.clone())
}

/// The fill a pane takes at glass strength `alpha`.
fn pane_fill(window: &Window, alpha: f32) -> Result<Color, Box<dyn Error>> {
    let hub = window.state.hub.try_borrow()?;
    Ok(colour(hub.model.tokens.pane_tint.alpha(alpha)))
}

/// Record 2.7.4, column "Pixel result".
#[test]
fn each_saved_layout_draws_its_panes_where_the_record_measures_them() -> Check {
    let record: [(Slug, &[Measured]); 5] = [
        (
            "everything",
            &[
                (Files, [0.0, 0.0, 208.0, 676.0]),
                (Changes, [208.0, 0.0, 270.0, 416.0]),
                (Terminal, [208.0, 416.0, 270.0, 260.0]),
                (Web, [478.0, 0.0, 286.0, 226.0]),
                (Remote, [478.0, 226.0, 286.0, 190.0]),
                (Report, [478.0, 416.0, 286.0, 260.0]),
                (Code, [764.0, 0.0, 270.0, 226.0]),
                (Chat, [764.0, 226.0, 270.0, 450.0]),
            ],
        ),
        (
            "review",
            &[
                (Files, [0.0, 0.0, 220.0, 676.0]),
                (Changes, [220.0, 0.0, 514.0, 526.0]),
                (Terminal, [220.0, 526.0, 514.0, 150.0]),
                (Chat, [734.0, 0.0, 300.0, 676.0]),
            ],
        ),
        (
            "screens",
            &[
                (Web, [0.0, 0.0, 404.0, 676.0]),
                (Remote, [404.0, 0.0, 340.0, 338.0]),
                (Report, [404.0, 338.0, 340.0, 338.0]),
                (Chat, [744.0, 0.0, 290.0, 676.0]),
            ],
        ),
        (
            "code",
            &[
                (Files, [0.0, 0.0, 220.0, 676.0]),
                (Code, [220.0, 0.0, 407.0, 506.0]),
                (Changes, [627.0, 0.0, 407.0, 506.0]),
                (Terminal, [220.0, 506.0, 814.0, 170.0]),
            ],
        ),
        ("just-chat", &[(Chat, [0.0, 0.0, 1034.0, 676.0])]),
    ];
    for (saved, panes) in record {
        let (window, _) = window(saved)?;
        assert_eq!(frames(&window).len(), panes.len(), "{saved}: panes drawn");
        for (kind, want) in panes {
            assert_at(&window, *kind, *want)?;
        }
        assert_tiled(&window)?;
    }
    Ok(())
}

/// Record 2.7.6: open file over chat on the far left, Files on the far right.
#[test]
fn chat_on_the_left_mirrors_the_whole_grid() -> Check {
    let (mut window, _) = window("everything")?;
    window.update(|win, tx| act(win, tx, Act::FlipChat));
    rest(&mut window);
    assert_at(&window, Code, [0.0, 0.0, 270.0, 226.0])?;
    assert_at(&window, Chat, [0.0, 226.0, 270.0, 450.0])?;
    assert_at(&window, Web, [270.0, 0.0, 286.0, 226.0])?;
    assert_at(&window, Changes, [556.0, 0.0, 270.0, 416.0])?;
    assert_at(&window, Files, [826.0, 0.0, 208.0, 676.0])?;
    assert_tiled(&window)
}

/// Record 2.6 and the mockup's measured sizes: a bar of 20 over the pane's width, tabs of
/// 19, icons of 11, the tab in front's meta between the tabs and the three icons.
#[test]
fn a_bar_holds_tabs_meta_and_three_icons_at_the_record_sizes() -> Check {
    let (window, _) = window("everything")?;
    let bar = part(&window, Changes, "pane-tab-bar")?.rect;
    assert!(near(bar, [208.0, 0.0, 270.0, 20.0]), "bar at {bar:?}");
    let tab = part(&window, Changes, "tab-0.pane-tab")?.rect;
    assert!((tab[3] - 19.0).abs() < 0.01, "tab is {} tall", tab[3]);
    assert!((tab[0] - 208.0).abs() < 0.01 && (tab[1]).abs() < 0.01);
    let tree = window.elements();
    let words = |text: &str| {
        let shown = |element: &&ElementInfo| {
            let [x, y, ..] = element.rect;
            element.text.as_deref() == Some(text) && x >= bar[0] && x < bar[0] + bar[2] && y < 20.0
        };
        let found = tree.iter().find(shown);
        found
            .map(|element| element.rect)
            .ok_or(format!("no {text:?} in the bar"))
    };
    let (title, count, meta) = (words("Changes")?, words("2")?, words("2 files · +54 −1")?);
    assert!(title[0] < count[0], "the count follows the title");
    assert!(
        count[0] + count[2] <= tab[0] + tab[2],
        "the count is inside the tab"
    );
    let icons = ["pane-split", "pane-out", "pane-close"].map(|name| part(&window, Changes, name));
    let [split, out, close] = icons.map(|icon| icon.map(|icon| icon.rect));
    let (split, out, close) = (split?, out?, close?);
    for icon in [split, out, close] {
        assert!((icon[2] - 11.0).abs() < 0.01 && (icon[3] - 11.0).abs() < 0.01);
    }
    assert!(meta[0] + meta[2] <= split[0], "meta stops before the icons");
    assert!(split[0] < out[0] && out[0] < close[0], "split, out, close");
    assert!((close[0] + close[2] + 5.0 - (bar[0] + bar[2])).abs() < 0.01);
    assert!((out[0] - (split[0] + split[2]) - 6.0).abs() < 0.01);
    // The terminal's second tab is behind: only the tab in front has the fill.
    let front = part(&window, Terminal, "tab-0.pane-tab")?;
    let behind = part(&window, Terminal, "tab-1.pane-tab")?;
    assert_ne!(front.fill, Color::TRANSPARENT);
    assert_eq!(behind.fill.a, 0.0, "a tab behind has no fill");
    Ok(())
}

/// Record 2.7.7: just chat is one pane with no tab bar and no surface of its own.
#[test]
fn just_chat_is_one_pane_with_no_bar() -> Check {
    let (window, _) = window("just-chat")?;
    let tree = window.elements();
    let bars = tree.iter().filter(|element| {
        let key = element.key.as_deref();
        key.is_some_and(|key| key.ends_with(".pane-tab-bar"))
    });
    assert_eq!(bars.count(), 0);
    assert_eq!(part(&window, Chat, "pane-frame")?.fill.a, 0.0);
    Ok(())
}

/// Record 2.6: chat is in focus at the start; a click in another pane moves the darker fill.
#[test]
fn a_click_in_a_pane_gives_it_the_darker_fill() -> Check {
    let (mut window, _) = window("review")?;
    let (dark, light) = (pane_fill(&window, 0.3)?, pane_fill(&window, 0.2)?);
    assert_eq!(part(&window, Chat, "pane-frame")?.fill, dark);
    assert_eq!(part(&window, Files, "pane-frame")?.fill, light);
    let at = middle(frame(&window, Files)?);
    click(&mut window, at);
    assert_eq!(part(&window, Files, "pane-frame")?.fill, dark);
    assert_eq!(part(&window, Chat, "pane-frame")?.fill, light);
    Ok(())
}

/// Record 2.11.2 says stiffness 210 and damping 22 in the hand, 150 and 15 let go.
#[test]
fn the_springs_are_the_ones_the_record_gives() {
    let close = |got: f32, want: f32| (got - want).abs() < want * 0.001;
    assert!(close(HELD.stiffness(), 210.0) && close(HELD.damping(), 22.0));
    assert!(close(LOOSE.stiffness(), 150.0) && close(LOOSE.damping(), 15.0));
}

/// Every scene the grid registers draws its window whole; the posed one holds the terminal
/// 150 right and 150 up of its slot, lifted and leaning (record 2.11.2).
#[test]
fn every_grid_scene_draws_its_window_whole() -> Check {
    for scene in SCENES {
        let hub = Hub::new(DEFS, Vec::new());
        (scene.setup)(&mut *hub.try_borrow_mut()?);
        let mut window = Runtime::new(Win::new(hub, WINDOW), view, SIZE, Env::new(true, 1.0));
        rest(&mut window);
        if scene.name == "grid/drop-edge" {
            let shown = find(&window.elements(), "grid.drop-zone")?.rect;
            assert!(
                near(shown, [734.0, 0.0, 300.0, 526.0]),
                "the zone is at {shown:?}"
            );
            continue;
        }
        if scene.name != "grid/dragging" {
            assert_tiled(&window).map_err(|error| format!("{}: {error}", scene.name))?;
            continue;
        }
        let held = part(&window, Terminal, "pane-frame")?;
        let (x, y) = middle(held.rect);
        let home = middle([220.0, 526.0, 514.0, 150.0]);
        assert!((x - home.0 - 150.0).abs() < 0.5 && (y - home.1 + 150.0).abs() < 0.5);
        assert!((held.scale - LIFT).abs() < 0.001);
        assert!(
            held.rect[3] > 150.0 * LIFT + 1.0,
            "a leaning pane's box is taller"
        );
        assert!(matches!(read(&window.state).gesture, Gesture::Pane(_)));
    }
    Ok(())
}

/// Dogfood: a fresh window shows the chat alone, and Files, Changes, Code and the Terminal had
/// no command that opened them, so ⌘K answered "Nothing matches" for "Show Files".
#[test]
fn every_pane_has_a_command_that_shows_it() {
    let missing: Vec<&str> = PaneKind::ALL
        .iter()
        .filter(|kind| super::picker::show(**kind).is_none())
        .map(|kind| kind.title())
        .collect();
    assert_eq!(missing, Vec::<&str>::new(), "no command shows these panes");
}

//! The window layer on Rune's virtual desktop, every window of a run driven by a script,
//! and in process where no window has to be real.

use std::cell::RefCell;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use rune::tooling::WindowFrame;
use rune::{ElementInfo, Env, RunOptions, Runtime};
use yd_core::grid::layout::MIN_WINDOW;
use yd_core::grid::windows::WindowOp;
use yd_core::ids::TabId;
use yd_core::msg::Effect;
use yd_core::pane::PaneKind;
use yd_core::wire::replay::play;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::grid_view::view::{Act, act};

use super::scenes::{HOME, REMOTE, WEB, docked, moved, three, two};
use super::*;

/// Rune keeps the headless run in one place for the process: one desktop at a time.
static DESK: Mutex<()> = Mutex::new(());
/// A flat two-by-two picture, stretched over the desktop behind the windows.
const BACKDROP: [u8; 73] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0,
    0, 0, 253, 212, 154, 115, 0, 0, 0, 16, 73, 68, 65, 84, 120, 218, 99, 80, 211, 113, 2, 34, 6, 8,
    5, 0, 15, 206, 2, 81, 12, 90, 156, 176, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
/// The mockup's desktop: one screen of 1200 by 750 points at 2x.
const ONE: &str = r#"{"id":1,"frame":[0,0,1200,750],"scale":2,"primary":true}"#;
/// The same with a second screen of 1200 by 750 at 1x on its right.
const TWO: &str = r#"{"id":1,"frame":[0,0,1200,750],"scale":2,"primary":true},
    {"id":2,"frame":[1200,0,1200,750],"scale":1}"#;
/// A real server's list of six threads; five are known only from the list.
const LISTED: &str = include_str!("../../../../../fixtures/wire/lists.jsonl");
/// The web app's pane in Everything, and the window the grid makes for the first pane out.
const WEB_PANE: PaneId = PaneId(4);
const OUT: &str = r#"{"type":"tap","key":"web-4.pane-out""#;
const OWN: WindowId = WindowId(1);

type Asked = Rc<RefCell<Vec<Effect>>>;

mod apart;
mod restart;

/// A hub whose window effects reach the window layer and whose every effect is kept.
fn hub(setup: fn(&mut Hub)) -> Result<(Shared, Asked), Box<dyn Error>> {
    let shared = root::init().hub;
    let asked = Asked::default();
    {
        let mut hub = shared.try_borrow_mut()?;
        setup(&mut hub);
        let (log, windows) = (Rc::clone(&asked), Shared::clone(&shared));
        hub.set_sink(move |effect| {
            if matches!(effect, Effect::Window(_)) {
                reconcile(&windows);
            }
            log.borrow_mut().push(effect);
        });
    }
    Ok((shared, asked))
}

/// What a run on the virtual desktop left behind.
struct Ran {
    shared: Shared,
    asked: Asked,
    frames: Vec<WindowFrame>,
}

impl Ran {
    /// The window as the run ended: its frame, and whether it showed and had the keys.
    fn window(&self, key: &str) -> Result<&WindowFrame, Box<dyn Error>> {
        let last = self.frames.iter().rev().find(|frame| frame.key == key);
        Ok(last.ok_or_else(|| format!("no window {key}"))?)
    }

    /// How many views have the thread the main window shows open on the wire.
    fn views(&self) -> Result<u32, Box<dyn Error>> {
        let hub = self.shared.try_borrow()?;
        Ok(shown::thread(&hub, MAIN).ok_or("no thread shows")?.views)
    }

    /// The main window drawn again over the hub as the run left it.
    fn main(&self) -> Vec<ElementInfo> {
        let win = Win::new(Shared::clone(&self.shared), MAIN);
        let mut runtime = Runtime::new(win, root::view, (HOME.w, HOME.h), Env::new(true, 1.0));
        runtime.frame(1.0 / 60.0);
        runtime.frame(1.0 / 60.0);
        runtime.elements()
    }
}

/// Runs the app's windows headless over `setup`'s state, plays `events`, and writes the
/// desktop as it ended to `target/scenes/windows/<name>.png`.
fn desk(
    name: &str,
    displays: &str,
    setup: fn(&mut Hub),
    events: &str,
) -> Result<Ran, Box<dyn Error>> {
    desk_in(name, displays, setup, events, Places::memory())
}

/// [`desk`] over the places a run kept in `places`.
fn desk_in(
    name: &str,
    displays: &str,
    setup: fn(&mut Hub),
    events: &str,
    places: Places,
) -> Result<Ran, Box<dyn Error>> {
    let _one = DESK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/scenes/windows");
    fs::create_dir_all(&out)?;
    let (backdrop, script) = (out.join("backdrop.png"), out.join(format!("{name}.json")));
    fs::write(&backdrop, BACKDROP)?;
    fs::write(
        &script,
        format!(r#"{{"displays":[{displays}],"events":[{events}]}}"#),
    )?;
    let (shared, asked) = hub(setup)?;
    let text = |path: PathBuf| path.to_string_lossy().into_owned();
    let wide = if displays == TWO {
        "2400x750"
    } else {
        "1200x750"
    };
    let args = [
        "--desktop".to_owned(),
        text(backdrop),
        "--script".to_owned(),
        text(script),
        "--size".to_owned(),
        wide.to_owned(),
        "--snap".to_owned(),
        text(out.join(format!("{name}.png"))),
    ];
    let windows = Shared::clone(&shared);
    rune::try_run_windows_with_args(RunOptions::default(), args, move |displays| {
        let launch = Launch {
            headless: true,
            places,
        };
        specs(&windows, displays, launch)
    })?;
    let run = rune::tooling::headless_run().ok_or("no headless run")?;
    Ok(Ran {
        shared,
        asked,
        frames: run.window_frames(),
    })
}

fn keyed<'a>(shown: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    shown.iter().find(|each| each.key.as_deref() == Some(key))
}

/// Fails unless the panes cover the grid exactly: no hole and nothing twice.
fn assert_tiled(shown: &[ElementInfo]) -> Result<(), Box<dyn Error>> {
    let grid = keyed(shown, "grid").ok_or("no grid")?.rect;
    let frame = |each: &&ElementInfo| {
        let key = each.key.as_deref();
        key.is_some_and(|key| key.ends_with(".pane-frame"))
    };
    let panes: Vec<[f32; 4]> = shown.iter().filter(frame).map(|each| each.rect).collect();
    let covered: f32 = panes.iter().map(|rect| rect[2] * rect[3]).sum();
    assert!(
        (covered - grid[2] * grid[3]).abs() < 1.0,
        "panes cover {covered} of {}",
        grid[2] * grid[3]
    );
    for (index, a) in panes.iter().enumerate() {
        let inside = a[0] >= grid[0] - 0.5
            && a[1] >= grid[1] - 0.5
            && a[0] + a[2] <= grid[0] + grid[2] + 0.5
            && a[1] + a[3] <= grid[1] + grid[3] + 0.5;
        assert!(inside, "{a:?} leaves the grid {grid:?}");
        for b in panes.iter().skip(index.saturating_add(1)) {
            let apart = a[0] + a[2] <= b[0] + 0.5
                || b[0] + b[2] <= a[0] + 0.5
                || a[1] + a[3] <= b[1] + 0.5
                || b[1] + b[3] <= a[1] + 0.5;
            assert!(apart, "{a:?} and {b:?} overlap");
        }
    }
    Ok(())
}

fn place(frame: Frame) -> [f32; 4] {
    [frame.x, frame.y, frame.w, frame.h]
}

#[test]
fn the_pull_out_button_gives_the_pane_a_window_and_the_grid_closes_up() -> Result<(), Box<dyn Error>>
{
    let events = format!(r#"{OUT},"t":0.3}},{{"t":1.0,"type":"mark","name":"end"}}"#);
    let ran = desk("pulled-out", ONE, docked, &events)?;
    let own = ran.window("window-1")?;
    assert!(
        own.visible && own.focused,
        "it opens in front, with the keys"
    );
    // Record 2.8.1: the slot it had in the app, moved 18 right and 14 up, 4 taller.
    let size = yd_core::grid::layout::Size {
        w: HOME.w,
        h: HOME.h,
    };
    let (shared, _) = hub(docked)?;
    let before = shared.try_borrow()?.model.grid.place(MAIN, size);
    let (_, slot) = before
        .iter()
        .find(|(pane, _)| *pane == WEB_PANE)
        .ok_or("no slot")?;
    let off_slot = [
        HOME.x + slot.x + 18.0,
        HOME.y + slot.y - 14.0,
        slot.w.max(MIN_WINDOW.w),
        (slot.h + 4.0).max(MIN_WINDOW.h),
    ];
    assert_eq!(
        own.frame, off_slot,
        "off its slot, never under the window minimum"
    );
    assert!(
        !ran.window("window-2")?.visible,
        "one pane left, one window"
    );
    let opened = Effect::Window(WindowOp::Open(OWN));
    assert!(
        ran.asked.borrow().contains(&opened),
        "the grid asked for it"
    );
    let main = ran.main();
    assert!(
        keyed(&main, "web-4.pane-frame").is_none(),
        "it left the app"
    );
    assert_tiled(&main)?;
    assert_eq!(
        ran.views()?,
        3,
        "the app and the pane's window both show it, and the bell holds it for its ask"
    );
    Ok(())
}

#[test]
fn the_put_back_button_closes_the_window_and_the_pane_is_home() -> Result<(), Box<dyn Error>> {
    let events =
        format!(r#"{OUT},"t":0.3}},{OUT},"t":1.0}},{{"t":1.6,"type":"mark","name":"end"}}"#);
    let ran = desk("put-back", ONE, docked, &events)?;
    assert!(!ran.window("window-1")?.visible, "its window is gone");
    let closed = Effect::Window(WindowOp::Close(OWN));
    assert!(ran.asked.borrow().contains(&closed), "the grid let it go");
    let main = ran.main();
    assert!(
        keyed(&main, "web-4.pane-frame").is_some(),
        "the pane is home"
    );
    assert_tiled(&main)?;
    assert_eq!(
        ran.views()?,
        2,
        "the window's view of the thread closed with it: the app's and the bell's stay"
    );
    Ok(())
}

#[test]
fn a_pane_pulled_out_a_second_time_opens_its_thread_again() -> Result<(), Box<dyn Error>> {
    let events = format!(
        r#"{OUT},"t":0.3}},{OUT},"t":1.0}},{OUT},"t":1.7}},{{"t":2.4,"type":"mark","name":"end"}}"#
    );
    let ran = desk("out-again", ONE, docked, &events)?;
    assert!(ran.window("window-1")?.visible);
    assert_eq!(
        ran.views()?,
        3,
        "the app's, the new window's and the bell's"
    );
    Ok(())
}

#[test]
fn one_pane_out_is_two_windows_and_two_out_are_three() -> Result<(), Box<dyn Error>> {
    let end = r#"{"t":0.5,"type":"mark","name":"end"}"#;
    let ran = desk("two", ONE, two, end)?;
    assert_eq!(ran.window("main")?.frame, place(HOME));
    let web = ran.window("window-1")?;
    assert!(web.visible, "the web app's own window");
    assert_eq!(web.frame, place(WEB), "where the scene put it");
    assert!(!ran.window("window-2")?.visible);
    let main = ran.main();
    assert!(keyed(&main, "web-4.pane-frame").is_none());
    assert_tiled(&main)?;

    let ran = desk("three", ONE, three, end)?;
    assert_eq!(ran.window("window-1")?.frame, place(WEB));
    let remote = ran.window("window-2")?;
    assert!(remote.visible, "the remote desktop's own window");
    assert_eq!(remote.frame, place(REMOTE));
    assert_tiled(&ran.main())
}

/// Frame 3 of the mockup: the app on the laptop, four panes on the second screen.
fn second_screen(hub: &mut Hub) {
    three(hub);
    let terminal = hub.model.grid.panes.iter();
    let terminal = terminal
        .clone()
        .find(|pane| pane.kind == PaneKind::Terminal);
    if let Some(pane) = terminal.map(|pane| pane.id) {
        yd_core::msg::step(&mut hub.model, Msg::Grid(GridMsg::Leave(pane)));
    }
    let places = [
        (WindowId(1), [1236.0, 40.0, 650.0, 436.0]),
        (WindowId(2), [1906.0, 40.0, 424.0, 292.0]),
        (WindowId(3), [1236.0, 492.0, 650.0, 196.0]),
    ];
    for (window, [x, y, w, h]) in places {
        moved(hub, window, Frame { x, y, w, h });
    }
}

#[test]
fn windows_sit_on_a_second_screen() -> Result<(), Box<dyn Error>> {
    let end = r#"{"t":0.5,"type":"mark","name":"end"}"#;
    let ran = desk("two-screens", TWO, second_screen, end)?;
    assert_eq!(
        ran.window("main")?.frame,
        place(HOME),
        "the app on the laptop"
    );
    for (key, at) in [
        ("window-1", [1236.0, 40.0, 650.0, 436.0]),
        ("window-2", [1906.0, 40.0, 424.0, 292.0]),
        ("window-3", [1236.0, 492.0, 650.0, 196.0]),
    ] {
        let window = ran.window(key)?;
        assert!(window.visible, "{key} shows");
        assert_eq!(window.frame, at, "{key} is on the second screen");
    }
    assert_tiled(&ran.main())
}

/// A display Rune would report: `size` pixels at `position`, `scale` pixels a point.
fn display(position: (i32, i32), size: (u32, u32), scale: f64, primary: bool) -> Display {
    Display {
        id: None,
        name: None,
        position,
        size,
        scale,
        primary,
    }
}

/// The windows of a run that is never started: the registry and the hooks with no window
/// behind them.
fn started(setup: fn(&mut Hub)) -> Result<(Shared, Asked, Vec<AnyWindowSpec>), Box<dyn Error>> {
    let (shared, asked) = hub(setup)?;
    let launch = Launch {
        headless: false,
        places: Places::memory(),
    };
    let laptop = display((0, 0), (2400, 1500), 2.0, true);
    let specs = specs(&shared, &[laptop], launch);
    Ok((shared, asked, specs))
}

/// One window of a run that is never started, on Rune's in-process runtime.
fn window(shared: &Shared, id: WindowId) -> Runtime<Win> {
    let win = Win::new(Shared::clone(shared), id);
    let view = if id == MAIN { root::view } else { view::view };
    let mut runtime = Runtime::new(win, view, (312.0, 296.0), Env::new(true, 1.0));
    runtime.poll = Some(if id == MAIN { root::poll } else { poll_pane });
    runtime
}

fn listed(hub: &mut Hub) {
    let lines: Vec<&str> = LISTED.lines().take(29).collect();
    assert!(play(&mut hub.model, &lines.join("\n")).is_ok());
}

fn leave_web(shared: &Shared) -> Result<(), Box<dyn Error>> {
    let mut main = Win::new(Shared::clone(shared), MAIN);
    dispatch(&mut main, Msg::Grid(GridMsg::Leave(WEB_PANE)));
    let hub = shared.try_borrow()?;
    assert_eq!(hub.model.grid.window_of(WEB_PANE), Some(OWN));
    let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
    assert!(registry.shows(OWN), "the window layer opened it");
    Ok(())
}

#[test]
fn a_panes_window_draws_the_pane_alone_with_its_bar_clear_of_the_lights()
-> Result<(), Box<dyn Error>> {
    let (shared, _, _specs) = started(two)?;
    let mut runtime = window(&shared, OWN);
    let mut lights = rune::ChromeInsets::default();
    lights.leading = 60.0;
    runtime.set_chrome_insets(lights);
    runtime.frame(1.0 / 60.0);
    let shown = runtime.elements();
    assert!(
        keyed(&shown, "shell.top-bar").is_none(),
        "no shell of its own"
    );
    let bar = keyed(&shown, "web-4.pane-tab-bar")
        .ok_or("no tab bar")?
        .rect;
    assert_eq!(
        (bar[0], bar[1], bar[2], bar[3]),
        (0.0, 0.0, 312.0, TITLED),
        "the pane's bar is the window's title bar, edge to edge"
    );
    let gap = keyed(&shown, "web-4.pane-lights")
        .ok_or("no gap for the lights")?
        .rect;
    assert!(gap[2] >= 60.0, "room for the lights: {gap:?}");
    let tab = keyed(&shown, "web-4.tab-0.pane-tab").ok_or("no tab")?.rect;
    assert!(tab[0] >= 60.0, "the first tab starts after them: {tab:?}");
    let body = keyed(&shown, "web-4").ok_or("the pane is not drawn")?.rect;
    assert_eq!(
        (body[0], body[2]),
        (0.0, 312.0),
        "the pane fills the window"
    );
    assert!(
        keyed(&shown, "web-4.pane-out").is_some(),
        "the put-back button"
    );
    // Holding the bar's free part moves the window.
    runtime.pointer_move(30.0, 10.0);
    runtime.pointer_down(30.0, 10.0);
    runtime.frame(1.0 / 60.0);
    assert_eq!(
        runtime.take_titlebar_action(),
        Some(rune::TitlebarAction::Drag)
    );
    Ok(())
}

#[test]
fn a_panes_window_shows_the_thread_it_left_with() -> Result<(), Box<dyn Error>> {
    let (shared, _, _specs) = started(listed)?;
    let (first, other) = {
        let hub = shared.try_borrow()?;
        let mut roots = shown::roots(&hub.model.wire).into_iter();
        let first = roots.next().ok_or("no thread")?.id.clone();
        (first, roots.next().ok_or("one thread only")?.id.clone())
    };
    shown::pick(&mut *shared.try_borrow_mut()?, MAIN, other.clone());
    leave_web(&shared)?;
    shown::pick(&mut *shared.try_borrow_mut()?, MAIN, first.clone());
    let hub = shared.try_borrow()?;
    let of = |window| shown::thread(&hub, window).map(|thread| thread.id.clone());
    assert_eq!(of(MAIN), Some(first), "the app moved on");
    assert_eq!(
        of(OWN),
        Some(other),
        "the window kept the thread it left with"
    );
    Ok(())
}

#[test]
fn a_tab_torn_out_of_its_strip_gets_a_window_and_keeps_what_it_showed() -> Result<(), Box<dyn Error>>
{
    let (shared, _, _specs) = started(|_| {})?;
    let second = Tab {
        id: TabId(1),
        title: "Orders".to_owned(),
        ..Tab::of(PaneKind::Web)
    };
    {
        let mut hub = shared.try_borrow_mut()?;
        let tabs = hub.panes.get_mut::<Tabs>(Some(WEB_PANE)).ok_or("no tabs")?;
        tabs.tabs = vec![Tab::of(PaneKind::Web), second];
    }
    let mut main = window(&shared, MAIN);
    main.update(|win, tx| act(win, tx, Act::TearOut(WEB_PANE, TabId(1))));
    let hub = shared.try_borrow()?;
    let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
    assert!(registry.shows(OWN), "the torn tab has a window");
    let torn = lone(&hub.model.grid, OWN).ok_or("its window holds no one pane")?;
    assert_ne!(torn, WEB_PANE, "the pane it left stays in the app");
    let titles = |pane| {
        let kept = hub.panes.get::<Tabs>(Some(pane));
        let tabs = kept.map(|tabs| tabs.tabs.iter());
        let titles = tabs.into_iter().flatten().map(|tab| tab.title.clone());
        titles.collect::<Vec<_>>()
    };
    assert_eq!(titles(torn), ["Orders"], "the tab went with it");
    assert_eq!(titles(WEB_PANE), ["Web app"]);
    Ok(())
}

#[test]
fn the_window_in_front_points_the_file_git_and_language_sources_at_its_thread()
-> Result<(), Box<dyn Error>> {
    let (shared, asked, _specs) = started(listed)?;
    leave_web(&shared)?;
    let (other, folder) = {
        let hub = shared.try_borrow()?;
        let other = shown::roots(&hub.model.wire).into_iter().nth(1);
        let other = other.ok_or("one thread only")?;
        (other.id.clone(), PathBuf::from(shown::folder(other)))
    };
    shown::pick(&mut *shared.try_borrow_mut()?, MAIN, other);
    let mut main = window(&shared, MAIN);
    asked.borrow_mut().clear();
    main.update(|win, tx| front(win, false, tx));
    assert!(
        asked.borrow().is_empty(),
        "a window going behind points nothing"
    );
    main.update(|win, tx| front(win, true, tx));
    let roots: Vec<Effect> = [FILES, CHANGES, LSP]
        .into_iter()
        .map(|feed| Effect::Source(feed, SourceCmd::Root(folder.clone())))
        .collect();
    assert_eq!(*asked.borrow(), roots);
    Ok(())
}

fn views(shared: &Shared) -> Result<u32, Box<dyn Error>> {
    let hub = shared.try_borrow()?;
    Ok(shown::thread(&hub, OWN).ok_or("no thread shows")?.views)
}

#[test]
fn closing_a_panes_window_by_hand_sends_the_pane_home_and_closes_its_thread()
-> Result<(), Box<dyn Error>> {
    let (shared, _, _specs) = started(listed)?;
    leave_web(&shared)?;
    let mut own = window(&shared, OWN);
    own.frame(1.0 / 60.0);
    assert_eq!(views(&shared)?, 1, "the window opened the thread it shows");
    own.update(closed);
    let hub = shared.try_borrow()?;
    assert_eq!(hub.model.grid.window_of(WEB_PANE), Some(MAIN), "home again");
    assert_eq!(hub.model.grid.windows.len(), 1);
    let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
    assert!(!registry.shows(OWN));
    drop(hub);
    assert_eq!(views(&shared)?, 0, "and closed it on the wire");
    Ok(())
}

/// The threads waiting on an answer, and the cancelled answers sent so far.
fn waiting(shared: &Shared, asked: &Asked) -> Result<(usize, usize), Box<dyn Error>> {
    let hub = shared.try_borrow()?;
    let cancelled = |effect: &&Effect| match effect {
        Effect::Wire(request) => request.line.contains("cancelled"),
        _ => false,
    };
    let sent = asked.borrow().iter().filter(cancelled).count();
    Ok((hub.model.wire.asks.len(), sent))
}

#[test]
fn quitting_answers_every_waiting_approval_cancelled_once() -> Result<(), Box<dyn Error>> {
    let (shared, asked, _specs) = started(two)?;
    let (asks, _) = waiting(&shared, &asked)?;
    assert!(asks > 0, "the scene has an approval waiting");
    let mut main = window(&shared, MAIN);
    main.update(root::quit);
    assert_eq!(waiting(&shared, &asked)?, (0, asks));
    let mut own = window(&shared, OWN);
    own.update(root::quit);
    own.update(closed);
    assert_eq!(
        waiting(&shared, &asked)?.1,
        asks,
        "the second window adds none"
    );
    let hub = shared.try_borrow()?;
    assert_eq!(
        hub.model.grid.window_of(WEB_PANE),
        Some(OWN),
        "it stays out"
    );
    Ok(())
}

/// The owner's first look: closing the app's last window quits it, status item and all.
#[test]
fn closing_the_last_window_quits_the_app() -> Result<(), Box<dyn Error>> {
    let (shared, asked, _specs) = started(docked)?;
    let (asks, _) = waiting(&shared, &asked)?;
    assert!(asks > 0, "the scene has an approval waiting");
    let mut main = window(&shared, MAIN);
    main.frame(1.0 / 60.0);
    main.update(closed);
    assert_eq!(
        waiting(&shared, &asked)?,
        (0, asks),
        "the app goes, asks end"
    );
    Ok(())
}

#[test]
fn a_run_opens_the_windows_the_last_run_left() -> Result<(), Box<dyn Error>> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("launch-{}.json", std::process::id()));
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(|_| {})?;
    let second_screen = display((1200, 0), (1200, 750), 1.0, false);
    let both = [display((0, 0), (2400, 1500), 2.0, true), second_screen];
    let _specs = specs(&first, &both, launch());
    leave_web(&first)?;
    let there = Frame {
        x: 1500.0,
        y: 100.0,
        w: 400.0,
        h: 300.0,
    };
    moved(&mut *first.try_borrow_mut()?, OWN, there);
    keep(&first);

    let (next, _) = hub(|_| {})?;
    let specs = specs(&next, &both[..1], launch());
    fs::remove_file(&file)?;
    assert_eq!(specs.len(), 2, "the app and the pane that was out");
    let hub = next.try_borrow()?;
    assert_eq!(hub.model.grid.window_of(WEB_PANE), Some(OWN), "still out");
    let place = hub.model.grid.window(OWN).and_then(|window| window.frame);
    let main_screen = Frame {
        x: 400.0,
        y: 225.0,
        ..there
    };
    assert_eq!(place, Some(main_screen), "its screen is gone");
    let registry = hub.panes.get::<Registry>(None).ok_or("no registry")?;
    assert!(registry.shows(MAIN) && registry.shows(OWN));
    Ok(())
}

#[test]
fn just_chat_picked_and_the_app_killed_comes_back_as_just_chat() -> Result<(), Box<dyn Error>> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("killed-{}.json", std::process::id()));
    let laptop = [display((0, 0), (2400, 1500), 2.0, true)];
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(|_| {})?;
    let _specs = specs(&first, &laptop, launch());
    let mut main = window(&first, MAIN);
    main.update(|win, _| {
        let saved = "just-chat";
        dispatch(
            win,
            Msg::Grid(GridMsg::Show {
                window: MAIN,
                saved,
            }),
        );
    });
    main.frame(1.0 / 60.0);
    drop(main);
    let (next, _) = hub(|_| {})?;
    let _specs = specs(&next, &laptop, launch());
    let _ = fs::remove_file(&file);
    let hub = next.try_borrow()?;
    let saved = hub.model.grid.window(MAIN).and_then(|window| window.saved);
    assert_eq!(saved, Some("just-chat"), "kept with no quit");
    Ok(())
}

#[test]
fn show_settings_opens_a_window_of_760_by_560_and_a_second_asks_the_same_one_forward()
-> Result<(), Box<dyn Error>> {
    let show = r#"{"menu":"pane.show-settings""#;
    let events = format!(
        r#"{show},"t":0.3}},{{"t":0.8,"type":"tap","x":14,"y":70}},{show},"t":1.4}},{{"t":2.0,"type":"mark","name":"end"}}"#
    );
    let ran = desk("show-settings", ONE, docked, &events)?;
    let own = ran.window("window-1")?;
    assert_eq!(
        (own.frame[2], own.frame[3]),
        (760.0, 560.0),
        "beside eight panes it was too narrow to press"
    );
    assert!(own.visible && own.focused, "asked again, it is in front");
    assert!(
        !ran.window("window-2")?.visible,
        "the second ask made no second window"
    );
    let hub = ran.shared.try_borrow()?;
    let settings = hub
        .model
        .grid
        .panes
        .iter()
        .filter(|p| p.kind == PaneKind::Settings);
    assert_eq!(settings.count(), 1, "one Settings pane, not two");
    Ok(())
}

#[test]
fn show_themes_opens_its_own_window_too() -> Result<(), Box<dyn Error>> {
    let events = r#"{"menu":"pane.show-themes","t":0.3},{"t":1.0,"type":"mark","name":"end"}"#;
    let ran = desk("show-themes", ONE, docked, events)?;
    let own = ran.window("window-1")?;
    assert_eq!((own.frame[2], own.frame[3]), (760.0, 560.0));
    assert!(own.visible);
    Ok(())
}

/// Dies with a Settings window that ⌘W leaves open: with no pane focused, Close Tab found
/// nothing to close.
#[test]
fn command_w_closes_the_settings_window() -> Result<(), Box<dyn Error>> {
    let show = r#"{"menu":"pane.show-settings","t":0.3}"#;
    let close = r#"{"t":1.0,"type":"key","name":"w","cmd":true,"code":"w"}"#;
    let events = format!(r#"{show},{close},{{"t":2.0,"type":"mark","name":"end"}}"#);
    let ran = desk("settings-command-w", ONE, docked, &events)?;
    {
        let hub = ran.shared.try_borrow()?;
        let left = hub
            .model
            .grid
            .panes
            .iter()
            .filter(|p| p.kind == PaneKind::Settings);
        assert_eq!(left.count(), 0, "the Settings pane is still open");
    }
    assert!(!ran.window("window-1")?.visible, "the window is still up");
    Ok(())
}

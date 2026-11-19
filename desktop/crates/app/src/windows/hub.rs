//! The registry of open windows and the window effects a step asks for.

use std::io::Write;
use std::path::PathBuf;

use rune::{
    AnyWindowSpec, App, AppHandle, AppSpec, Display, DisplayTarget, Titlebar, Tx, WindowHandle,
    WindowOptions, WindowSpec,
};
use yd_core::address::uri::Address;
use yd_core::command::table::{Route, Run, Target};
use yd_core::feed::{CHANGES, FILES, LSP, SourceCmd};
use yd_core::grid::step::GridMsg;
use yd_core::grid::windows::{Frame, Window, Windows};
use yd_core::ids::{AgentId, PaneId, ThreadId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::CLOSE;
use yd_ui::chrome::tabs::TITLED;
use yd_ui::shown;
use yd_ui::win::{Hub, Shared, Win, dispatch};

use crate::notify;
use crate::root::{self, MAIN};
use crate::shell::view::{change, shell};
use crate::windows::places::{self, KeptApart, KeptChildren, MainShown, Places, Screen};
use crate::windows::tear::lifted;
use yd_panes::chat::pane::child::{Child, Children};

mod apart;
mod scenes;
#[cfg(test)]
mod tests;
mod view;

pub use apart::{APART, apart, cycle};
pub use scenes::SCENES;

use apart::{APART_AT, APART_SIZE, least};

/// The windows a headless run makes hidden beside the main one: it can open none later.
pub const SPARES: u32 = 4;
/// The first traffic light, from the window's top left (record 2.8.5: 8 from the left).
const LIGHTS: (f32, f32) = (8.0, 4.0);
/// A pane's window before anything says how large: no slot to read and no place kept.
const FIRST_SIZE: (f32, f32) = (420.0, 300.0);
/// A window's title when it draws no pane of one kind.
const TITLE: &str = "Yi Desktop";
/// The gap between the app and a pane's window opened beside it.
const BESIDE: f32 = 12.0;
/// How a run starts its windows.
#[derive(Debug)]
pub struct Launch {
    /// Every window is made at the start, since a headless run can open none later.
    pub headless: bool,
    /// Where the grid is kept between runs.
    pub places: Places,
}

impl Launch {
    pub fn native() -> Self {
        Self {
            headless: false,
            places: Places::open(),
        }
    }

    /// A headless run reads and keeps nothing on disk: a picture shows its scene alone.
    pub fn headless() -> Self {
        Self {
            headless: true,
            places: Places::memory(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Up,
    /// Made hidden by a headless run: free for the next window the grid makes with its id.
    Spare,
    /// Closed by its red button while the grid still holds it: only the user brings it back.
    Away,
}

/// One window Rune has, open or not.
struct Open {
    id: WindowId,
    app: AppHandle<Win>,
    /// The native window, for reading where it is; a headless run has none.
    native: Option<WindowHandle>,
    /// Its name in Rune's window table, which a headless run's frames are listed under.
    key: String,
    state: State,
    /// Settings or Themes: a window whose place is kept when it closes.
    apart: Option<PaneKind>,
}

impl Open {
    /// Where the window is on the desktop, in points. A headless run has no native window
    /// and lists its frames by name instead.
    fn frame(&self, headless: bool) -> Option<Frame> {
        if !headless {
            let at = self.native.as_ref()?.placement()?;
            return Some(places::points(at.position, at.content_size, at.scale));
        }
        let frames = rune::tooling::headless_run()?.window_frames();
        let last = frames.into_iter().rev().find(|at| at.key == self.key)?;
        let [x, y, w, h] = last.frame;
        Some(Frame { x, y, w, h })
    }
}

/// Every window of the run. It lives in the hub (`hub.panes`, under no pane) so a window's
/// own hooks and the effect sink reach the same one.
#[derive(Default)]
pub struct Registry {
    open: Vec<Open>,
    screens: Vec<Screen>,
    headless: bool,
    /// One more for every window opened after the start, so no two share a name.
    serial: u32,
    places: Option<Places>,
    quitting: bool,
    /// Each window's saved layout and panes as last kept: a change to them is kept at once.
    held: Vec<(WindowId, Option<yd_core::grid::saved::Slug>, Vec<PaneId>)>,
    /// The grid as last kept, and as the last poll saw it: a resize is kept once it rests.
    kept: Option<Windows>,
    seen: Option<Windows>,
    /// What the main window showed, and its sidebar's width, as last kept.
    shown: Option<(MainShown, Option<f32>)>,
    /// The chat panes that show a child agent, as last kept.
    children: Option<KeptChildren>,
    /// Where Settings and Themes were last, by slug: they are not reopened at a start.
    apart: KeptApart,
}

/// What `reconcile` needs to open one window, read while the hub is only borrowed.
struct Opening {
    id: WindowId,
    /// The thread the pane showed in the window it left.
    thread: Option<ThreadId>,
    frame: Option<Frame>,
    /// The kind of the one pane it draws, and the least it may be resized to.
    kind: Option<PaneKind>,
    least: (f32, f32),
}

impl Registry {
    fn find(&self, id: WindowId) -> Option<&Open> {
        self.open.iter().find(|open| open.id == id)
    }

    fn find_mut(&mut self, id: WindowId) -> Option<&mut Open> {
        self.open.iter_mut().find(|open| open.id == id)
    }

    /// Whether `id` is on screen, as Rune was last asked.
    pub fn shows(&self, id: WindowId) -> bool {
        self.find(id).is_some_and(|open| open.state == State::Up)
    }

    /// Where `id` is on the desktop, in points.
    pub fn frame(&self, id: WindowId) -> Option<Frame> {
        self.find(id).and_then(|open| open.frame(self.headless))
    }

    fn opening(&self, hub: &Hub, window: &Window) -> Opening {
        let grid = &hub.model.grid;
        let pane = lone(grid, window.id);
        let origin = pane.and_then(|pane| grid.origin_of(pane));
        let thread = origin.and_then(|origin| shown::thread(hub, origin));
        let off_slot = || {
            let home = self.frame(origin?)?;
            let lifted = lifted(grid, pane?, home)?;
            // Incident: a pane out over the app looked like the app's own view; beside it shows.
            let right = home.x + home.w + BESIDE;
            let beside = places::inside(
                Frame {
                    x: right,
                    y: home.y,
                    ..lifted
                },
                &self.screens,
            );
            let clear = beside.x >= right;
            Some(if clear {
                beside
            } else {
                places::inside(lifted, &self.screens)
            })
        };
        let kind = pane.and_then(|pane| grid.kind(pane));
        let apart = || {
            let kind = kind.filter(|kind| APART.contains(kind))?;
            let last = self.apart.iter().find(|(slug, _)| slug == kind.slug());
            if let Some((_, frame)) = last {
                return Some(places::inside(*frame, &self.screens));
            }
            let (w, h) = APART_SIZE;
            let home = origin.and_then(|origin| self.frame(origin));
            let (x, y) = home.map_or(APART_AT, |home| {
                (home.x + (home.w - w) / 2.0, home.y + (home.h - h) / 2.0)
            });
            Some(places::inside(Frame { x, y, w, h }, &self.screens))
        };
        Opening {
            id: window.id,
            thread: thread.map(|thread| thread.id.clone()),
            frame: window.frame.or_else(apart).or_else(off_slot),
            kind,
            least: least(kind, root::zoom(hub) as f32),
        }
    }

    /// The spec of window `id` and the registry's row for it.
    fn window(
        &self,
        shared: &Shared,
        id: WindowId,
        frame: Option<Frame>,
        state: State,
        (app, least): (App<Win>, (f32, f32)),
    ) -> (WindowSpec<Win>, Open) {
        let hub = Shared::clone(shared);
        let app = app
            .init(move || Win::new(hub, id))
            .on_close(closed)
            .on_focus(front)
            .on_quit(root::quit)
            .on_visibility(visible);
        let handle = app.handle();
        let placed = frame.map(|frame| places::pixels(frame, &self.screens));
        let options = WindowOptions {
            transparent: true,
            visible: state == State::Up,
            position: placed.map(|(at, _)| at),
            physical_size: placed.map(|(_, size)| size),
            min_size: (id != MAIN).then_some(least),
            ..WindowOptions::default()
        };
        let key = match (id == MAIN, self.serial) {
            (true, _) => "main".to_owned(),
            (false, 0) => format!("window-{}", id.0),
            (false, serial) => format!("window-{}-{serial}", id.0),
        };
        let created = handle.clone();
        let spec = WindowSpec::from_app(key.clone(), app)
            .options(options)
            .on_created(move |native| {
                created.update(move |win, _| {
                    with(&win.hub, |registry, _| {
                        if let Some(open) = registry.find_mut(win.window) {
                            open.native = Some(native);
                        }
                    });
                });
            });
        let open = Open {
            id,
            app: handle,
            native: None,
            key,
            state,
            apart: None,
        };
        (spec, open)
    }

    fn raise(&mut self, shared: &Shared, opening: Opening, wins: &mut Wins) {
        let Opening {
            id,
            frame,
            kind,
            least,
            ..
        } = opening;
        let apart = kind.filter(|kind| APART.contains(kind));
        // Rune places a rectangle in points from the main screen's top left.
        let rect = frame.map(|frame| places::from_main(frame, &self.screens));
        if let Some(spare) = self.find_mut(id) {
            if let Some(rect) = rect {
                spare.app.place(DisplayTarget::Primary, rect);
            }
            spare.app.show();
            spare.app.focus();
            spare.state = State::Up;
            spare.apart = apart;
            return;
        }
        if self.headless {
            return;
        }
        self.serial = self.serial.saturating_add(1);
        let title = kind.map_or(TITLE, PaneKind::title);
        let app = (pane_app(title, least), least);
        let (spec, mut open) = self.window(shared, id, frame, State::Up, app);
        open.apart = apart;
        wins.push((id, open.app.clone()));
        open.native = Some(rune::open_window(spec));
        self.open.push(open);
    }

    fn shut(&mut self, id: WindowId, wins: &mut Wins) {
        let Some(at) = self.open.iter().position(|open| open.id == id) else {
            return;
        };
        let leaving = self.open.get(at).and_then(|open| {
            let kind = open.apart?;
            Some((kind, open.frame(self.headless)?))
        });
        if let Some((kind, frame)) = leaving {
            self.remember(kind, frame);
        }
        // The thread it showed is closed from a window that stays, else from its own turn.
        let stays = |open: &&Open| open.id != id && open.state == State::Up;
        if let Some(via) = self.open.iter().find(stays).or(self.open.get(at)) {
            via.app.update(move |win, _| part(win, id));
        }
        if self.headless {
            if let Some(open) = self.open.get_mut(at) {
                open.app.hide();
                open.state = State::Spare;
            }
        } else {
            wins.retain(|(window, _)| *window != id);
            self.open.remove(at).app.close();
        }
    }
}

type Wins = Vec<(WindowId, AppHandle<Win>)>;

/// Runs `change` on the registry, with the list of windows a change wakes.
fn with<T>(shared: &Shared, change: impl FnOnce(&mut Registry, &mut Wins) -> T) -> Option<T> {
    let mut hub = shared.try_borrow_mut().ok()?;
    let Hub { panes, wins, .. } = &mut *hub;
    panes
        .get_mut::<Registry>(None)
        .map(|registry| change(registry, wins))
}

/// The one pane a window draws, when it is a pane's own window.
pub fn lone(grid: &Windows, window: WindowId) -> Option<PaneId> {
    let mut docked = grid.window(window)?.layout.docked();
    let (pane, _) = docked.next()?;
    docked.next().is_none().then_some(pane)
}

/// A pane's own window: titled after its pane (the Window menu lists it so), see-through, its
/// lights over the pane's tab bar.
fn pane_app(title: &'static str, least: (f32, f32)) -> App<Win> {
    let spec = AppSpec {
        min_size: Some(least),
        families: root::FAMILIES,
        poll: Some(poll_pane),
        on_key: Some(root::on_key),
        ..AppSpec::new(title, FIRST_SIZE, root::init, view::view)
    };
    App::new(spec).titlebar(Titlebar::new(TITLED).traffic_lights(LIGHTS.0, LIGHTS.1))
}

/// Every window a run starts with, over one hub: the main window, a window for each pane
/// that was out when the grid was last kept, and the spare ones of a headless run.
pub fn specs(shared: &Shared, displays: &[Display], launch: Launch) -> Vec<AnyWindowSpec> {
    let Ok(mut hub) = shared.try_borrow_mut() else {
        return Vec::new();
    };
    let screens = places::screens(displays);
    if let Some(kept) = launch.places.read(MAIN, &screens) {
        hub.model.grid = kept;
    }
    // Incident: a Settings window left open at a kill came back uninvited; it opens when asked.
    let apart = hub
        .model
        .grid
        .panes
        .iter()
        .filter(|pane| APART.contains(&pane.kind));
    let apart: Vec<PaneId> = apart.map(|pane| pane.id).collect();
    for pane in apart {
        step(&mut hub.model, Msg::Grid(GridMsg::Close(pane)));
    }
    let width = launch.places.read_width();
    change(&mut hub, MAIN, |shell| shell.width = width);
    if let Some((hidden, thread, checkouts)) = launch.places.read_shown() {
        change(&mut hub, MAIN, |shell| shell.hidden = hidden);
        if let Some(thread) = thread {
            shown::pick(&mut hub, MAIN, ThreadId::new(thread));
        }
        for (thread, path) in checkouts {
            shown::remember(&mut hub, ThreadId::new(thread), path);
        }
    }
    let kept = launch.places.read_children().unwrap_or_default();
    if let Some(children) = hub.panes.get_mut::<Children>(None) {
        for (pane, thread, agent) in kept {
            let (thread, agent) = (ThreadId::new(thread), AgentId::new(agent));
            children.0.insert(PaneId(pane), Child { thread, agent });
        }
    }
    let mut registry = Registry {
        apart: launch.places.read_apart(&screens),
        screens,
        headless: launch.headless,
        places: Some(launch.places),
        ..Registry::default()
    };
    let place = |id: WindowId| hub.model.grid.window(id).and_then(|window| window.frame);
    let held = hub.model.grid.windows.iter().map(|window| window.id);
    let mut rows: Vec<(WindowId, State)> = vec![(MAIN, State::Up)];
    rows.extend(held.filter(|id| *id != MAIN).map(|id| (id, State::Up)));
    if launch.headless {
        let free = |id: &WindowId| !rows.iter().any(|(held, _)| held == id);
        let spares: Vec<WindowId> = (1..=SPARES).map(WindowId).filter(free).collect();
        rows.extend(spares.into_iter().map(|id| (id, State::Spare)));
    }
    // The main window alone carries the Mac's menus, read from the hub's tables.
    let zoom = root::zoom(&hub) as f32;
    let app = |id: WindowId| {
        let kind = lone(&hub.model.grid, id).and_then(|pane| hub.model.grid.kind(pane));
        let least = least(kind, zoom);
        let title = kind.map_or(TITLE, PaneKind::title);
        match id {
            MAIN => (root::main(!launch.headless, &hub), least),
            _ => (pane_app(title, least), least),
        }
    };
    let made: Vec<_> = rows
        .into_iter()
        .map(|(id, state)| registry.window(shared, id, place(id), state, app(id)))
        .collect();
    let mut specs = Vec::with_capacity(made.len());
    for (spec, open) in made {
        hub.wins.push((open.id, open.app.clone()));
        registry.open.push(open);
        specs.push(spec.boxed());
    }
    if let Some(kept) = hub.panes.get_mut::<Registry>(None) {
        *kept = registry;
    }
    specs
}

/// Makes the real windows the ones the grid holds: opens each it has made, closes each it
/// has dropped. The effect sink calls it for every window effect; true when one changed.
pub fn reconcile(shared: &Shared) -> bool {
    let (opens, closes) = {
        let Ok(hub) = shared.try_borrow() else {
            return false;
        };
        let Some(registry) = hub.panes.get::<Registry>(None) else {
            return false;
        };
        let grid = &hub.model.grid;
        let made = |id: WindowId| {
            let open = registry.find(id);
            open.is_some_and(|open| open.state != State::Spare)
        };
        let lacking = grid.windows.iter().filter(|window| !made(window.id));
        let opens: Vec<Opening> = lacking
            .map(|window| registry.opening(&hub, window))
            .collect();
        let dropped = |open: &&Open| {
            open.state == State::Up && open.id != MAIN && grid.window(open.id).is_none()
        };
        let closes: Vec<WindowId> = registry
            .open
            .iter()
            .filter(dropped)
            .map(|open| open.id)
            .collect();
        (opens, closes)
    };
    if opens.is_empty() && closes.is_empty() {
        return false;
    }
    if let Ok(mut hub) = shared.try_borrow_mut() {
        // A pane's window shows the thread its pane left with, whatever its home shows later.
        for opening in &opens {
            if let Some(thread) = &opening.thread {
                shown::pick(&mut hub, opening.id, thread.clone());
            }
        }
    }
    with(shared, |registry, wins| {
        closes.into_iter().for_each(|id| registry.shut(id, wins));
        for opening in opens {
            registry.raise(shared, opening, wins);
        }
    });
    true
}

/// Closes on the wire the thread window `id` had open, from whichever window's turn this
/// is, and forgets what the window showed: the next window with the id starts afresh.
fn part(win: &mut Win, id: WindowId) {
    let had = win.hub.try_borrow_mut().ok();
    let had = had.and_then(|mut hub| shown::forget(&mut hub, id));
    let thread = had.and_then(|thread| Address::new(Kind::Thread, thread.as_str()).ok());
    if let Some(thread) = thread {
        let run = Run::new(CLOSE).on(Target::Thing(thread));
        dispatch(win, Msg::Command(Route::Wire, run));
    }
}

/// The user brought back a window they had closed: it is up again.
fn returned(win: &mut Win) {
    let id = win.window;
    with(&win.hub, |registry, _| {
        if let Some(open) = registry
            .find_mut(id)
            .filter(|open| open.state == State::Away)
        {
            open.state = State::Up;
        }
    });
}

/// What a window does each frame for the window layer: `root::poll` calls it before it syncs
/// its thread; true when a window was opened or closed.
pub fn poll(win: &mut Win, tx: &mut Tx) -> bool {
    let changed = reconcile(&win.hub);
    hold(&win.hub, tx);
    changed
}

/// Incident: Just chat picked, then the app killed, came back as Everything: the grid was kept
/// only on quit. A layout shown or a pane opened is kept at once, a resize once it rests.
fn hold(shared: &Shared, tx: &mut Tx) {
    let Ok(mut hub) = shared.try_borrow_mut() else {
        return;
    };
    hold_children(&mut hub);
    let Hub { model, panes, .. } = &mut *hub;
    let Some(registry) = panes.get_mut::<Registry>(None) else {
        return;
    };
    let grid = &model.grid;
    let docked = |w: &Window| w.layout.docked().map(|(pane, _)| pane).collect();
    let now: Vec<_> = grid
        .windows
        .iter()
        .map(|w| (w.id, w.saved, docked(w)))
        .collect();
    let moved = registry.kept.as_ref() != Some(grid);
    let rests = registry.seen.as_ref() == Some(grid);
    if moved && !rests {
        registry.seen = Some(grid.clone());
        tx.wake_after(std::time::Duration::from_millis(500));
    }
    if registry.held != now || (moved && rests) {
        registry.held = now;
        registry.kept = Some(grid.clone());
        if let Some(Err(error)) = registry.places.as_mut().map(|p| p.keep(grid)) {
            let _ = writeln!(
                std::io::stderr(),
                "yi-desktop: the windows' places were not kept: {error}"
            );
        }
    }
    let thread = shown::thread(&hub, MAIN).map(|thread| thread.id.as_str().to_owned());
    let main = shell(&hub, MAIN);
    let seen = ((main.hidden, thread, shown::checkouts(&hub)), main.width);
    let Some(registry) = hub.panes.get_mut::<Registry>(None) else {
        return;
    };
    if registry.shown.as_ref() == Some(&seen) {
        return;
    }
    let kept = registry
        .places
        .as_mut()
        .map(|places| places.keep_shown(&seen.0, seen.1));
    registry.shown = Some(seen);
    if let Some(Err(error)) = kept {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: what the window showed was not kept: {error}"
        );
    }
}

/// A child's chat pane is kept with the grid, and forgotten once the grid no longer holds it.
fn hold_children(hub: &mut Hub) {
    let Hub { model, panes, .. } = hub;
    let Some(children) = panes.get_mut::<Children>(None) else {
        return;
    };
    let grid = &model.grid;
    children
        .0
        .retain(|pane, _| grid.kind(*pane) == Some(PaneKind::Chat));
    let now: KeptChildren = children
        .0
        .iter()
        .map(|(pane, child)| (pane.0, child.thread.to_string(), child.agent.to_string()))
        .collect();
    let Some(registry) = panes.get_mut::<Registry>(None) else {
        return;
    };
    if registry.children.as_ref() == Some(&now) {
        return;
    }
    let kept = registry.places.as_mut().map(|p| p.keep_children(&now));
    registry.children = Some(now);
    if let Some(Err(error)) = kept {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: the agents' chat panes were not kept: {error}"
        );
    }
}

fn poll_pane(win: &mut Win, seconds: f64, tx: &mut Tx) -> bool {
    // A spare of a headless run shows nothing: it has no thread to open.
    let up = win.hub.try_borrow().is_ok_and(|hub| {
        let registry = hub.panes.get::<Registry>(None);
        registry.is_some_and(|registry| registry.shows(win.window))
    });
    up && root::poll(win, seconds, tx)
}

/// Reads where every open window is, tells the grid, and keeps the grid for the next run.
pub fn keep(shared: &Shared) {
    let Ok(mut hub) = shared.try_borrow_mut() else {
        return;
    };
    let Hub { model, panes, .. } = &mut *hub;
    let Some(registry) = panes.get_mut::<Registry>(None) else {
        return;
    };
    let mut apart = Vec::new();
    for open in registry.open.iter().filter(|open| open.state == State::Up) {
        if let Some(frame) = open.frame(registry.headless) {
            let window = open.id;
            step(model, Msg::Grid(GridMsg::Moved { window, frame }));
            apart.extend(open.apart.map(|kind| (kind, frame)));
        }
    }
    for (kind, frame) in apart {
        registry.remember(kind, frame);
    }
    let kept = registry
        .places
        .as_mut()
        .map(|places| places.keep(&model.grid));
    if let Some(Err(error)) = kept {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: the windows' places were not kept: {error}"
        );
    }
}

/// The window layer's part of quitting, run in each window still open: the places are kept,
/// once. `root::quit` calls it and then answers the waiting asks.
pub fn quit(win: &mut Win, _: &mut Tx) {
    let first = with(&win.hub, |registry, _| {
        !std::mem::replace(&mut registry.quitting, true)
    });
    if first == Some(true) {
        keep(&win.hub);
    }
}

/// Rune's close hook, for every window: its red button, or the app quitting. A window the
/// grid closed is out of the registry by now, and its thread is closed from another.
pub fn closed(win: &mut Win, tx: &mut Tx) {
    let id = win.window;
    keep(&win.hub);
    let gone = with(&win.hub, |registry, wins| {
        let at = registry.open.iter().position(|open| open.id == id)?;
        let open = registry.open.get_mut(at)?;
        if registry.quitting || open.state != State::Up {
            return None;
        }
        if id == MAIN {
            open.state = State::Away;
        } else {
            registry.open.remove(at);
            wins.retain(|(window, _)| *window != id);
        }
        let last = !registry.open.iter().any(|open| open.state == State::Up);
        Some(last)
    });
    let Some(Some(quits)) = gone else {
        return;
    };
    // Invariant: the main window only goes away; its thread stays open and its asks wait
    // until the app quits.
    if id != MAIN {
        part(win, id);
    }
    // A pane's window closed by hand: the pane goes home, or goes for good if it has none.
    let changes: Vec<GridMsg> = match win.hub.try_borrow() {
        Ok(hub) if id != MAIN => {
            let grid = &hub.model.grid;
            let docked = grid.window(id).map(|window| window.layout.docked());
            let panes = docked.into_iter().flatten().map(|(pane, _)| pane);
            let home = |pane: PaneId| match grid.origin_of(pane) {
                Some(_) => GridMsg::Rejoin(pane),
                None => GridMsg::Close(pane),
            };
            panes.map(home).collect()
        }
        Ok(_) | Err(_) => Vec::new(),
    };
    for change in changes {
        dispatch(win, Msg::Grid(change));
    }
    if quits {
        root::quit(win, tx);
    } else {
        keep(&win.hub);
    }
}

/// Rune's focus hook, for every window. The window in front points the file, git and
/// language sources: they are single feeds, so they show the thread of the window worked in.
pub fn front(win: &mut Win, focused: bool, _: &mut Tx) {
    notify::focus(win, focused);
    if !focused {
        return;
    }
    returned(win);
    let folder = win.hub.try_borrow().ok().and_then(|hub| {
        let folder = shown::works_in(&hub, shown::thread(&hub, win.window)?);
        (!folder.is_empty()).then(|| PathBuf::from(folder))
    });
    let Some(folder) = folder else { return };
    for feed in [FILES, CHANGES, LSP] {
        dispatch(win, Msg::Source(feed, SourceCmd::Root(folder.clone())));
    }
}

/// Rune's visibility hook, for every window: one brought back is up again.
pub fn visible(win: &mut Win, shows: bool, _: &mut Tx) {
    if shows {
        returned(win);
    }
}

//! Places every pane of a window's layout and glides it on a layout change.

use rune::{Chord, Curve, Cx, Easing, El, Key, KeyEvent, Spring, Tx, col, el};
use yd_core::address::uri::Address;
use yd_core::command::table::{Command, FLIP_CHAT, OPEN_FILE, Route, Run, Target};
use yd_core::grid::layout::{Edge, Rect, Size};
use yd_core::grid::saved::{JUST_CHAT, OwnId, Slug};
use yd_core::grid::step::GridMsg;
use yd_core::grid::swap::{Spot, Zone};
use yd_core::ids::{CommandId, PaneId, TabId, WindowId};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::chrome::frame::{Look, Surface, surface};
use crate::chrome::tabs::{self, Bar, Tabs, Turn};
use crate::grid_view::drag::{At, GLIDE, Gesture, HELD, LIFT, LOOSE, preview};
use crate::grid_view::{picker, resize};
use crate::menu::command_box;
use crate::pane::{PaneCx, SCENE_PANE, Scene, Stage, TabAsk};
use crate::theme::look::colour;
use crate::theme::scale::LINE_BAR;
use crate::win::{Edits, Hub, Win, dispatch};

mod scenes;
#[cfg(test)]
mod tests;

pub const SCENES: &[Scene] = scenes::SCENES;

/// The grid's own key: its size is read back from it, and its lines and menu sit under it.
pub const KEY: &str = "grid";
/// The pane in the hand, and the one springing home, paint over the others.
const OVER_OTHERS: i16 = 1;
/// A pane's fill eases to its focused strength, and the others fade while one is held.
const FOCUS: Spring = Spring::new(0.35, 0.0);
const FADE: Curve = Curve::tween(0.3, Easing::EASE);
const DIMMED: f32 = 0.7;

/// What one window's grid remembers between frames.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Grip {
    /// The pane the user clicked into last; chat, or the first pane, until they do.
    pub focus: Option<PaneId>,
    pub gesture: Gesture,
    /// Whether the Panes panel is open, and what it is asking a name for.
    pub picking: bool,
    pub naming: picker::Naming,
}

/// Every window's grip. It lives in the hub so that a scene can pose one; a window reads
/// and changes only its own.
#[derive(Debug, Default)]
pub struct Grips(Vec<(WindowId, Grip)>);

impl Grips {
    pub fn of(&self, window: WindowId) -> Grip {
        let found = self.0.iter().find(|(id, _)| *id == window);
        found.map(|(_, grip)| *grip).unwrap_or_default()
    }

    pub fn set(&mut self, window: WindowId, grip: Grip) {
        self.0.retain(|(id, _)| *id != window);
        self.0.push((window, grip));
    }
}

/// This window's grip as it stands.
pub(crate) fn read(win: &Win) -> Grip {
    let hub = win.hub.try_borrow().ok();
    let grips = hub.as_ref().and_then(|hub| hub.panes.get::<Grips>(None));
    grips.map(|grips| grips.of(win.window)).unwrap_or_default()
}

/// Changes this window's grip, and forgets the grip of every window that has closed.
pub(crate) fn grip(win: &mut Win, change: impl FnOnce(&mut Grip)) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Hub { model, panes, .. } = &mut *hub;
    let Some(grips) = panes.get_mut::<Grips>(None) else {
        return;
    };
    grips.0.retain(|(id, _)| model.grid.window(*id).is_some());
    let mut next = grips.of(window);
    change(&mut next);
    grips.set(window, next);
}

/// The pane keys and commands act on: the one clicked into while it is still drawn here,
/// else chat, else the first pane (record 2.6).
pub fn focused(hub: &Hub, window: WindowId) -> Option<PaneId> {
    let grid = &hub.model.grid;
    let layout = &grid.window(window)?.layout;
    let drawn = |pane: &PaneId| layout.docked().any(|(other, _)| other == *pane);
    let kept = hub
        .panes
        .get::<Grips>(None)
        .and_then(|grips| grips.of(window).focus);
    let chat = layout
        .docked()
        .map(|(pane, _)| pane)
        .find(|pane| grid.kind(*pane) == Some(PaneKind::Chat));
    kept.filter(drawn)
        .or(chat)
        .or_else(|| layout.docked().next().map(|(pane, _)| pane))
}

/// Everything a hand or a key does to the grid that is not a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Focus(PaneId),
    /// Bring a tab in front, and focus its pane.
    Tab(PaneId, TabId),
    /// Close the tab in front; the pane goes with its last tab.
    CloseTab(PaneId),
    /// Close this tab, in front or not: its cross.
    CloseOne(PaneId, TabId),
    Turn(PaneId, Turn),
    /// A tab dragged out of its strip: it becomes a pane in a window of its own.
    TearOut(PaneId, TabId),
    /// The bar's `+`, on a pane that keeps its own tabs.
    NewTab(PaneId),
    Split(PaneId),
    /// Pull the pane out as a window, or dock a pulled-out pane back.
    PopOut(PaneId),
    Close(PaneId),
    /// Show the pane in every session of its window, or only in the one shown.
    Across(PaneId, bool),
    /// Move the pane onto the edge of the pane drawn next to it on that side.
    Move(PaneId, Edge),
    Show(Slug),
    /// Show a layout of the person's own.
    Wear(OwnId),
    FlipChat,
    /// Open or close the Panes panel.
    Pick,
}

/// The kind of `pane`: the grid's word, or the stage's for the pane a scene draws alone.
fn kind_of(hub: &Hub, pane: PaneId) -> Option<PaneKind> {
    let staged = match hub.stage {
        Stage::Pane(kind) if pane == SCENE_PANE => Some(kind),
        Stage::Pane(_) | Stage::App | Stage::View(_) => None,
    };
    hub.model.grid.kind(pane).or(staged)
}

/// What a pane's bar shows: the tabs its definition keeps, else the tabs kept for it, else
/// the one tab its kind names.
fn tabs_of(context: &PaneCx<'_>, kind: PaneKind) -> Tabs {
    let hub = context.hub;
    let own = hub
        .def(kind)
        .and_then(|def| def.tabs)
        .map(|own| own(context));
    let kept = hub.panes.get::<Tabs>(Some(context.pane));
    Tabs::shown(own.as_ref().or(kept), kind)
}

/// Whether panes of `kind` keep their own tabs and hear what is asked of them.
fn hears_tabs(hub: &Hub, kind: PaneKind) -> bool {
    let def = hub.def(kind);
    def.is_some_and(|def| def.tabs.is_some() && def.on_tab.is_some())
}

type Hears = fn(&mut Win, PaneId, TabAsk);

/// The tabs `pane` keeps itself and who hears a press on them; none for a pane whose tabs
/// the chrome keeps.
fn own_tabs(win: &Win, pane: PaneId) -> Option<(Tabs, Hears)> {
    let hub = win.hub.try_borrow().ok()?;
    let def = hub.def(kind_of(&hub, pane)?)?;
    let context = PaneCx {
        hub: &hub,
        pane,
        focused: false,
        edits: None,
    };
    Some(((def.tabs?)(&context), def.on_tab?))
}

/// Runs `code.open` on `file` and puts this window's code viewer in focus, opened when the
/// layout has none, so a file pressed anywhere shows where keys then go.
pub fn open_file(win: &mut Win, tx: &mut Tx, file: Address) {
    let run = Run::new(OPEN_FILE).on(Target::Thing(file));
    dispatch(win, Msg::Command(Route::Model, run));
    // Incident: a layout with no Code pane took the press and showed nothing; it opens one.
    show_pane(win, tx, PaneKind::Code);
}

fn with_tabs<T>(win: &mut Win, pane: PaneId, change: impl FnOnce(&mut Tabs) -> T) -> Option<T> {
    let mut hub = win.hub.try_borrow_mut().ok()?;
    hub.panes.get_mut::<Tabs>(Some(pane)).map(change)
}

fn highest(win: &Win) -> Option<PaneId> {
    let hub = win.hub.try_borrow().ok()?;
    hub.model.grid.panes.iter().map(|pane| pane.id).max()
}

/// A tab pulled out is a split and a leave: the grid makes a pane of the same kind beside
/// this one and pulls it out, which asks the window layer for its window.
fn tear_out(win: &mut Win, tx: &mut Tx, pane: PaneId, tab: TabId) {
    let before = highest(win);
    tx.motion_curve(GLIDE);
    dispatch(win, Msg::Grid(GridMsg::Split(pane)));
    let Some(copy) = highest(win).filter(|id| Some(*id) > before) else {
        return;
    };
    let Some(Some(torn)) = with_tabs(win, pane, |tabs| tabs.take(tab)) else {
        return;
    };
    with_tabs(win, copy, |tabs| {
        tabs.active = Some(torn.id);
        tabs.tabs = vec![torn];
    });
    dispatch(win, Msg::Grid(GridMsg::Leave(copy)));
}

/// Whether this window is not the first and holds one pane: a pane's own window.
fn apart(win: &Win) -> bool {
    let hub = win.hub.try_borrow().ok();
    let layout = hub
        .as_ref()
        .and_then(|hub| hub.model.grid.window(win.window));
    win.window != WindowId(0) && layout.is_some_and(|w| w.layout.docked().count() == 1)
}

/// A tab act on a pane that keeps its own tabs: the pane hears of it and nothing is kept
/// here. False when the chrome keeps this pane's tabs.
fn asked(win: &mut Win, tx: &mut Tx, act_on: Act) -> bool {
    let pane = match act_on {
        Act::Tab(pane, _)
        | Act::CloseTab(pane)
        | Act::CloseOne(pane, _)
        | Act::Turn(pane, _)
        | Act::TearOut(pane, _)
        | Act::NewTab(pane) => pane,
        Act::Focus(_)
        | Act::Split(_)
        | Act::PopOut(_)
        | Act::Close(_)
        | Act::Across(..)
        | Act::Move(..)
        | Act::Show(_)
        | Act::Wear(_)
        | Act::FlipChat
        | Act::Pick => return false,
    };
    let Some((mut tabs, hears)) = own_tabs(win, pane) else {
        return matches!(act_on, Act::NewTab(_));
    };
    let front = tabs.active().map(|tab| tab.id);
    match act_on {
        Act::Tab(_, tab) => {
            hears(win, pane, TabAsk::Show(tab));
            act(win, tx, Act::Focus(pane));
            // The arrow keys rove the strip, so the keyboard stays on the tab it moved to.
            // The arrow keys rove the strip, so the keyboard stays on the tab it moved to.
            if !tx.focus_visible() {
                crate::term::view::focus_shown(win, tx, pane);
            }
        }
        // The pane goes with its last tab, as for tabs the chrome keeps.
        Act::CloseTab(_) | Act::CloseOne(..) if tabs.tabs.len() < 2 => {
            act(win, tx, Act::Close(pane));
        }
        // A pane that is its own window and whose front tab cannot close (Settings): Close Tab
        // closes the window.
        Act::CloseTab(_) if tabs.active().is_some_and(|tab| tab.fixed) && apart(win) => {
            act(win, tx, Act::Close(pane));
        }
        Act::CloseTab(_) => front
            .into_iter()
            .for_each(|tab| hears(win, pane, TabAsk::Close(tab))),
        Act::CloseOne(_, tab) => hears(win, pane, TabAsk::Close(tab)),
        Act::Turn(_, way) => {
            tabs.turn(way);
            let next = tabs.active().map(|tab| tab.id);
            next.into_iter()
                .for_each(|tab| hears(win, pane, TabAsk::Show(tab)));
        }
        Act::NewTab(_) => {
            hears(win, pane, TabAsk::New);
            crate::term::view::focus_shown(win, tx, pane);
        }
        // A pane's own tab stands for something of the pane's: it cannot leave alone.
        Act::TearOut(..)
        | Act::Focus(_)
        | Act::Split(_)
        | Act::PopOut(_)
        | Act::Close(_)
        | Act::Across(..)
        | Act::Move(..)
        | Act::Show(_)
        | Act::Wear(_)
        | Act::FlipChat
        | Act::Pick => {}
    }
    true
}

/// The one place the grid's own state changes outside a drag.
pub fn act(win: &mut Win, tx: &mut Tx, act_on: Act) {
    if asked(win, tx, act_on) {
        return;
    }
    match act_on {
        Act::Focus(pane) => {
            tx.color_motion(FOCUS);
            grip(win, |grip| grip.focus = Some(pane));
        }
        Act::Tab(pane, tab) => {
            with_tabs(win, pane, |tabs| tabs.select(tab));
            act(win, tx, Act::Focus(pane));
        }
        Act::CloseTab(pane) => {
            let front = with_tabs(win, pane, |tabs| tabs.active().map(|tab| tab.id));
            if let Some(front) = front.flatten() {
                act(win, tx, Act::CloseOne(pane, front));
            }
        }
        Act::CloseOne(pane, tab) => {
            let left = with_tabs(win, pane, |tabs| {
                tabs.take(tab);
                tabs.tabs.len()
            });
            if left == Some(0) {
                act(win, tx, Act::Close(pane));
            }
        }
        Act::Turn(pane, way) => {
            with_tabs(win, pane, |tabs| tabs.turn(way));
        }
        Act::TearOut(pane, tab) => tear_out(win, tx, pane, tab),
        Act::NewTab(_) => {}
        Act::Split(pane) => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Grid(GridMsg::Split(pane)));
        }
        Act::PopOut(pane) => {
            let pulled = win
                .hub
                .try_borrow()
                .is_ok_and(|hub| hub.model.grid.origin_of(pane).is_some());
            tx.motion_curve(GLIDE);
            let change = if pulled {
                GridMsg::Rejoin(pane)
            } else {
                GridMsg::Leave(pane)
            };
            dispatch(win, Msg::Grid(change));
        }
        Act::Close(pane) => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Grid(GridMsg::Close(pane)));
            // A closed pane's tabs go with it; one the grid kept keeps them.
            let gone = win
                .hub
                .try_borrow()
                .is_ok_and(|hub| hub.model.grid.kind(pane).is_none());
            if gone {
                with_tabs(win, pane, |tabs| *tabs = Tabs::default());
            }
        }
        Act::Across(pane, on) => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Grid(GridMsg::Across { pane, on }));
        }
        Act::Move(pane, edge) => {
            let window = win.window;
            let asked = win.hub.try_borrow().ok().and_then(|hub| {
                let grid = &hub.model.grid;
                Some((grid.toward(window, pane, edge)?, grid.room(window)))
            });
            if let Some((at, size)) = asked {
                tx.motion_curve(GLIDE);
                dispatch(
                    win,
                    Msg::Grid(GridMsg::Drop {
                        window,
                        pane,
                        at,
                        size,
                    }),
                );
            }
        }
        Act::Show(saved) => {
            tx.motion_curve(GLIDE);
            let window = win.window;
            dispatch(win, Msg::Grid(GridMsg::Show { window, saved }));
            // A layout with chat in it focuses chat: no kept focus means chat first.
            grip(win, |grip| (grip.focus, grip.picking) = (None, false));
        }
        Act::Wear(own) => {
            tx.motion_curve(GLIDE);
            let window = win.window;
            dispatch(win, Msg::Grid(GridMsg::Wear { window, own }));
            grip(win, |grip| (grip.focus, grip.picking) = (None, false));
        }
        Act::FlipChat => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
        }
        Act::Pick => grip(win, |grip| {
            grip.picking = !grip.picking;
            grip.naming = picker::Naming::Idle;
        }),
    }
}

/// Puts a pane of `kind` in front in this window, opening one beside the largest pane when
/// the window draws none. None when the grid cannot make room for it.
pub fn show_pane(win: &mut Win, tx: &mut Tx, kind: PaneKind) -> Option<PaneId> {
    let window = win.window;
    let drawn = |win: &Win| {
        let hub = win.hub.try_borrow().ok()?;
        let grid = &hub.model.grid;
        let mut docked = grid.window(window)?.layout.docked().map(|(pane, _)| pane);
        docked.find(|pane| grid.kind(*pane) == Some(kind))
    };
    let pane = match drawn(win) {
        Some(pane) => pane,
        None => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Grid(GridMsg::Open { window, kind }));
            drawn(win)?
        }
    };
    act(win, tx, Act::Focus(pane));
    Some(pane)
}

/// Every command the grid carries out, for the shortcut map and the command box.
pub fn commands() -> impl Iterator<Item = &'static Command> {
    let layouts = picker::LAYOUTS.iter().map(|(command, _)| command);
    let panes = picker::PANES.iter().map(|(command, _)| command);
    let own = layouts.chain(picker::OWN).chain(picker::COMMANDS);
    own.chain(tabs::COMMANDS).chain(panes)
}

/// Carries out one of the grid's commands in this window, aimed at the pane in focus.
/// False when the command is not the grid's, or there is no pane to aim it at.
pub fn run(win: &mut Win, tx: &mut Tx, command: CommandId) -> bool {
    if let Some(kind) = picker::pane_of(command) {
        return show_pane(win, tx, kind).is_some();
    }
    let pane = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| focused(&hub, win.window));
    let on_pane = |act_on: fn(PaneId) -> Act| pane.map(act_on);
    let found = match command {
        picker::PICK => Some(Act::Pick),
        FLIP_CHAT => Some(Act::FlipChat),
        tabs::CLOSE_TAB => on_pane(Act::CloseTab),
        tabs::PREVIOUS_TAB => pane.map(|pane| Act::Turn(pane, Turn::Back)),
        tabs::NEXT_TAB => pane.map(|pane| Act::Turn(pane, Turn::On)),
        tabs::SPLIT => on_pane(Act::Split),
        tabs::POP_OUT => on_pane(Act::PopOut),
        tabs::CLOSE => on_pane(Act::Close),
        tabs::ACROSS => pane.map(|pane| Act::Across(pane, true)),
        tabs::HERE => pane.map(|pane| Act::Across(pane, false)),
        tabs::MOVE_LEFT => pane.map(|pane| Act::Move(pane, Edge::Left)),
        tabs::MOVE_RIGHT => pane.map(|pane| Act::Move(pane, Edge::Right)),
        tabs::MOVE_UP => pane.map(|pane| Act::Move(pane, Edge::Top)),
        tabs::MOVE_DOWN => pane.map(|pane| Act::Move(pane, Edge::Bottom)),
        other => picker::slug_of(other)
            .map(Act::Show)
            .or_else(|| kept(win, other)),
    };
    found.map(|found| act(win, tx, found)).is_some()
}

/// The kept layout a command shows, while one is kept under its number.
fn kept(win: &Win, command: CommandId) -> Option<Act> {
    let own = picker::own_of(command)?;
    let hub = win.hub.try_borrow().ok()?;
    let found = hub.model.grid.own.iter().any(|kept| kept.id == own);
    found.then_some(Act::Wear(own))
}

/// The grid's shortcuts: true when the key was one of them. The window's own key handler
/// may call it too, for keys pressed while nothing in the grid has the keyboard.
pub fn keys(win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    if event.key == Key::Escape && read(win).picking {
        picker::close(win);
        return true;
    }
    let map = match win.hub.try_borrow() {
        Ok(hub) => command_box::keymap(&hub),
        Err(_) => return false,
    };
    // Incident: ⌘, in a pane's window docked Settings there; the window opens it apart.
    let apart = [picker::SHOW_SETTINGS, picker::SHOW_THEMES];
    let pressed = |(key, id): &(&str, CommandId)| {
        let chord = key.parse::<Chord>().ok();
        commands().any(|command| command.id == *id)
            && !apart.contains(id)
            && chord.is_some_and(|chord| chord.matches_tx(event, tx))
    };
    let hit = map.bound().find(pressed).map(|(_, id)| id);
    hit.is_some_and(|command| run(win, tx, command))
}

/// What every pane of one frame shares.
struct Frame<'a> {
    hub: &'a Hub,
    edits: &'a Edits,
    /// The room the traffic lights take, in a window whose one pane's bar is its title bar.
    lights: Option<f32>,
    at: At,
    grip: Grip,
    /// Where the pane in the hand would land.
    zone: Option<Zone>,
    focus: Option<PaneId>,
    /// Just chat: one pane, no bar, no surface.
    bare: bool,
    /// The only pane of its window.
    alone: bool,
    /// Reduced motion: a held pane moves plainly, with no tilt and no lift.
    calm: bool,
}

fn pane(frame: &Frame<'_>, cx: &mut Cx<Win>, id: PaneId, kind: PaneKind, rect: Rect) -> El {
    let hub = frame.hub;
    let tokens = &hub.model.tokens;
    let pulled = hub.model.grid.origin_of(id).is_some();
    let focused = frame.focus == Some(id) && !pulled && !frame.bare;
    let look = match frame.grip.gesture {
        Gesture::Pane(held) if held.pane == id => Look::Held,
        Gesture::Pane(_) if frame.zone.is_some_and(|z| z.spot == Spot::Onto(id)) => Look::Target,
        Gesture::Pane(_) => Look::Dimmed,
        Gesture::Idle
        | Gesture::Armed(_)
        | Gesture::Home(_)
        | Gesture::Tab { .. }
        | Gesture::Line(_) => Look::Rest,
    };
    let context = PaneCx {
        hub,
        pane: id,
        focused,
        edits: Some(frame.edits),
    };
    let key = context.key(kind);
    let shown = tabs_of(&context, kind);
    let bar = (!frame.bare).then(|| {
        let bar = Bar {
            pane: id,
            key: &key,
            tabs: &shown,
            focused,
            pulled,
            alone: frame.alone,
            closes: hears_tabs(hub, kind),
            lights: frame.lights,
            at: frame.at,
        };
        tabs::bar(tokens, &bar)
    });
    let placed = surface(
        tokens,
        Surface {
            focused,
            bare: frame.bare,
            look,
        },
    )
    .key(format!("{key}.pane-frame"))
    .absolute(rect.x, rect.y)
    .size(rect.w, rect.h)
    .opacity_motion(FADE)
    .on_background_press_tx(move |win: &mut Win, tx: &mut Tx| act(win, tx, Act::Focus(id)))
    .children(bar)
    .child(body(&context, cx, kind, frame.bare));
    match frame.grip.gesture {
        Gesture::Pane(held) if held.pane == id => {
            let (tilt, lift) = if frame.calm {
                (0.0, 1.0)
            } else {
                (held.tilt.to_radians(), LIFT)
            };
            placed
                .paint_order(OVER_OTHERS)
                .offset_motion(HELD)
                .offset(held.by[0], held.by[1])
                .rotate_motion(HELD)
                .rotate(tilt)
                .scale_motion(HELD)
                .scale(lift)
        }
        Gesture::Home(home) if home == id => placed
            .paint_order(OVER_OTHERS)
            .offset_motion(LOOSE)
            .offset(0.0, 0.0)
            .rotate_motion(LOOSE)
            .rotate(0.0)
            .scale_motion(LOOSE),
        Gesture::Pane(_) => placed.opacity(DIMMED),
        Gesture::Idle
        | Gesture::Armed(_)
        | Gesture::Home(_)
        | Gesture::Tab { .. }
        | Gesture::Line(_) => placed,
    }
}

/// A pane's own view under its bar. The bar's hairline falls on the first line under it
/// (record 2.6); a bare pane has no bar and no line.
fn body(context: &PaneCx<'_>, cx: &mut Cx<Win>, kind: PaneKind, bare: bool) -> El {
    let def = context.hub.def(kind);
    let under = if bare { 0.0 } else { 1.0 };
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .border_sides(
            [under, 0.0, 0.0, 0.0],
            colour(LINE_BAR.of(context.tokens())),
        )
        .child(def.map_or_else(el, |def| (def.view)(context, cx)))
}

/// One pane filling its window under its bar, outside any grid: what a scene of one pane
/// draws, so its tabs and meta are the ones the grid would give it.
pub fn alone(win: &Win, hub: &Hub, cx: &mut Cx<Win>, kind: PaneKind) -> El {
    let context = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    let key = context.key(kind);
    let bar = Bar {
        pane: SCENE_PANE,
        key: &key,
        tabs: &tabs_of(&context, kind),
        focused: true,
        pulled: false,
        alone: true,
        closes: hears_tabs(hub, kind),
        lights: None,
        at: At {
            origin: [0.0, 0.0],
            size: Size { w: 0.0, h: 0.0 },
        },
    };
    col()
        .w_full()
        .h_full()
        .child(tabs::bar(&hub.model.tokens, &bar))
        .child(body(&context, cx, kind, false))
}

/// The grid of one window: every pane where `place` puts it, and the lines between them. It
/// fills whatever room its parent gives it.
pub fn grid(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let known = cx.window_layout(KEY);
    let [x, y, w, h] = known.unwrap_or_default();
    let at = At {
        origin: [x, y],
        size: Size { w, h },
    };
    let model = &hub.model;
    // Incident: panes laid out before the grid knew its size were 0 by 0 for one layout, and
    // every scroll area in them was reported as collapsed. None is drawn until it is known.
    let placed = match known {
        Some(_) => model.grid.place(win.window, at.size),
        None => Vec::new(),
    };
    let grips = hub.panes.get::<Grips>(None);
    let saved = model
        .grid
        .window(win.window)
        .and_then(|window| window.saved);
    // Invariant: the grid's first window has the shell and its top bar; in any other, a
    // lone pane's bar is the title bar and starts clear of the traffic lights.
    let home = model.grid.windows.first().map(|window| window.id);
    let titled = placed.len() == 1 && home.is_some_and(|home| home != win.window);
    let grip = grips.map(|grips| grips.of(win.window)).unwrap_or_default();
    let zone = match grip.gesture {
        Gesture::Pane(held) => held.zone(hub, win.window, at.size),
        _ => None,
    };
    let frame = Frame {
        hub,
        edits: &win.edits,
        lights: titled.then(|| cx.chrome_insets().leading),
        at,
        grip,
        zone,
        focus: focused(hub, win.window),
        bare: saved == Some(JUST_CHAT.slug),
        alone: placed.len() == 1,
        calm: cx.is_reduced_motion(),
    };
    let mut panes = Vec::with_capacity(placed.len());
    for (id, rect) in &placed {
        if let Some(kind) = model.grid.kind(*id) {
            panes.push(pane(&frame, cx, *id, kind, *rect));
        }
    }
    // A pane in the hand is not resizing: the lines stay out of its way.
    let lines = if matches!(frame.grip.gesture, Gesture::Pane(_)) {
        Vec::new()
    } else {
        resize::handles(&model.grid, win.window, at, &placed)
    };
    el().key(KEY)
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .focusable(true)
        .no_focus_ring()
        .on_key(keys)
        .children(panes)
        .children(lines)
        .children(zone.map(|zone| preview(&model.tokens, zone)))
}

//! Dragging a pane by its bar: onto another to swap, onto an edge of one or of the window to
//! dock it there, with weight and a preview of where it lands.

use rune::{Curve, Drag, Easing, El, Phase, Spring, Tx, col, el, text};
use yd_core::grid::layout::{Axis, Line, Point, Rect, Size};
use yd_core::grid::step::GridMsg;
use yd_core::grid::swap::Zone;
use yd_core::ids::{PaneId, TabId, WindowId};
use yd_core::msg::Msg;
use yd_core::tokens::Tokens;

use crate::chrome::tabs::{BAR, Tabs};
use crate::grid_view::view::{Act, act, grip, read};
use crate::theme::look::{META, colour};
use crate::theme::scale::TARGET_RING;
use crate::win::{Hub, Win, dispatch};

/// A layout change glides on the mockup's own curve (record 2.7.3).
pub const GLIDE: Curve = Curve::tween(0.52, Easing::cubic(0.22, 1.0, 0.36, 1.0));
/// Record 2.11.2 as Rune writes a spring: stiffness 210 and damping 22 in the hand,
/// 150 and 15 let go, mass 1. `duration` is 2π/√k and `bounce` is 1 − c/(2√k).
pub const HELD: Spring = Spring::new(0.433_58, 0.240_92);
pub const LOOSE: Spring = Spring::new(0.513_02, 0.387_63);
/// A held pane draws this much larger.
pub const LIFT: f32 = 1.025;
/// Degrees of tilt for each point a second of sideways speed, and the most it tilts.
const TILT: f32 = 0.012;
const TILT_MOST: f32 = 3.2;
/// How far above or below its bar a tab is pulled before it comes out.
const TEAR: f32 = 24.0;
/// Incident: a click on a tab lifted its pane and sprang it home. A pane is carried only
/// once the pointer has gone this far with the button held.
const THRESHOLD: f32 = 4.0;
/// How strongly the theme's accent fills where a dropped pane will go.
const PREVIEW: f32 = 0.16;
/// Drawn over every pane, the held one included, so the place shows through it.
const OVER_ALL: i16 = 2;

/// Where the grid is drawn in its window, so a pointer can be read in the grid's own points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct At {
    pub origin: [f32; 2],
    pub size: Size,
}

impl At {
    fn point(&self, drag: &Drag) -> Point {
        let [x, y] = drag.window_point();
        let [left, top] = self.origin;
        Point {
            x: x - left,
            y: y - top,
        }
    }
}

/// What the pointer took hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    Bar(PaneId),
    Tab(PaneId, TabId),
    Line(Line),
}

/// A pane in the hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub pane: PaneId,
    /// How far the pointer has carried it from home.
    pub by: [f32; 2],
    /// Degrees it leans into its travel.
    pub tilt: f32,
    /// Where the pointer is, in the grid's points: the zone it is over says where it lands.
    pub at: Option<Point>,
    /// When the pointer last moved and how far across it was, for its speed.
    seen: (f64, f32),
}

impl Held {
    /// A pane held still at `by` from home: what a posed picture shows.
    pub fn posed(pane: PaneId, by: [f32; 2], tilt: f32, at: Option<Point>) -> Held {
        Held {
            pane,
            by,
            tilt,
            at,
            seen: (0.0, by[0]),
        }
    }

    /// The drop zone under the pointer in `window`'s grid of `size`.
    pub fn zone(&self, hub: &Hub, window: WindowId, size: Size) -> Option<Zone> {
        let at = self.at?;
        hub.model.grid.zone_at(window, size, self.pane, at)
    }
}

/// What the pointer is doing to one window's grid.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Gesture {
    #[default]
    Idle,
    /// Pressed on a bar or a lone tab: a click until the pointer passes the threshold.
    Armed(PaneId),
    Pane(Held),
    /// Let go: the pane stays over the others while it springs home.
    Home(PaneId),
    Tab {
        pane: PaneId,
        tab: TabId,
        /// Pulled far enough from its bar to come out on release.
        torn: bool,
    },
    Line(Line),
}

fn placed(win: &Win, at: At) -> Vec<(PaneId, Rect)> {
    let hub = win.hub.try_borrow();
    hub.map_or_else(
        |_| Vec::new(),
        |hub| hub.model.grid.place(win.window, at.size),
    )
}

/// One step of a drag that began on a bar, a tab or a line between panes.
pub fn hand(win: &mut Win, grab: Grab, at: At, drag: &Drag, tx: &mut Tx) {
    match drag.phase {
        Phase::Down => down(win, grab, at, drag.clicks, tx),
        Phase::Move => moved(win, at, drag, tx),
        Phase::Up => up(win, at, drag.cancelled(), tx),
    }
}

fn down(win: &mut Win, grab: Grab, at: At, clicks: u32, tx: &mut Tx) {
    let several = placed(win, at).len() > 1;
    // A window's only pane has nothing to swap with: its bar moves the window instead.
    let hold = |pane: PaneId| {
        if several {
            Gesture::Armed(pane)
        } else {
            Gesture::Idle
        }
    };
    let gesture = match grab {
        Grab::Line(line) if clicks >= 2 => {
            tx.motion_curve(GLIDE);
            let (window, size) = (win.window, at.size);
            dispatch(win, Msg::Grid(GridMsg::Even { window, line, size }));
            Gesture::Idle
        }
        Grab::Line(line) => Gesture::Line(line),
        Grab::Bar(pane) => {
            act(win, tx, Act::Focus(pane));
            hold(pane)
        }
        Grab::Tab(pane, tab) => {
            act(win, tx, Act::Tab(pane, tab));
            let tabs = win.hub.try_borrow().ok().map_or(0, |hub| {
                let kept = hub.panes.get::<Tabs>(Some(pane));
                kept.map_or(0, |tabs| tabs.tabs.len())
            });
            if tabs > 1 {
                Gesture::Tab {
                    pane,
                    tab,
                    torn: false,
                }
            } else {
                hold(pane)
            }
        }
    };
    grip(win, |grip| grip.gesture = gesture);
}

fn moved(win: &mut Win, at: At, drag: &Drag, tx: &mut Tx) {
    let point = at.point(drag);
    let far = drag.dx.hypot(drag.dy) >= THRESHOLD;
    let gesture = match read(win).gesture {
        Gesture::Armed(pane) if far => Gesture::Pane(Held {
            seen: (tx.clock(), 0.0),
            ..Held::posed(pane, [0.0, 0.0], 0.0, None)
        }),
        other => other,
    };
    match gesture {
        Gesture::Idle | Gesture::Home(_) | Gesture::Armed(_) => {}
        Gesture::Pane(held) => {
            let now = tx.clock();
            let (then, across) = held.seen;
            // Seconds between two pointer moves: small, so the narrowing loses nothing.
            let passed = (now - then) as f32;
            let speed = if passed > 0.0 {
                (drag.dx - across) / passed
            } else {
                0.0
            };
            let next = Held {
                by: [drag.dx, drag.dy],
                tilt: (speed * TILT).clamp(-TILT_MOST, TILT_MOST),
                at: Some(point),
                seen: (now, drag.dx),
                ..held
            };
            tx.animated();
            grip(win, |grip| grip.gesture = Gesture::Pane(next));
        }
        Gesture::Tab { pane, tab, .. } => {
            let home = placed(win, at)
                .into_iter()
                .find(|(other, _)| *other == pane);
            let Some((_, rect)) = home else { return };
            let torn = point.y < rect.y - TEAR || point.y > rect.y + BAR + TEAR;
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(tabs) = hub.panes.get_mut::<Tabs>(Some(pane))
            {
                let slot = tabs.slot_at(tab, point.x - rect.x);
                tabs.place(tab, slot);
            }
            tx.animated();
            grip(win, |grip| grip.gesture = Gesture::Tab { pane, tab, torn });
        }
        Gesture::Line(line) => {
            let to = match line.axis {
                Axis::Cols => point.x,
                Axis::Rows => point.y,
            };
            let (window, size) = (win.window, at.size);
            let change = GridMsg::Resize {
                window,
                line,
                to,
                size,
            };
            dispatch(win, Msg::Grid(change));
        }
    }
}

fn up(win: &mut Win, at: At, cancelled: bool, tx: &mut Tx) {
    let gesture = read(win).gesture;
    let next = match gesture {
        Gesture::Pane(held) => {
            let window = win.window;
            let zone = win
                .hub
                .try_borrow()
                .ok()
                .and_then(|hub| held.zone(&hub, window, at.size));
            match zone.filter(|zone| zone.lands.is_some() && !cancelled) {
                Some(zone) => {
                    tx.motion_curve(GLIDE);
                    let (pane, size) = (held.pane, at.size);
                    let at = zone.spot;
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
                None => tx.animated(),
            }
            Gesture::Home(held.pane)
        }
        Gesture::Tab {
            pane,
            tab,
            torn: true,
        } if !cancelled => {
            act(win, tx, Act::TearOut(pane, tab));
            Gesture::Idle
        }
        Gesture::Tab { .. } | Gesture::Line(_) | Gesture::Idle | Gesture::Armed(_) => Gesture::Idle,
        Gesture::Home(pane) => Gesture::Home(pane),
    };
    grip(win, |grip| grip.gesture = next);
}

/// Where a carried pane will go, drawn over the grid in the theme's accent; where it cannot
/// go, the zone itself, saying so.
pub fn preview(tokens: &Tokens, zone: Zone) -> El {
    let ring = colour(TARGET_RING.of(tokens));
    let Some(lands) = zone.lands else {
        let band = zone.band;
        let note = text("Not enough room for a pane here")
            .font(META)
            .color(colour(tokens.faint));
        return col()
            .key("grid.drop-refused")
            .absolute(band.x, band.y)
            .size(band.w, band.h)
            .center()
            .fill(colour(tokens.window.alpha(PREVIEW * 4.0)))
            .paint_order(OVER_ALL)
            .inert()
            .child(note);
    };
    el().key("grid.drop-zone")
        .absolute(lands.x, lands.y)
        .size(lands.w, lands.h)
        .fill(colour(tokens.accent.alpha(PREVIEW)))
        .border_sides([1.5; 4], ring)
        .paint_order(OVER_ALL)
        .inert()
}

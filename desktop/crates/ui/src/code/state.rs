//! The surface's own state: which token the pointer rests on. One pointer, so one hover for
//! the whole app, kept beside the panes' states and stepped by pointer moves and the clock.

use std::rc::Rc;

use rune::Tx;

use super::doc::LineNo;
use super::pointer::{CodeEvent, Delays, Effect, Hover, Input, Token};
use crate::win::{Hub, Win};

/// Who hears a surface: the handler its view was built with.
pub type Hear = Rc<dyn Fn(&mut Win, CodeEvent, &mut Tx)>;

/// Where the pointer is on a surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pointer {
    pub line: LineNo,
    pub token: Option<Token>,
    /// Left of the code: over the line's number or its mark column.
    pub gutter: bool,
}

/// The pointer on the code surfaces of every window.
#[derive(Default)]
pub struct Code {
    /// The key of the surface the pointer is over.
    surface: String,
    hover: Hover<Token>,
    pointer: Option<Pointer>,
    /// The handler of that surface, for a hover the clock starts or ends.
    hear: Option<Hear>,
}

impl Code {
    fn on<'a>(hub: &'a Hub, surface: &str) -> Option<&'a Code> {
        let code = hub.panes.get::<Code>(None)?;
        (code.surface == surface).then_some(code)
    }

    /// The line whose row shows a hand: the pointer is on a name to follow there.
    fn hand(&self) -> Option<LineNo> {
        let pointer = self.pointer?;
        pointer
            .token
            .is_some_and(|t| t.follows)
            .then_some(pointer.line)
    }

    /// Steps the hover and returns what it reports, asking `tx` for the wakes it needs.
    fn step(&mut self, input: Input<Token>, tx: &mut Tx) -> Vec<CodeEvent> {
        let mut events = Vec::new();
        for effect in self.hover.step(input, tx.clock(), Delays::DEFAULT) {
            match effect {
                Effect::WakeAt(due) => tx.wake_at(due),
                Effect::Enter(token) => events.push(CodeEvent::HoverStart(token)),
                Effect::Leave(token) => events.push(CodeEvent::HoverEnd(token)),
            }
        }
        events
    }
}

/// The token whose hover has started on the surface keyed `surface`.
pub fn hovered(hub: &Hub, surface: &str) -> Option<Token> {
    Code::on(hub, surface)?.hover.shown().copied()
}

/// The line of `surface` that shows a hand.
pub(super) fn hand(hub: &Hub, surface: &str) -> Option<LineNo> {
    Code::on(hub, surface)?.hand()
}

/// Where the pointer is on `surface`, for a press.
pub(super) fn pointer(hub: &Hub, surface: &str) -> Option<Pointer> {
    Code::on(hub, surface)?.pointer
}

/// Steps the hover's timers. Call it from the window's `poll`; the view runs again when it
/// returns true.
pub fn tick(win: &mut Win, tx: &mut Tx) -> bool {
    let stepped = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        let code = hub.panes.get_mut::<Code>(None)?;
        Some((code.step(Input::Tick, tx), code.hear.clone()))
    });
    let Some((events, hear)) = stepped else {
        return false;
    };
    let changed = !events.is_empty();
    tell(win, hear.as_ref(), events, tx);
    changed
}

/// The pointer moved to `to` on `surface`, or off it with none. The view runs again only when
/// a hover started or ended, or the hand moved to another line.
pub(super) fn point(win: &mut Win, surface: &str, to: Option<Pointer>, hear: &Hear, tx: &mut Tx) {
    let stepped = win.hub.try_borrow_mut().ok().and_then(|mut hub| {
        let code = hub.panes.get_mut::<Code>(None)?;
        if code.surface != surface {
            // A leave from a surface the pointer is no longer on changes nothing.
            to?;
            *code = Code {
                surface: surface.to_owned(),
                ..Code::default()
            };
        }
        let hand = code.hand();
        // Invariant: only `tick` steps the clock, so a hover starts and ends from the poll and
        // a line the pointer crosses never runs the whole view.
        let events = code.step(Input::Over(to.and_then(|p| p.token)), tx);
        (code.pointer, code.hear) = (to, Some(hear.clone()));
        Some((events, hand != code.hand()))
    });
    let Some((events, hand_moved)) = stepped else {
        tx.live_only();
        return;
    };
    if events.is_empty() && !hand_moved {
        tx.live_only();
    }
    tell(win, Some(hear), events, tx);
}

/// The pointer left `line` of `surface`: nothing more unless it is still held to be there.
pub(super) fn leave(win: &mut Win, surface: &str, line: LineNo, hear: &Hear, tx: &mut Tx) {
    let there = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| pointer(&hub, surface));
    if there.is_some_and(|p| p.line == line) {
        point(win, surface, None, hear, tx);
    } else {
        tx.live_only();
    }
}

fn tell(win: &mut Win, hear: Option<&Hear>, events: Vec<CodeEvent>, tx: &mut Tx) {
    if let Some(hear) = hear {
        events.into_iter().for_each(|event| hear(win, event, tx));
    }
}

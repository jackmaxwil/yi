//! Hover intent, press and command-press on a token: what the surface tells its caller, and
//! when resting on a token counts as a hover. The clock comes in with each step.

use rune::machine::Effects;

use super::doc::{Cols, LineNo};
use super::rows::Lines;

/// A token of the text: a name the caller marked as one to follow, or else the word under the
/// pointer. `cols` are bytes of the line's source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token {
    pub line: LineNo,
    pub cols: Cols,
    pub follows: bool,
}

/// What the surface tells its caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CodeEvent {
    /// The pointer has rested on a token.
    HoverStart(Token),
    /// The pointer has been off that token for a while.
    HoverEnd(Token),
    Press(Token),
    /// A press with Command held.
    CommandPress(Token),
    /// A press left of the code: on a line's number or its mark column.
    GutterPress(LineNo),
    /// A right-click on a line: the token under it, and where in the window a menu opens.
    SecondaryPress {
        line: LineNo,
        token: Option<Token>,
        at: [f32; 2],
    },
    /// The lines the selection now touches; none once it is cleared.
    Selection(Option<Lines>),
}

/// Seconds the pointer rests before a hover starts, and stays away before it ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Delays {
    pub enter: f64,
    pub leave: f64,
}

impl Delays {
    pub const DEFAULT: Delays = Delays {
        enter: 0.35,
        leave: 0.12,
    };
}

/// The hover over tokens of type `T`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Hover<T> {
    #[default]
    Idle,
    /// The pointer rests on `at`; the hover starts at `due`.
    Entering {
        at: T,
        due: f64,
    },
    Shown {
        at: T,
    },
    /// The pointer left `at` for `next`; the hover ends at `due` unless it comes back.
    Leaving {
        at: T,
        due: f64,
        next: Option<T>,
    },
}

/// What happened: the pointer is over a token or none, or time passed.
#[derive(Debug, Clone, PartialEq)]
pub enum Input<T> {
    Over(Option<T>),
    Tick,
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect<T> {
    /// Step again with [`Input::Tick`] at this time.
    WakeAt(f64),
    Enter(T),
    Leave(T),
}

impl<T: Clone + PartialEq> Hover<T> {
    /// The token whose hover has started and not yet ended.
    pub fn shown(&self) -> Option<&T> {
        match self {
            Hover::Shown { at } | Hover::Leaving { at, .. } => Some(at),
            Hover::Idle | Hover::Entering { .. } => None,
        }
    }

    pub fn step(&mut self, input: Input<T>, now: f64, delays: Delays) -> Effects<Effect<T>> {
        let enter = |at: T| {
            let due = now + delays.enter;
            (
                Hover::Entering { at, due },
                Effects::one(Effect::WakeAt(due)),
            )
        };
        let (next, effects) = match (std::mem::take(self), input) {
            (Hover::Idle, Input::Over(Some(at))) => enter(at),
            (Hover::Entering { at, due }, Input::Over(Some(to))) if to == at => {
                (Hover::Entering { at, due }, Effects::none())
            }
            (Hover::Entering { .. }, Input::Over(Some(to))) => enter(to),
            (Hover::Entering { .. }, Input::Over(None)) => (Hover::Idle, Effects::none()),
            (Hover::Entering { at, due }, Input::Tick) if now >= due => (
                Hover::Shown { at: at.clone() },
                Effects::one(Effect::Enter(at)),
            ),
            (Hover::Shown { at }, Input::Over(to)) if to.as_ref() == Some(&at) => {
                (Hover::Shown { at }, Effects::none())
            }
            (Hover::Shown { at }, Input::Over(next)) => {
                let due = now + delays.leave;
                (
                    Hover::Leaving { at, due, next },
                    Effects::one(Effect::WakeAt(due)),
                )
            }
            (Hover::Leaving { at, .. }, Input::Over(to)) if to.as_ref() == Some(&at) => {
                (Hover::Shown { at }, Effects::none())
            }
            (Hover::Leaving { at, due, .. }, Input::Over(next)) => {
                (Hover::Leaving { at, due, next }, Effects::none())
            }
            (Hover::Leaving { at, due, next }, Input::Tick) if now >= due => {
                let mut effects = Effects::one(Effect::Leave(at));
                let (state, more) = next.map_or((Hover::Idle, Effects::none()), enter);
                effects.extend(more);
                (state, effects)
            }
            (state, Input::Over(None) | Input::Tick) => (state, Effects::none()),
        };
        *self = next;
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::{Delays, Effect, Hover, Input};

    type Check = Result<(), Box<dyn std::error::Error>>;

    const D: Delays = Delays {
        enter: 0.35,
        leave: 0.12,
    };

    fn step(
        h: &mut Hover<&'static str>,
        input: Input<&'static str>,
        now: f64,
    ) -> Vec<Effect<&'static str>> {
        h.step(input, now, D).into_iter().collect()
    }

    #[test]
    fn a_hover_starts_after_a_rest_and_ends_after_a_gap() -> Check {
        let mut h = Hover::Idle;
        assert_eq!(
            step(&mut h, Input::Over(Some("take")), 0.0),
            [Effect::WakeAt(0.35)]
        );
        assert_eq!(
            step(&mut h, Input::Over(Some("take")), 0.2),
            [],
            "moving on the same token does not start the wait again"
        );
        assert_eq!(step(&mut h, Input::Tick, 0.2), []);
        assert_eq!(h.shown(), None);
        assert_eq!(step(&mut h, Input::Tick, 0.35), [Effect::Enter("take")]);
        assert_eq!(h.shown(), Some(&"take"));

        assert_eq!(step(&mut h, Input::Over(None), 1.0), [Effect::WakeAt(1.12)]);
        assert_eq!(
            step(&mut h, Input::Over(Some("take")), 1.05),
            [],
            "back before the gap ran out"
        );
        assert_eq!(step(&mut h, Input::Tick, 1.12), []);
        assert_eq!(h, Hover::Shown { at: "take" });

        assert_eq!(
            step(&mut h, Input::Over(Some("Bucket")), 2.0),
            [Effect::WakeAt(2.12)]
        );
        assert_eq!(
            step(&mut h, Input::Tick, 2.12),
            [Effect::Leave("take"), Effect::WakeAt(2.12 + 0.35)]
        );
        assert_eq!(step(&mut h, Input::Tick, 2.47), [Effect::Enter("Bucket")]);
        Ok(())
    }

    #[test]
    fn a_pass_shorter_than_the_rest_reports_nothing() -> Check {
        let mut h = Hover::Idle;
        assert_eq!(
            step(&mut h, Input::Over(Some("take")), 0.0),
            [Effect::WakeAt(0.35)]
        );
        assert_eq!(step(&mut h, Input::Over(None), 0.1), []);
        assert_eq!(step(&mut h, Input::Tick, 0.35), []);
        assert_eq!(h, Hover::Idle);
        Ok(())
    }
}

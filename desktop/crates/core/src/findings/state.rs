//! A finding's stages and the one table of what moves a finding between them.

use crate::findings::triage::Triage;

/// The findings part of the model: only what the user decided. Findings themselves are a
/// view, derived on every read by [`crate::findings::view::findings`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Findings {
    pub triage: Triage,
}

/// Where a finding is in its life. `New` is a suggestion: it is never counted as open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    New,
    Open,
    Fixing,
    Proving,
    Watching,
    Done,
}

impl Stage {
    pub const ALL: [Stage; 6] = [
        Stage::New,
        Stage::Open,
        Stage::Fixing,
        Stage::Proving,
        Stage::Watching,
        Stage::Done,
    ];

    /// Filed by the user: anything past a suggestion.
    pub fn filed(self) -> bool {
        !matches!(self, Stage::New)
    }

    /// Filed and not closed: what the hub's "open" count and the gate read.
    pub fn open(self) -> bool {
        self.filed() && self != Stage::Done
    }
}

/// The mark a check carries, as the hub draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Met,
    Failing,
    /// Runnable, waiting on someone: often the user's go-ahead.
    Waiting,
    /// Not yet runnable.
    Pending,
}

/// That every check of a finding passed. Only [`Proof::of`] makes one, so nothing reaches
/// [`Stage::Done`] without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proof(());

impl Proof {
    /// A proof when there is at least one check and every one is met.
    pub fn of(marks: impl IntoIterator<Item = Mark>) -> Option<Proof> {
        let mut any = false;
        for mark in marks {
            if mark != Mark::Met {
                return None;
            }
            any = true;
        }
        any.then_some(Proof(()))
    }
}

/// What happens to a finding, read from the wire or from the user's triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The user files a suggestion.
    Accept,
    /// Someone is on it.
    Assign,
    /// A fix exists together with what would prove it.
    Propose,
    Pass(Proof),
    /// A check failed: a proving fix goes back to fixing, a closed finding reopens.
    Fail,
    /// It becomes a standing check's to watch.
    Watch,
}

const PASS: Event = Event::Pass(Proof(()));

/// Every move a finding can make. An event with no row from the current stage changes nothing.
pub const MOVES: &[(Stage, Event, Stage)] = &[
    (Stage::New, Event::Accept, Stage::Open),
    (Stage::Open, Event::Assign, Stage::Fixing),
    (Stage::Open, Event::Watch, Stage::Watching),
    (Stage::Fixing, Event::Propose, Stage::Proving),
    (Stage::Proving, PASS, Stage::Done),
    (Stage::Proving, Event::Fail, Stage::Fixing),
    (Stage::Watching, Event::Assign, Stage::Fixing),
    (Stage::Watching, PASS, Stage::Done),
    (Stage::Done, Event::Fail, Stage::Open),
];

/// Where a born finding stands once each event in order has been tried against [`MOVES`].
pub fn walk(born: Stage, events: impl IntoIterator<Item = Event>) -> Stage {
    events.into_iter().fold(born, |stage, event| {
        MOVES
            .iter()
            .find(|(from, on, _)| *from == stage && *on == event)
            .map_or(stage, |(_, _, to)| *to)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    #[test]
    fn every_stage_is_reachable_from_a_suggestion() -> Check {
        let mut seen = BTreeSet::from([Stage::New]);
        let mut grew = true;
        while grew {
            let before = seen.len();
            for (from, _, to) in MOVES {
                if seen.contains(from) {
                    seen.insert(*to);
                }
            }
            grew = seen.len() > before;
        }
        assert_eq!(seen, BTreeSet::from(Stage::ALL));
        Ok(())
    }

    #[test]
    fn nothing_reaches_done_but_a_passed_proof() -> Check {
        for (from, event, to) in MOVES {
            if *to == Stage::Done {
                assert!(matches!(event, Event::Pass(_)), "{from:?} skips proof");
            }
        }
        assert_eq!(Proof::of([]), None, "no check proves nothing");
        assert_eq!(Proof::of([Mark::Met, Mark::Waiting]), None);
        assert!(Proof::of([Mark::Met, Mark::Met]).is_some());
        Ok(())
    }

    #[test]
    fn each_move_lands_where_the_record_says() -> Check {
        let pass = Event::Pass(Proof::of([Mark::Met]).ok_or("a met check proves")?);
        let cases = [
            (Stage::New, vec![Event::Accept], Stage::Open),
            (Stage::Open, vec![Event::Assign], Stage::Fixing),
            (Stage::Open, vec![Event::Watch], Stage::Watching),
            (Stage::Fixing, vec![Event::Propose], Stage::Proving),
            (Stage::Proving, vec![pass], Stage::Done),
            (Stage::Proving, vec![Event::Fail], Stage::Fixing),
            (Stage::Watching, vec![Event::Assign], Stage::Fixing),
            (Stage::Watching, vec![pass], Stage::Done),
            (Stage::Done, vec![Event::Fail], Stage::Open),
            // A suggestion the user never accepted stays one, whatever the agent did.
            (
                Stage::New,
                vec![Event::Assign, Event::Propose, pass],
                Stage::New,
            ),
            (Stage::Open, vec![pass], Stage::Open),
            (Stage::Fixing, vec![pass], Stage::Fixing),
        ];
        for (born, events, want) in cases {
            assert_eq!(walk(born, events.clone()), want, "{born:?} {events:?}");
        }
        Ok(())
    }
}

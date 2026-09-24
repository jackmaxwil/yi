//! `--deadline` as one clock: the environment line, the tool cancel and the stop all read it.

use std::time::{Duration, Instant};

/// The longest request measured ran about 170 s; one started inside the margin must finish.
const STOP_MARGIN: Duration = Duration::from_secs(240);
const LAST_WORD_CAP: Duration = Duration::from_secs(90);
const SHUTDOWN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    pub(crate) started: Instant,
    pub(crate) total: Duration,
}

impl Deadline {
    pub(super) fn new(total: Duration) -> Self {
        Self {
            started: Instant::now(),
            total,
        }
    }

    pub(super) fn passed(self, margin: Duration) -> bool {
        self.started.elapsed().saturating_add(margin) >= self.total
    }

    /// No turn starts in the last `STOP_MARGIN`; a short budget keeps three quarters of itself.
    pub(crate) fn winding_down(self) -> bool {
        self.passed(STOP_MARGIN.min(self.total / 4))
    }

    /// A running tool is cancelled this long before the deadline, so the last word lands in it.
    pub(super) fn last_word_due(self, longest_turn: Option<Duration>) -> bool {
        let cap = LAST_WORD_CAP.min(self.total / 8);
        let grace = longest_turn.map_or(cap, |turn| turn.saturating_add(SHUTDOWN).min(cap));
        self.passed(grace)
    }
}

impl super::Shared {
    pub(super) fn last_word_due(&self) -> bool {
        let longest = self.turn_time.lock().ok().and_then(|clock| clock.1);
        self.deadline
            .get()
            .is_some_and(|clock| clock.last_word_due(longest))
    }

    pub(super) fn time_turn(&self, event: &yi_types::event::AgentEvent) {
        use yi_types::event::AgentEvent;
        let Ok(mut clock) = self.turn_time.lock() else {
            return;
        };
        match event {
            AgentEvent::TurnStart => clock.0 = Some(Instant::now()),
            AgentEvent::MessageEnd {
                message: yi_types::message::AgentMessage::Assistant { .. },
            } => {
                if let Some(started) = clock.0.take() {
                    clock.1 = clock.1.max(Some(started.elapsed()));
                }
            }
            _ => {}
        }
    }

    /// The loop's check at its message boundary: out of clock, or cancelled by the parent.
    pub(super) fn winding_down(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
            || self
                .deadline
                .get()
                .is_some_and(|clock| clock.winding_down())
    }
}

impl super::AgentSession {
    /// A `cancel` from the parent: the turn in flight settles and no request follows it.
    pub(crate) fn cancel(&self) {
        let flag = &self.shared.cancelled;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.shared
            .cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begun(ago: u64, total: u64) -> Deadline {
        Deadline {
            started: Instant::now()
                .checked_sub(Duration::from_secs(ago))
                .unwrap(),
            total: Duration::from_secs(total),
        }
    }

    /// A flat 240 s margin is a 120 s fixture's whole budget: one turn per run.
    #[test]
    fn the_stop_margin_is_four_minutes_or_a_quarter_of_the_budget() {
        assert!(!begun(80, 120).winding_down());
        assert!(begun(91, 120).winding_down());
        assert!(!begun(750, 1000).winding_down());
        assert!(begun(761, 1000).winding_down());
        assert!(!begun(761, 1000).passed(Duration::ZERO));
    }

    /// Dies with the last word started at the deadline itself: `yi ask`'s caller killed the
    /// run as the tool-free turn began, so the answer and the lane release were lost.
    #[test]
    fn the_last_word_reserves_the_longest_turn_and_the_shutdown_capped() {
        let turn = Some(Duration::from_secs(20));
        assert!(!begun(969, 1000).last_word_due(turn));
        assert!(begun(971, 1000).last_word_due(turn));
        assert!(
            begun(911, 1000).last_word_due(None),
            "unmeasured reserves the 90 s cap"
        );
        assert!(!begun(909, 1000).last_word_due(None));
        assert!(
            begun(106, 120).last_word_due(turn),
            "a short budget caps it at an eighth"
        );
    }
}

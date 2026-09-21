//! `--deadline` as one clock: the environment line, the tool cancel and the stop all read it.

use std::time::{Duration, Instant};

/// The longest request measured, a settled reasoning cut, ran about 170 s; one that starts
/// inside the margin still has to finish before harbor's kill.
const STOP_MARGIN: Duration = Duration::from_secs(240);

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

    /// No turn starts in the last `STOP_MARGIN`; a short budget (a 120 s fixture) keeps
    /// three quarters of itself instead of one turn.
    pub(crate) fn winding_down(self) -> bool {
        self.passed(STOP_MARGIN.min(self.total / 4))
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
}

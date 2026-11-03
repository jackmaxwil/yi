use yi_types::goal::GoalStatus;
use yi_types::schedule::DeliveryMode;

use super::{GoalService, goal_prompt_message};

/// Continuations that changed nothing, in a row, before the goal is blocked.
pub const QUIET_RUNS_CAP: u32 = 3;

fn released_text(runs: u32) -> String {
    format!(
        "[\u{2026} goal continuation stopped after {runs} runs that changed nothing (cap {QUIET_RUNS_CAP}); the goal is blocked, so no continuation follows]. Finish it with goal.update(status=\"complete\") once the objective holds, or restate it with goal.objective and a user:// citation to continue."
    )
}

impl GoalService {
    /// Invariant: continuations that change nothing are counted, and three in a row release the goal.
    pub(super) fn settle(&self, reason: Option<&yi_types::event::RunEnd>) {
        let continuing = self
            .continuing
            .lock()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or(false);
        if continuing {
            use yi_types::event::RunEnd;
            let quiet = match reason {
                Some(RunEnd::Error) => true,
                None | Some(RunEnd::Stop | RunEnd::Drained) => !Self::flag(&self.called),
                Some(RunEnd::Aborted | RunEnd::Length | RunEnd::Guard | RunEnd::Other(_)) => {
                    return self.continue_if_idle();
                }
            };
            let runs = self.quiet_run(quiet);
            if runs >= QUIET_RUNS_CAP {
                self.release(runs);
                return;
            }
        }
        self.continue_if_idle();
    }

    fn quiet_run(&self, quiet: bool) -> u32 {
        self.quiet
            .lock()
            .map(|mut runs| {
                *runs = if quiet { runs.saturating_add(1) } else { 0 };
                *runs
            })
            .unwrap_or(0)
    }

    fn release(&self, runs: u32) {
        let Some(mut goal) = self.read_goal() else {
            return;
        };
        if !goal.status.is_active() {
            return;
        }
        goal.status = GoalStatus::Blocked;
        goal.updated = yi_session::now_ms();
        if self.write_goal(goal).is_err() {
            return;
        }
        let _ = self.quiet.lock().map(|mut slot| *slot = 0);
        (self.deliver)(
            goal_prompt_message(released_text(runs)),
            DeliveryMode::Steer,
        );
    }
}

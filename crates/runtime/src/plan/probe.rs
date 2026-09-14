use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::doc::{BlockedOn, Plan, PlanId, PlanState, TodoLabel, TodoState};
use yi_types::schedule::DeliveryMode;

use super::ops::{Actor, Op, OpRequest, PlanEngine};
use super::store::PlanStore;
use crate::goal::DeliverFn;

pub const FIRST_DELAY: Duration = Duration::from_secs(60);

/// The ceiling, and the interval a probeless block nudges its owner on.
pub const MAX_DELAY: Duration = Duration::from_secs(30 * 60);

/// Nothing is due with no block open, so the loop still wakes often enough to
/// notice one that opened between its own ticks.
const IDLE_POLL: Duration = Duration::from_secs(60);

const PROBE_TIMEOUT_MS: u64 = 30_000;
const SATURATED_SHIFT: u32 = 5;

pub type ProbeRun = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Invariant: the ladder saturates rather than growing without bound, so a condition nobody
/// satisfies costs one check per [`MAX_DELAY`], not one per turn or an overflowing interval.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rung(u32);

impl Rung {
    /// Incident: `checked_shl` refuses only a shift of 64 or more, so rungs 62
    /// and 63 shifted every bit out and read as a zero-second delay.
    pub fn delay(self) -> Duration {
        let seconds = FIRST_DELAY.as_secs() << self.0.min(SATURATED_SHIFT);
        Duration::from_secs(seconds.min(MAX_DELAY.as_secs()))
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

fn key(plan: &PlanId, label: &TodoLabel) -> String {
    format!("{plan}/{label}")
}

/// Kept as data so a tick's decision is testable without a clock or a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Unblocked { label: TodoLabel },
    Retry { label: TodoLabel, rung: Rung },
    Nudged { label: TodoLabel },
}

struct Pending {
    rung: Rung,
    due: Instant,
}

/// §5's saturating ladder: a `Blocked{on: External}` todo whose probe passes is unblocked by
/// the host; one with no probe nudges its owner, since an unexamined block never returns.
pub struct ProbeLadder {
    engine: Arc<PlanEngine>,
    plans_dir: PathBuf,
    deliver: DeliverFn,
    run: ProbeRun,
    pending: Mutex<HashMap<String, Pending>>,
}

impl ProbeLadder {
    pub fn new(engine: Arc<PlanEngine>, plans_dir: PathBuf, deliver: DeliverFn) -> Self {
        Self {
            engine,
            plans_dir,
            deliver,
            run: Arc::new(|command| crate::goal::run_check(command, PROBE_TIMEOUT_MS)),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_run(mut self, run: ProbeRun) -> Self {
        self.run = run;
        self
    }

    fn plans(&self) -> Vec<Plan> {
        let Ok(store) = PlanStore::open(self.plans_dir.clone()) else {
            return Vec::new();
        };
        store
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| store.read(&id).ok())
            .filter(|plan| plan.state == PlanState::Active)
            .collect()
    }

    /// Invariant: due times live only in memory, so a resumed session restarts every ladder
    /// at the first rung; a probe is cheap and is not the runaway the spawn fuse guards.
    pub fn tick(&self, now: Instant) -> Vec<Verdict> {
        let mut verdicts = Vec::new();
        let mut live = Vec::new();
        for plan in self.plans() {
            for todo in &plan.todos {
                let TodoState::Blocked {
                    on: BlockedOn::External { probe },
                    note: _,
                } = &todo.state
                else {
                    continue;
                };
                let slot = key(&plan.id, &todo.label);
                live.push(slot.clone());
                if !self.due(&slot, now) {
                    continue;
                }
                let verdict = match probe {
                    Some(command) => match (self.run)(command.as_str()) {
                        Ok(()) if self.unblock(&plan.id, &todo.label) => Verdict::Unblocked {
                            label: todo.label.clone(),
                        },
                        Ok(()) => Verdict::Retry {
                            label: todo.label.clone(),
                            rung: self.climb(&slot, now),
                        },
                        Err(_red) => Verdict::Retry {
                            label: todo.label.clone(),
                            rung: self.climb(&slot, now),
                        },
                    },
                    None => {
                        self.nudge(&slot, now, &todo.label);
                        Verdict::Nudged {
                            label: todo.label.clone(),
                        }
                    }
                };
                verdicts.push(verdict);
            }
        }
        self.forget_all_but(&live);
        verdicts
    }

    /// A slot seen for the first time waits one full rung before its first run,
    /// so blocking a todo never runs its probe inside the same turn.
    fn due(&self, slot: &str, now: Instant) -> bool {
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        let entry = pending.entry(slot.to_owned()).or_insert_with(|| Pending {
            rung: Rung::default(),
            due: now.checked_add(FIRST_DELAY).unwrap_or(now),
        });
        entry.due <= now
    }

    fn climb(&self, slot: &str, now: Instant) -> Rung {
        let Ok(mut pending) = self.pending.lock() else {
            return Rung::default();
        };
        let Some(entry) = pending.get_mut(slot) else {
            return Rung::default();
        };
        entry.rung = entry.rung.next();
        entry.due = now.checked_add(entry.rung.delay()).unwrap_or(now);
        entry.rung
    }

    fn nudge(&self, slot: &str, now: Instant, label: &TodoLabel) {
        if let Ok(mut pending) = self.pending.lock()
            && let Some(entry) = pending.get_mut(slot)
        {
            entry.rung = entry.rung.next();
            entry.due = now.checked_add(MAX_DELAY).unwrap_or(now);
        }
        self.say(format!(
            "external block {label} carries no probe, so nothing can clear it \
             automatically. Check the condition and unblock it, or record why it stands."
        ));
    }

    /// Incident: a refused unblock left the slot due, so the loop re-ran the
    /// probe and re-delivered the refusal every second until the lease freed.
    fn unblock(&self, plan: &PlanId, label: &TodoLabel) -> bool {
        let request = OpRequest {
            plan: Some(plan.clone()),
            actor: Actor::Host,
            op: Op::Unblock {
                label: label.clone(),
            },
            request_id: None,
            expected_revision: None,
        };
        match self.engine.apply(request) {
            Ok(_) => {
                self.say(format!(
                    "external block {label} cleared: its probe passed, so the todo is ready again."
                ));
                true
            }
            Err(refused) => {
                self.say(format!(
                    "external block {label} passed its probe but could not be unblocked: {refused}"
                ));
                false
            }
        }
    }

    fn say(&self, text: String) {
        (self.deliver)(
            AgentMessage::Custom {
                custom_type: "plan_probe".to_owned(),
                content: UserContent::Text(text),
                display: true,
                details: None,
                timestamp: yi_session::now_ms(),
            },
            DeliveryMode::Steer,
        );
    }

    fn forget_all_but(&self, live: &[String]) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|slot, _| live.contains(slot));
        }
    }

    fn next_wake(&self, now: Instant) -> Duration {
        self.pending
            .lock()
            .ok()
            .and_then(|pending| pending.values().map(|entry| entry.due).min())
            .map_or(IDLE_POLL, |due| {
                due.saturating_duration_since(now).min(IDLE_POLL)
            })
    }
}

/// The ladder runs off the wall clock, not off turns: a session whose loop has
/// gone quiet on a `Cadence` posture is exactly the one waiting on a probe.
pub fn spawn(ladder: Arc<ProbeLadder>) {
    tokio::spawn(async move {
        loop {
            let wake = ladder.next_wake(Instant::now()).max(Duration::from_secs(1));
            tokio::time::sleep(wake).await;
            let ticking = Arc::clone(&ladder);
            if tokio::task::spawn_blocking(move || ticking.tick(Instant::now()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_doubles_from_a_minute_and_saturates_at_half_an_hour() {
        let mut rung = Rung::default();
        let mut seen = Vec::new();
        for _ in 0..12 {
            seen.push(rung.delay().as_secs());
            rung = rung.next();
        }
        assert_eq!(
            seen,
            vec![
                60, 120, 240, 480, 960, 1800, 1800, 1800, 1800, 1800, 1800, 1800
            ]
        );
    }

    #[test]
    fn a_rung_far_past_the_ceiling_still_names_the_ceiling() {
        for rung in [6, 31, 62, 63, 64, 4_000_000_000] {
            assert_eq!(Rung(rung).delay(), MAX_DELAY, "rung {rung}");
        }
    }
}

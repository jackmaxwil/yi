use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::plan::doc::{BlockedOn, Plan, PlanId, PlanState, TodoLabel, TodoState};
use yi_types::schedule::DeliveryMode;

use super::ops::{Actor, Op, OpRequest, PlanEngine};
use super::store::PlanStore;
use crate::family::{MemberView, StuckLatch};
use crate::goal::DeliverFn;
use crate::subagent::NoticeFn;

pub const FIRST_DELAY: Duration = Duration::from_secs(60);

/// The ceiling, and the interval a probeless block nudges its owner on.
pub const MAX_DELAY: Duration = Duration::from_secs(30 * 60);

/// Nothing is due with no block open, so the loop still wakes often enough to notice one
/// that opened between its ticks; the stuck job runs on every wake, so this bounds it too.
const IDLE_POLL: Duration = Duration::from_secs(60);

const PROBE_TIMEOUT_MS: u64 = 30_000;
const SATURATED_SHIFT: u32 = 5;

pub type ProbeRun = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
pub type Children = Arc<dyn Fn() -> Vec<MemberView> + Send + Sync>;

/// Invariant: the ladder saturates rather than growing without bound, so a condition nobody
/// satisfies costs one check per [`MAX_DELAY`], not one per turn or an overflowing interval.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rung(u32);

impl Rung {
    /// Incident: `checked_shl` refuses only a shift of 64 or more, so rungs 62
    /// and 63 shifted every bit out and read as a zero-second delay.
    pub fn delay(self) -> Duration {
        let levers = crate::levers::get();
        let seconds = levers.plan_probe_first_s << self.0.min(SATURATED_SHIFT);
        Duration::from_secs(seconds.min(levers.plan_probe_max_s))
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
    clock: Clock,
    pending: Mutex<HashMap<String, Pending>>,
    /// Every due time registered from outside the ladder, each cleared once its tick has run:
    /// an earlier registration never forgets a later one.
    due_at: Mutex<BTreeSet<Instant>>,
    /// Stores one permit, so a `notify_one` before the loop parks is not lost.
    wake: Notify,
    /// A probe tick in flight; the loop never waits on it, and the tick never wakes the loop
    /// when it ends, since a stored permit would run the next tick at once: a spin.
    probing: AtomicBool,
    children: Option<(Children, Arc<NoticeFn>)>,
    latch: Mutex<StuckLatch>,
    /// The lease timer's job (plan section 7.4): run on every wake, before the probe tick is
    /// even looked at, so a probe still in flight never delays a cancel's expiry.
    leases: Option<Arc<dyn Fn() + Send + Sync>>,
    owned: Option<Arc<dyn Fn() -> Vec<PlanId> + Send + Sync>>,
}

impl ProbeLadder {
    pub fn new(engine: Arc<PlanEngine>, plans_dir: PathBuf, deliver: DeliverFn) -> Self {
        Self {
            engine,
            plans_dir,
            deliver,
            run: Arc::new(|command| crate::goal::run_check(command, PROBE_TIMEOUT_MS)),
            clock: Arc::new(Instant::now),
            pending: Mutex::new(HashMap::new()),
            due_at: Mutex::new(BTreeSet::new()),
            wake: Notify::new(),
            probing: AtomicBool::new(false),
            children: None,
            latch: Mutex::new(StuckLatch::default()),
            leases: None,
            owned: None,
        }
    }

    pub fn with_owned(mut self, owned: Arc<dyn Fn() -> Vec<PlanId> + Send + Sync>) -> Self {
        self.owned = Some(owned);
        self
    }

    pub fn with_run(mut self, run: ProbeRun) -> Self {
        self.run = run;
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The stuck job's source and its notice path: the running children as their records
    /// show them, and the host's lifecycle notice that wakes the parent.
    pub fn with_children(mut self, children: Children, notice: Arc<NoticeFn>) -> Self {
        self.children = Some((children, notice));
        self
    }

    pub fn with_leases(mut self, expire: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.leases = Some(expire);
        self
    }

    /// The injected clock's now, for a caller turning a grace into a due time.
    pub fn now(&self) -> Instant {
        (self.clock)()
    }

    /// Registers a due time; one earlier than every other registered interrupts the loop's
    /// sleep. Later ones are kept for their own turn.
    pub fn wake_at(&self, due: Instant) {
        let earliest = match self.due_at.lock() {
            Ok(mut dues) => {
                let earliest = dues.first().is_none_or(|first| due < *first);
                dues.insert(due);
                earliest
            }
            Err(_) => false,
        };
        if earliest {
            self.wake.notify_one();
        }
    }

    /// The registered due times still to come.
    pub fn due_times(&self) -> usize {
        self.due_at.lock().map(|dues| dues.len()).unwrap_or(0)
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

    /// The registered due times that have come are taken by the tick that serves them; a wake
    /// that finds a tick still in flight leaves them armed for the next one.
    fn take_due(&self, now: Instant) {
        if let Ok(mut dues) = self.due_at.lock() {
            dues.retain(|due| *due > now);
        }
    }

    /// Invariant: due times live only in memory, so a resumed session restarts every ladder
    /// at the first rung; a probe is cheap and is not the runaway the spawn fuse guards.
    pub fn tick(&self, now: Instant) -> Vec<Verdict> {
        self.take_due(now);
        if let Some(owned) = &self.owned {
            self.engine.dispatch_ready_in(&owned());
        }
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

    /// The stuck job: one `[child <name> stuck: <note>]` per episode, from the records the
    /// loop already writes (D165). Returns the notices it sent.
    pub fn watch(&self) -> Vec<String> {
        let Some((children, notice)) = &self.children else {
            return Vec::new();
        };
        let views = children();
        let Ok(mut latch) = self.latch.lock() else {
            return Vec::new();
        };
        let notices = latch.notices(&views);
        drop(latch);
        for text in &notices {
            notice(text);
        }
        notices
    }

    /// A slot seen for the first time waits one full rung before its first run,
    /// so blocking a todo never runs its probe inside the same turn.
    fn due(&self, slot: &str, now: Instant) -> bool {
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        let entry = pending.entry(slot.to_owned()).or_insert_with(|| Pending {
            rung: Rung::default(),
            due: now.checked_add(Rung::default().delay()).unwrap_or(now),
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
            let max = Duration::from_secs(crate::levers::get().plan_probe_max_s);
            entry.due = now.checked_add(max).unwrap_or(now);
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

    /// A slot due while a tick is in flight wakes the loop on the one-second floor, where
    /// `wake_once` finds the tick still running and returns; the climb sizes the next wake.
    fn next_wake(&self, now: Instant) -> Duration {
        let probes = self
            .pending
            .lock()
            .ok()
            .and_then(|pending| pending.values().map(|entry| entry.due).min());
        let registered = self
            .due_at
            .lock()
            .ok()
            .and_then(|dues| dues.first().copied());
        [probes, registered]
            .into_iter()
            .flatten()
            .min()
            .map_or(IDLE_POLL, |due| {
                due.saturating_duration_since(now).min(IDLE_POLL)
            })
    }

    /// One wake: the stuck job and the probes each on a blocking thread the loop does not wait
    /// for, so a slow probe never delays a stuck check and neither stalls the reactor.
    fn wake_once(self: &Arc<Self>) {
        let now = (self.clock)();
        if let Some(expire) = &self.leases {
            expire();
        }
        let watching = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            watching.watch();
        });
        if self.probing.swap(true, Ordering::SeqCst) {
            // The due times stay armed for the tick that follows the one in flight.
            return;
        }
        // Taken on the loop's thread before the tick is spawned, so the next sleep never
        // reads a due time the tick has not cleared yet.
        self.take_due(now);
        let ticking = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            ticking.tick(now);
            ticking.probing.store(false, Ordering::SeqCst);
        });
    }
}

/// The ladder runs off the wall clock, not off turns; the sleep is raced against the
/// `Notify`, so an earlier due time registered mid-sleep runs at its own time (section 7.4).
pub fn spawn(ladder: Arc<ProbeLadder>) {
    tokio::spawn(async move {
        loop {
            let wake = ladder
                .next_wake((ladder.clock)())
                .max(Duration::from_secs(1));
            tokio::select! {
                () = tokio::time::sleep(wake) => {}
                () = ladder.wake.notified() => {}
            }
            ladder.wake_once();
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

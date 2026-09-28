use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use yi_types::plan::doc::PlanId;

use super::ops::PlanEngine;
use crate::family::{MemberView, StuckLatch};
use crate::subagent::NoticeFn;

/// With no due time registered the loop still wakes this often, since the stuck job and the
/// dispatch backstop run on every wake.
const IDLE_POLL: Duration = Duration::from_secs(60);

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
pub type Children = Arc<dyn Fn() -> Vec<MemberView> + Send + Sync>;
pub type Owned = Arc<dyn Fn() -> Vec<PlanId> + Send + Sync>;

/// The plan's host timer: each wake runs the lease job, the stuck job and the dispatch backstop.
/// A blocked todo's wait is a channel subscription, never this loop's (D287).
pub struct PlanTimer {
    engine: Arc<PlanEngine>,
    clock: Clock,
    /// Every due time registered from outside, each cleared once its tick has run: an earlier
    /// registration never forgets a later one.
    due_at: Mutex<BTreeSet<Instant>>,
    /// Stores one permit, so a `notify_one` before the loop parks is not lost.
    wake: Notify,
    /// A backstop tick in flight; the loop never waits on it, and the tick never wakes the loop
    /// when it ends, since a stored permit would run the next tick at once: a spin.
    ticking: AtomicBool,
    children: Option<(Children, Arc<NoticeFn>)>,
    latch: Mutex<StuckLatch>,
    /// The lease timer's job (plan section 7.4): run on every wake, before the backstop tick is
    /// even looked at, so a tick still in flight never delays a cancel's expiry.
    leases: Option<Arc<dyn Fn() + Send + Sync>>,
    owned: Option<Owned>,
}

impl PlanTimer {
    pub fn new(engine: Arc<PlanEngine>) -> Self {
        Self {
            engine,
            clock: Arc::new(Instant::now),
            due_at: Mutex::new(BTreeSet::new()),
            wake: Notify::new(),
            ticking: AtomicBool::new(false),
            children: None,
            latch: Mutex::new(StuckLatch::default()),
            leases: None,
            owned: None,
        }
    }

    pub fn with_owned(mut self, owned: Owned) -> Self {
        self.owned = Some(owned);
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

    /// The registered due times that have come are taken by the tick that serves them; a wake
    /// that finds a tick still in flight leaves them armed for the next one.
    fn take_due(&self, now: Instant) {
        if let Ok(mut dues) = self.due_at.lock() {
            dues.retain(|due| *due > now);
        }
    }

    /// The backstop: starts the ready todos of the roots this session's ledger names.
    pub fn tick(&self, now: Instant) {
        self.take_due(now);
        if let Some(owned) = &self.owned {
            self.engine.dispatch_ready_in(&owned());
        }
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
            notice(text, None);
        }
        notices
    }

    fn next_wake(&self, now: Instant) -> Duration {
        self.due_at
            .lock()
            .ok()
            .and_then(|dues| dues.first().copied())
            .map_or(IDLE_POLL, |due| {
                due.saturating_duration_since(now).min(IDLE_POLL)
            })
    }

    /// One wake: the stuck job and the backstop each on a blocking thread the loop does not
    /// wait for, so a slow tick never delays a stuck check and neither stalls the reactor.
    fn wake_once(self: &Arc<Self>) {
        let now = (self.clock)();
        if let Some(expire) = &self.leases {
            expire();
        }
        let watching = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            watching.watch();
        });
        if self.ticking.swap(true, Ordering::SeqCst) {
            // The due times stay armed for the tick that follows the one in flight.
            return;
        }
        // Taken on the loop's thread before the tick is spawned, so the next sleep never
        // reads a due time the tick has not cleared yet.
        self.take_due(now);
        let ticking = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            ticking.tick(now);
            ticking.ticking.store(false, Ordering::SeqCst);
        });
    }
}

/// The loop runs off the wall clock, not off turns; the sleep is raced against the `Notify`,
/// so an earlier due time registered mid-sleep runs at its own time (section 7.4).
pub fn spawn(timer: Arc<PlanTimer>) {
    tokio::spawn(async move {
        loop {
            let wake = timer.next_wake((timer.clock)()).max(Duration::from_secs(1));
            tokio::select! {
                () = tokio::time::sleep(wake) => {}
                () = timer.wake.notified() => {}
            }
            timer.wake_once();
        }
    });
}

//! Lane slots counted by purpose, so a worker cannot take the checkout its own verification
//! needs (plan section 7.6): the two shares are disjoint, so each holder counts only its own.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Lane slots held back from workers for one acceptance. Its candidate check and its staging
/// merge run in order, never together (section 6.6), so one slot serves both.
pub const VERIFICATION_RESERVE: u8 = 1;

/// How long a reservation waits for the reserve before it refuses: two candidates verified
/// at once take turns rather than one of them failing (plan section 6.3, bounded backoff).
pub const RESERVE_WAIT: Duration = Duration::from_secs(30);
const RESERVE_POLL: Duration = Duration::from_millis(100);

/// What a reservation is for. Charging verification against the worker share is exactly how
/// retained workers come to occupy every slot their own verification needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// A worker's own checkout: the lane a worktree child runs in.
    Worker,
    /// The candidate check and the staging merge of one acceptance.
    Verification,
}

impl Purpose {
    pub fn name(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Verification => "verification",
        }
    }
}

impl fmt::Display for Purpose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A refused reservation with the numbers that refused it: expose allocation, refuse
/// over-allocation, hide nothing (section 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{purpose} capacity is full: {held} of {cap} held")]
pub struct Exhausted {
    pub purpose: Purpose,
    pub held: u8,
    pub cap: u8,
}

#[derive(Debug, Default, Clone, Copy)]
struct Held {
    worker: u8,
    verification: u8,
}

impl Held {
    fn of(&mut self, purpose: Purpose) -> &mut u8 {
        match purpose {
            Purpose::Worker => &mut self.worker,
            Purpose::Verification => &mut self.verification,
        }
    }
}

/// The split of one lane pool. One mutex over both counters: they are read and moved together,
/// and a reservation is a handful of instructions, never an await.
#[derive(Debug)]
pub struct Capacity {
    worker_cap: u8,
    verification_cap: u8,
    held: Mutex<Held>,
}

impl Capacity {
    /// The split of a pool of `slots` lanes. A pool too small to split leaves each side one,
    /// and the pool's own `PoolFull` catches the overcommit.
    pub fn for_slots(slots: u8) -> Arc<Self> {
        Arc::new(Self {
            worker_cap: slots.saturating_sub(VERIFICATION_RESERVE).max(1),
            verification_cap: VERIFICATION_RESERVE,
            held: Mutex::new(Held::default()),
        })
    }

    pub fn cap(&self, purpose: Purpose) -> u8 {
        match purpose {
            Purpose::Worker => self.worker_cap,
            Purpose::Verification => self.verification_cap,
        }
    }

    pub fn held(&self, purpose: Purpose) -> u8 {
        *self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .of(purpose)
    }

    /// # Errors
    /// [`Exhausted`], naming which counter ran out and at what count.
    pub fn reserve(self: &Arc<Self>, purpose: Purpose) -> Result<Permit, Exhausted> {
        let cap = self.cap(purpose);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.of(purpose);
        if *count >= cap {
            return Err(Exhausted {
                purpose,
                held: *count,
                cap,
            });
        }
        *count = count.saturating_add(1);
        drop(held);
        Ok(Permit {
            capacity: Arc::clone(self),
            purpose,
        })
    }

    /// `reserve` with the bounded backoff of section 6.3: a full counter is waited out until
    /// `until`, then refused as [`Exhausted`].
    pub fn reserve_within(
        self: &Arc<Self>,
        purpose: Purpose,
        until: Instant,
    ) -> Result<Permit, Exhausted> {
        loop {
            match self.reserve(purpose) {
                Ok(permit) => return Ok(permit),
                Err(_full) if Instant::now() < until => std::thread::sleep(RESERVE_POLL),
                Err(exhausted) => return Err(exhausted),
            }
        }
    }

    fn release(&self, purpose: Purpose) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.of(purpose);
        *count = count.saturating_sub(1);
    }
}

/// One held slot. Dropping it returns the slot, so a reservation cannot outlive the checkout
/// it was taken for, even on an early return.
#[must_use = "dropping the permit returns the slot at once"]
pub struct Permit {
    capacity: Arc<Capacity>,
    purpose: Purpose,
}

impl Permit {
    pub fn purpose(&self) -> Purpose {
        self.purpose
    }
}

impl fmt::Debug for Permit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Permit")
            .field("purpose", &self.purpose)
            .finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.capacity.release(self.purpose);
    }
}

impl super::ops::PlanEngine {
    pub fn with_lane_home(self, home: std::path::PathBuf, slots: u8) -> Self {
        Self {
            lane_home: Some((home, slots)),
            ..self
        }
    }

    pub(super) fn pool(
        &self,
        label: &yi_types::plan::doc::TodoLabel,
    ) -> Result<&crate::lane::Pool, super::ops::PlanOpError> {
        use super::acceptance::verification;
        if let Some(pool) = self.lanes.get() {
            return Ok(pool);
        }
        let no_pool = || verification(label, "no lane pool is attached to the engine");
        let (home, slots) = self.lane_home.as_ref().ok_or_else(no_pool)?;
        let pool = crate::lane::Pool::open(home, &self.cwd, *slots)
            .map_err(|error| verification(label, error.to_string()))?;
        Ok(self.lanes.get_or_init(|| pool))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::plan::doc::{AgentId, Delegation, TodoAddr, TodoLabel};
    use yi_types::url::Url;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Dies with the reserve coming off the worker share: give workers the whole pool and the
    /// third lane of a three-slot pool is a worker's, not the verification's.
    #[test]
    fn workers_stop_at_the_share_the_reserve_leaves() -> TestResult {
        let capacity = Capacity::for_slots(3);
        assert_eq!(capacity.cap(Purpose::Worker), 2);
        let _first = capacity.reserve(Purpose::Worker)?;
        let _second = capacity.reserve(Purpose::Worker)?;
        assert_eq!(
            capacity.reserve(Purpose::Worker).err(),
            Some(Exhausted {
                purpose: Purpose::Worker,
                held: 2,
                cap: 2,
            })
        );
        assert_eq!(capacity.held(Purpose::Worker), 2);
        Ok(())
    }

    /// Dies with the verification counter being its own: charge it against the worker share and
    /// a full worker share leaves nothing for the candidate check.
    #[test]
    fn verification_stops_at_the_reserve_with_the_worker_share_full() -> TestResult {
        let capacity = Capacity::for_slots(3);
        let mut workers = Vec::new();
        for _ in 0..capacity.cap(Purpose::Worker) {
            workers.push(capacity.reserve(Purpose::Worker)?);
        }
        let verifying = capacity.reserve(Purpose::Verification)?;
        assert_eq!(verifying.purpose(), Purpose::Verification);
        assert_eq!(
            capacity.reserve(Purpose::Verification).err(),
            Some(Exhausted {
                purpose: Purpose::Verification,
                held: 1,
                cap: 1,
            })
        );
        drop(verifying);
        assert_eq!(capacity.held(Purpose::Verification), 0);
        let _again = capacity.reserve(Purpose::Verification)?;
        Ok(())
    }

    /// A one-slot pool cannot be split; each side keeps one rather than the workers keeping
    /// none, and the pool's own refusal is what catches the overcommit.
    #[test]
    fn a_pool_too_small_to_split_leaves_each_side_one() {
        let capacity = Capacity::for_slots(1);
        assert_eq!(capacity.cap(Purpose::Worker), 1);
        assert_eq!(capacity.cap(Purpose::Verification), 1);
    }

    struct NoChildren;

    impl super::super::ops::Delegate for NoChildren {
        fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
            Err("no children here".to_owned())
        }

        fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
            Ok(None)
        }
    }

    /// Dies with the pool opened only at construction: a repository initialised after the
    /// engine was built never gets a checkout, and its candidates are never verified.
    #[test]
    fn the_lane_pool_opens_at_the_first_checkout_that_needs_it() -> TestResult {
        let root = crate::scratch::Scratch::new("yi-capacity-lazy-pool")?;
        let work = root.join("work");
        std::fs::create_dir_all(&work)?;
        let store = super::super::store::PlanStore::open(root.join("plans"))?;
        let engine = super::super::ops::PlanEngine::new(store, Arc::new(NoChildren))
            .with_cwd(work.clone())
            .with_lane_home(root.join("home"), 2);
        let label = TodoLabel::new("alpha")?;
        assert!(engine.pool(&label).is_err(), "no repository yet");
        crate::lane::git(&work, &["init", "-q"])?;
        assert!(
            engine.pool(&label).is_ok(),
            "the repository appeared mid-session"
        );
        Ok(())
    }
}

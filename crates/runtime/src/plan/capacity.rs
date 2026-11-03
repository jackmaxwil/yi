//! Lanes counted by purpose. Verification is the one purpose left: a worktree child forks a tree of
//! its own, so a worker can no longer take the checkout its own verification needs.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

/// What a reservation is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// The candidate check and the staging merge of one acceptance.
    Verification,
}

impl Purpose {
    pub fn name(self) -> &'static str {
        match self {
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
    verification: u8,
}

impl Held {
    fn of(&mut self, purpose: Purpose) -> &mut u8 {
        match purpose {
            Purpose::Verification => &mut self.verification,
        }
    }
}

/// The split of one lane pool. One mutex over both counters: they are read and moved together,
/// and a reservation is a handful of instructions, never an await.
#[derive(Debug)]
pub struct Capacity {
    verification_cap: u8,
    held: Mutex<Held>,
    released: Condvar,
}

impl Capacity {
    /// Incident: one verification lane bounced each concurrent acceptance outlasting its wait,
    /// so a quarter of the pool, one to four lanes, verifies; the pool refuses overcommit.
    pub fn for_slots(slots: u8) -> Arc<Self> {
        let verification_cap = (slots / 4).clamp(1, 4);
        Arc::new(Self {
            verification_cap,
            held: Mutex::new(Held::default()),
            released: Condvar::new(),
        })
    }

    pub fn cap(&self, purpose: Purpose) -> u8 {
        match purpose {
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
        let _span = yi_types::trace::span("wait.capacity").arg("purpose", purpose.name());
        loop {
            match self.reserve(purpose) {
                Ok(permit) => return Ok(permit),
                Err(exhausted) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(exhausted);
                    }
                    let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
                    let full = |held: &mut Held| *held.of(purpose) >= self.cap(purpose);
                    let _woken = self.released.wait_timeout_while(held, left, full);
                }
            }
        }
    }

    fn release(&self, purpose: Purpose) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.of(purpose);
        *count = count.saturating_sub(1);
        self.released.notify_all();
    }
}

/// One held slot. Dropping it returns the slot, so a reservation cannot outlive the checkout
/// it was taken for, even on an early return.
#[must_use = "dropping the permit returns the slot at once"]
pub struct Permit {
    capacity: Arc<Capacity>,
    purpose: Purpose,
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
    use std::time::Duration;
    use yi_types::plan::doc::{AgentId, Delegation, TodoAddr, TodoLabel};
    use yi_types::url::Url;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn verification_stops_at_the_reserve() -> TestResult {
        let capacity = Capacity::for_slots(3);
        let verifying = capacity.reserve(Purpose::Verification)?;
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

    /// Dies with the 100 ms poll back: each reservation waited out a poll after the release,
    /// so ten handoffs took a second or more.
    #[test]
    fn a_waiting_reservation_takes_the_slot_as_it_is_released() -> TestResult {
        let capacity = Capacity::for_slots(1);
        let started = Instant::now();
        for _ in 0..10 {
            let held = capacity.reserve(Purpose::Verification)?;
            let waiter = Arc::clone(&capacity);
            let until = Instant::now() + Duration::from_secs(30);
            let taken =
                std::thread::spawn(move || waiter.reserve_within(Purpose::Verification, until));
            std::thread::sleep(Duration::from_millis(5));
            drop(held);
            drop(taken.join().map_err(|_| "waiter panicked")??);
        }
        let took = started.elapsed();
        assert!(
            took < Duration::from_millis(500),
            "ten handoffs took {took:?}"
        );
        Ok(())
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

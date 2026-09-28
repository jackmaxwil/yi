//! The plan's host timer: the lease job, the stuck job and the dispatch backstop run on every
//! wake, and a due time registered mid-sleep wakes the loop at its own time.
//!
//! F0d, the wake. Plan section 7.4 finding R9: `next_wake` caps the sleep at 60 seconds and
//! a due time registered mid-sleep waits it out, so a grace that fires in a second is a
//! claim the loop cannot keep. A `tokio::sync::Notify` is the fix, chosen because it stores
//! one permit, so a `notify_one` that lands before the loop reaches `notified()` is not
//! lost. The latency these tests report is observed, never inferred from the timer's minimum
//! sleep. The probe ladder that shared this loop retired into `exec` channel waits (D287).

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::timer::{PlanTimer, spawn};
use yi_types::plan::doc::{
    AgentId, Check, Delegation, GoalText, SpawnSpec, TodoAddr, TodoLabel, TodoState,
};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

struct Nobody;

impl Delegate for Nobody {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        Err("this plan delegates nothing".to_owned())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
}

struct Rig {
    /// The loop's clock, driven by the test.
    clock: Arc<Mutex<Instant>>,
    /// Wakes the loop took, counted through the stuck job's children source.
    wakes: Arc<AtomicU32>,
    _dir: Scratch,
}

fn rig() -> Result<(Rig, PlanTimer), Box<dyn Error>> {
    let dir = Scratch::new("yi-plan-timer")?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(Nobody),
    ));
    let clock = Arc::new(Mutex::new(Instant::now()));
    let wakes = Arc::new(AtomicU32::new(0));
    let (read_clock, woke) = (Arc::clone(&clock), Arc::clone(&wakes));
    let timer = PlanTimer::new(engine)
        .with_clock(Arc::new(move || {
            read_clock
                .lock()
                .map(|now| *now)
                .unwrap_or_else(|_| Instant::now())
        }))
        .with_children(
            Arc::new(move || {
                woke.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }),
            Arc::new(|_notice: &str, _| {}),
        );
    Ok((
        Rig {
            clock,
            wakes,
            _dir: dir,
        },
        timer,
    ))
}

fn now(rig: &Rig) -> Result<Instant, Box<dyn Error>> {
    Ok(*rig.clock.lock().map_err(|_| "poisoned")?)
}

fn set(rig: &Rig, at: Instant) -> Result<(), Box<dyn Error>> {
    *rig.clock.lock().map_err(|_| "poisoned")? = at;
    Ok(())
}

/// Refuses its first spawn, as a host whose roster is full, and takes every later one.
struct FullOnce(AtomicU32);

impl Delegate for FullOnce {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err("RLM child limit reached".to_owned());
        }
        AgentId::new("child-1").map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
}

/// Dies with the tick walking every root in the directory (timer.rs): a sibling session's
/// tick starts this session's todo under a host that will never report it.
#[test]
fn the_backstop_starts_only_the_roots_this_session_owns() -> TestResult {
    let dir = Scratch::new("yi-plan-probe-owned")?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(FullOnce(AtomicU32::new(0))),
    ));
    let accept = Check::Command("true".to_owned());
    let opened = engine.apply(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op: Op::Init {
            goal: GoalText::new("write the notes")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new("notes")?,
                after: Vec::new(),
                delegation: Some(Delegation {
                    spec: SpawnSpec {
                        role: None,
                        model: None,
                        effort: None,
                        tools: Vec::new(),
                        isolation: None,
                        budget: None,
                        wall: None,
                        parent_close: None,
                        extra: serde_json::Map::new(),
                    },
                    accept,
                    output: None,
                    context: Vec::new(),
                    note: None,
                    extra: serde_json::Map::new(),
                }),
                contract: None,
                children: Vec::new(),
                cites: Default::default(),
            }],
        },
        request_id: None,
        expected_revision: None,
    })?;
    let root = opened.plan.id.clone();
    let state = |engine: &PlanEngine| -> Result<TodoState, Box<dyn Error>> {
        let plan = engine.store().read(&root)?;
        Ok(plan
            .todo(&TodoLabel::new("notes")?)
            .ok_or("missing")?
            .state
            .clone())
    };
    assert_eq!(
        state(&engine)?,
        TodoState::Pending,
        "the first spawn was refused"
    );
    let sibling = PlanTimer::new(Arc::clone(&engine)).with_owned(Arc::new(Vec::new));
    sibling.tick(Instant::now());
    assert_eq!(
        state(&engine)?,
        TodoState::Pending,
        "a sibling's tick starts nothing here"
    );
    let owned = root.clone();
    let own = PlanTimer::new(Arc::clone(&engine)).with_owned(Arc::new(move || vec![owned.clone()]));
    own.tick(Instant::now());
    assert!(
        matches!(state(&engine)?, TodoState::Running { .. }),
        "{:?}",
        state(&engine)?
    );
    Ok(())
}

/// Polls until `done` holds, and reports how long that took; `None` past the limit.
async fn observed(done: impl Fn() -> bool, limit: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return Some(start.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

#[tokio::test]
async fn an_earlier_due_time_wakes_the_loop() -> TestResult {
    let (rig, timer) = rig()?;
    let start = now(&rig)?;
    let timer = Arc::new(timer);
    // A permit stored before the loop parks is not lost: the first wake comes at once
    // instead of after the idle poll.
    timer.wake_at(start);
    spawn(Arc::clone(&timer));
    let first = observed(
        || rig.wakes.load(Ordering::SeqCst) >= 1,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        first.is_some(),
        "a permit stored before the park wakes the loop"
    );
    // A parked loop takes no wake of its own: the tick's end is not a wake, so on a frozen
    // clock the count stays where the permit left it.
    let parked = rig.wakes.load(Ordering::SeqCst);
    assert!(
        observed(
            || rig.wakes.load(Ordering::SeqCst) > parked,
            Duration::from_millis(1_500)
        )
        .await
        .is_none(),
        "the loop woke itself {} times on a frozen clock",
        rig.wakes.load(Ordering::SeqCst).saturating_sub(parked)
    );

    // A due time registered mid-sleep runs at its own time, not at the end of the idle
    // poll it interrupted; the latency is observed, never inferred from the timer.
    let due = start + Duration::from_secs(10);
    set(&rig, due)?;
    timer.wake_at(due);
    let latency = observed(
        || rig.wakes.load(Ordering::SeqCst) > parked,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        latency.is_some(),
        "the loop woke on the registered due time (observed {latency:?}, the idle poll is 60 s)"
    );
    Ok(())
}

/// Dies with the registered-due term of `next_wake` (timer.rs): ignore it and a due time
/// registered while the loop sleeps is taken only at the idle poll. The wake here comes from
/// the timer, not from a stored permit: the permit is spent before the clock reaches the due.
#[tokio::test]
async fn a_due_time_inside_the_idle_poll_wakes_the_loop_on_its_own_time() -> TestResult {
    let (rig, timer) = rig()?;
    let start = now(&rig)?;
    let timer = Arc::new(timer);
    spawn(Arc::clone(&timer));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let due = start + Duration::from_secs(1);
    timer.wake_at(due);
    tokio::time::sleep(Duration::from_millis(300)).await;
    set(&rig, due)?;
    assert_eq!(
        timer.due_times(),
        1,
        "the permit's wake took a due time not yet come"
    );
    let taken = observed(|| timer.due_times() == 0, Duration::from_secs(4)).await;
    assert!(
        taken.is_some(),
        "the loop woke on the registered due time within its own second, not the idle poll"
    );
    Ok(())
}

/// Dies with the set of due times (timer.rs): keep one slot and an earlier registration
/// forgets a later one, which then waits the idle poll out (the R9 latency the Notify removes).
#[tokio::test]
async fn two_registered_due_times_both_wake_the_loop() -> TestResult {
    let (rig, timer) = rig()?;
    let start = now(&rig)?;
    let timer = Arc::new(timer);
    spawn(Arc::clone(&timer));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (earlier, later) = (
        start + Duration::from_secs(1),
        start + Duration::from_secs(2),
    );
    timer.wake_at(later);
    timer.wake_at(earlier);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        timer.due_times(),
        2,
        "an earlier registration keeps the later one"
    );
    set(&rig, earlier)?;
    assert!(
        observed(|| timer.due_times() == 1, Duration::from_secs(4))
            .await
            .is_some(),
        "the earlier due time is taken on its own second"
    );
    set(&rig, later)?;
    assert!(
        observed(|| timer.due_times() == 0, Duration::from_secs(4))
            .await
            .is_some(),
        "the later due time is taken on its own second, not at the idle poll"
    );
    Ok(())
}

/// Dies with where `wake_once` runs the lease job (timer.rs): put it behind the in-flight
/// check, or on the tick's own thread, and a cancel's grace ends only when a slow tick does.
#[tokio::test]
async fn a_revoke_due_time_wakes_the_loop_past_a_slow_tick() -> TestResult {
    let (rig, timer) = rig()?;
    let start = now(&rig)?;
    let (inside, release) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (entered, released) = (Arc::clone(&inside), Arc::clone(&release));
    let expiries = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&expiries);
    let timer = Arc::new(
        timer
            .with_owned(Arc::new(move || {
                entered.store(true, Ordering::SeqCst);
                while !released.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Vec::new()
            }))
            .with_leases(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
    );
    timer.wake_at(start);
    spawn(Arc::clone(&timer));
    let held = observed(|| inside.load(Ordering::SeqCst), Duration::from_secs(5)).await;
    assert!(held.is_some(), "the tick is in flight and will not return");

    // A revoke registers its grace's due time now, mid-tick and mid-sleep.
    let before = expiries.load(Ordering::SeqCst);
    let due = start + Duration::from_secs(5);
    set(&rig, due)?;
    timer.wake_at(due);
    let latency = observed(
        || expiries.load(Ordering::SeqCst) > before,
        Duration::from_secs(4),
    )
    .await;
    let still_ticking = !release.swap(true, Ordering::SeqCst);
    assert!(
        latency.is_some() && still_ticking,
        "the lease job ran on the due time while the tick was still out (observed {latency:?})"
    );
    Ok(())
}

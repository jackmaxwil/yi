//! The §5 saturating probe ladder: a `Blocked{on: External}` todo returns on
//! its own when its probe passes, and a probeless one nudges its owner.
//!
//! F0d, the wake. Plan section 7.4 finding R9: `next_wake` caps the sleep at 60 seconds and
//! a due time registered mid-sleep waits it out, so a grace that fires in a second is a
//! claim the loop cannot keep. A `tokio::sync::Notify` is the fix, chosen because it stores
//! one permit, so a `notify_one` that lands before the loop reaches `notified()` is not
//! lost. The row below uses this file's own helpers, `rig`, `open` and `arm`, plus the
//! `ran` counter the rig already carries, and registers the earlier due time through the
//! same path the stuck job and F2b's revoke grace use. The latency it reports is observed,
//! never inferred from the timer's minimum sleep.
//!
//! | test | tier | helpers | what it pins | the control it dies with |
//! |---|---|---|---|---|
//! | `an_earlier_due_time_wakes_the_loop` | T0 | `rig`, `open`, `arm`, the rig's `ran` and `wakes` counters, `ProbeLadder::tick` | A due time registered while the loop is parked on a long sleep runs at its own time and not at the end of that sleep; a due time later than the current wake changes nothing; a `notify_one` that arrives before the loop parks still wakes it, so the wake cannot be lost to the order of two threads; and a parked loop on a frozen clock takes no wake of its own. | The `Notify` being awaited alongside the sleep rather than checked before it, and the tick never notifying the loop when it ends. Poll for the due time instead and the cancel grace is bounded below by the ladder's tick, which is the false latency bound R9 named; let the tick notify and its stored permit runs the next tick at once, a spin that reads every plan on every iteration. |

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, TodoSpec};
use yi_runtime::plan::probe::{FIRST_DELAY, MAX_DELAY, ProbeLadder, Rung, Verdict, spawn};
use yi_runtime::plan::store::PlanStore;
use yi_types::message::AgentMessage;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Delegation, GoalText, PlanId, ProbeCommand, TodoAddr, TodoLabel, TodoState,
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
    store: PlanStore,
    engine: Arc<PlanEngine>,
    said: Arc<Mutex<Vec<String>>>,
    green: Arc<AtomicBool>,
    ran: Arc<AtomicU32>,
    /// The loop's clock, driven by the test.
    clock: Arc<Mutex<Instant>>,
    /// Wakes the loop took, counted through the stuck job's children source.
    wakes: Arc<AtomicU32>,
    _dir: Scratch,
}

fn rig() -> Result<(Rig, ProbeLadder), Box<dyn Error>> {
    let dir = Scratch::new("yi-plan-probe")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(Nobody),
    ));
    let said: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&said);
    let green = Arc::new(AtomicBool::new(false));
    let ran = Arc::new(AtomicU32::new(0));
    let pass = Arc::clone(&green);
    let count = Arc::clone(&ran);
    let clock = Arc::new(Mutex::new(Instant::now()));
    let wakes = Arc::new(AtomicU32::new(0));
    let read_clock = Arc::clone(&clock);
    let woke = Arc::clone(&wakes);
    let ladder = ProbeLadder::new(
        Arc::clone(&engine),
        dir.to_path_buf(),
        Arc::new(move |message: AgentMessage, _mode| {
            if let AgentMessage::Custom { content, .. } = message
                && let Ok(mut said) = sink.lock()
            {
                said.push(format!("{content:?}"));
            }
        }),
    )
    .with_run(Arc::new(move |_command: &str| {
        count.fetch_add(1, Ordering::SeqCst);
        if pass.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("still red".to_owned())
        }
    }))
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
        Arc::new(|_notice: &str| {}),
    );
    Ok((
        Rig {
            store,
            engine,
            said,
            green,
            ran,
            clock,
            wakes,
            _dir: dir,
        },
        ladder,
    ))
}

fn open(rig: &Rig, label: &str, probe: Option<&str>) -> Result<(), Box<dyn Error>> {
    rig.engine.apply(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op: Op::Init {
            goal: GoalText::new("wait on the world")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new(label)?,
                after: Vec::new(),
                delegation: None,
                contract: None,
                children: Vec::new(),
            }],
        },
        request_id: None,
        expected_revision: None,
    })?;
    let probe = probe.map(ProbeCommand::new).transpose()?;
    rig.engine.apply(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op: Op::Block {
            label: TodoLabel::new(label)?,
            on: BlockedOn::External { probe },
            note: "the deploy has to finish".to_owned(),
        },
        request_id: None,
        expected_revision: None,
    })?;
    Ok(())
}

/// The first tick a slot is seen on only arms it; the run comes a rung later.
fn arm(ladder: &ProbeLadder) -> Result<Instant, Box<dyn Error>> {
    let start = Instant::now();
    assert!(
        ladder.tick(start).is_empty(),
        "a freshly blocked todo waits one rung before its first run"
    );
    start
        .checked_add(FIRST_DELAY)
        .ok_or("clock overflowed".into())
}

/// Incident: the ladder walked the canonical root alone, so a block inside a
/// sub-plan was never probed and never nudged.
#[test]
fn a_block_inside_a_sub_plan_is_probed_too() -> TestResult {
    let (rig, ladder) = rig()?;
    let owner = |plan: Option<PlanId>, op: Op| OpRequest {
        plan,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    };
    rig.engine.apply(owner(
        None,
        Op::Init {
            goal: GoalText::new("wait on the world")?,
            todos: vec![TodoSpec {
                label: TodoLabel::new("deploy the widget")?,
                after: Vec::new(),
                delegation: None,
                contract: None,
                children: Vec::new(),
            }],
        },
    ))?;
    rig.engine.apply(owner(
        None,
        Op::Start {
            label: TodoLabel::new("deploy the widget")?,
        },
    ))?;
    let sub = rig
        .engine
        .apply(owner(
            None,
            Op::Decompose {
                label: TodoLabel::new("deploy the widget")?,
                todos: vec![TodoSpec {
                    label: TodoLabel::new("wait for staging")?,
                    after: Vec::new(),
                    delegation: None,
                    contract: None,
                    children: Vec::new(),
                }],
            },
        ))?
        .subplan
        .ok_or("decompose opened no sub-plan")?;
    rig.engine.apply(owner(
        Some(sub.clone()),
        Op::Block {
            label: TodoLabel::new("wait for staging")?,
            on: BlockedOn::External {
                probe: Some(ProbeCommand::new("curl staging")?),
            },
            note: "staging is deploying".to_owned(),
        },
    ))?;
    rig.green.store(true, Ordering::SeqCst);
    let due = arm(&ladder)?;
    let verdicts = ladder.tick(due);
    assert_eq!(
        verdicts,
        vec![Verdict::Unblocked {
            label: TodoLabel::new("wait for staging")?
        }],
        "the sub-plan's block is on the ladder"
    );
    let todo = rig
        .store
        .read(&sub)?
        .todo(&TodoLabel::new("wait for staging")?)
        .ok_or("todo missing")?
        .state
        .clone();
    assert_eq!(todo, TodoState::Pending);
    Ok(())
}

fn state_of(rig: &Rig, label: &str) -> Result<TodoState, Box<dyn Error>> {
    let id = rig.store.roots()?.into_iter().next().ok_or("no plan")?;
    let file = rig.store.read(&id)?;
    Ok(file
        .todo(&TodoLabel::new(label)?)
        .ok_or("todo missing")?
        .state
        .clone())
}

#[test]
fn a_passing_probe_unblocks_the_todo_under_host_authority() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("true"))?;
    let due = arm(&ladder)?;
    assert_eq!(rig.ran.load(Ordering::SeqCst), 0);
    let verdicts = ladder.tick(due);
    assert!(
        matches!(verdicts.as_slice(), [Verdict::Retry { rung, .. }] if *rung == Rung::default().next()),
        "a red probe climbs one rung: {verdicts:?}"
    );
    assert!(matches!(
        state_of(&rig, "wait for the deploy")?,
        TodoState::Blocked { .. }
    ));

    rig.green.store(true, Ordering::SeqCst);
    let next = due
        .checked_add(Rung::default().next().delay())
        .ok_or("clock overflowed")?;
    let verdicts = ladder.tick(next);
    assert!(
        matches!(verdicts.as_slice(), [Verdict::Unblocked { .. }]),
        "a green probe clears the block: {verdicts:?}"
    );
    assert_eq!(state_of(&rig, "wait for the deploy")?, TodoState::Pending);
    let said = rig.said.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(said.contains("cleared"), "the owner is told: {said}");
    Ok(())
}

#[test]
fn a_red_probe_is_not_re_run_before_its_rung_comes_due() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("false"))?;
    let due = arm(&ladder)?;
    ladder.tick(due);
    assert_eq!(rig.ran.load(Ordering::SeqCst), 1);
    // One second past the first failure is inside the second rung's minute.
    let too_soon = due
        .checked_add(std::time::Duration::from_secs(1))
        .ok_or("clock overflowed")?;
    assert!(ladder.tick(too_soon).is_empty());
    assert_eq!(
        rig.ran.load(Ordering::SeqCst),
        1,
        "the ladder must not re-run a probe before its own wait has elapsed"
    );
    Ok(())
}

#[test]
fn an_external_block_with_no_probe_nudges_its_owner_instead() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for legal", None)?;
    let due = arm(&ladder)?;
    let verdicts = ladder.tick(due);
    assert!(
        matches!(verdicts.as_slice(), [Verdict::Nudged { .. }]),
        "a probeless block nudges: {verdicts:?}"
    );
    assert_eq!(rig.ran.load(Ordering::SeqCst), 0);
    let said = rig.said.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(said.contains("carries no probe"), "{said}");
    assert!(
        ladder
            .tick(
                due.checked_add(MAX_DELAY)
                    .ok_or("clock overflowed")?
                    .checked_sub(std::time::Duration::from_secs(1))
                    .ok_or("clock underflowed")?
            )
            .is_empty(),
        "a nudge waits the ceiling interval before the next one"
    );
    Ok(())
}

#[test]
fn a_todo_that_is_no_longer_blocked_leaves_the_ladder() -> TestResult {
    let (rig, ladder) = rig()?;
    rig.green.store(true, Ordering::SeqCst);
    open(&rig, "wait for the deploy", Some("true"))?;
    let due = arm(&ladder)?;
    ladder.tick(due);
    assert_eq!(state_of(&rig, "wait for the deploy")?, TodoState::Pending);
    let ran = rig.ran.load(Ordering::SeqCst);
    assert!(ladder.tick(due).is_empty());
    assert_eq!(
        rig.ran.load(Ordering::SeqCst),
        ran,
        "an unblocked todo must not keep running its probe"
    );
    Ok(())
}

#[test]
fn a_passed_probe_whose_unblock_is_refused_climbs_the_rung() -> TestResult {
    let (rig, ladder) = rig()?;
    rig.green.store(true, Ordering::SeqCst);
    open(&rig, "wait for the deploy", Some("true"))?;
    let due = arm(&ladder)?;
    let held = rig.store.lease()?;
    let verdicts = ladder.tick(due);
    assert!(
        matches!(verdicts.as_slice(), [Verdict::Retry { .. }]),
        "a refused unblock is a retry, not a clear: {verdicts:?}"
    );
    assert!(matches!(
        state_of(&rig, "wait for the deploy")?,
        TodoState::Blocked { .. }
    ));
    let ran = rig.ran.load(Ordering::SeqCst);
    let too_soon = due
        .checked_add(std::time::Duration::from_secs(1))
        .ok_or("clock overflowed")?;
    assert!(ladder.tick(too_soon).is_empty());
    assert_eq!(
        rig.ran.load(Ordering::SeqCst),
        ran,
        "a refused unblock waits its rung instead of re-running every second"
    );
    drop(held);
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
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("false"))?;
    let start = *rig.clock.lock().map_err(|_| "poisoned")?;
    let ladder = Arc::new(ladder);
    // A permit stored before the loop parks is not lost: the first wake comes at once
    // instead of after the idle poll.
    ladder.wake_at(start);
    spawn(Arc::clone(&ladder));
    let first = observed(
        || rig.wakes.load(Ordering::SeqCst) >= 1,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        first.is_some(),
        "a permit stored before the park wakes the loop"
    );
    // That wake armed the slot one rung out, so the loop is parked on the full minute.
    assert_eq!(rig.ran.load(Ordering::SeqCst), 0);
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

    // A due time later than the current wake runs nothing.
    ladder.wake_at(start + MAX_DELAY);
    assert!(
        observed(
            || rig.ran.load(Ordering::SeqCst) >= 1,
            Duration::from_millis(1_500)
        )
        .await
        .is_none(),
        "a later due time changes nothing"
    );

    // An earlier due time registered mid-sleep runs at its own time, not at the end of
    // the sleep it interrupted; the latency is observed, never inferred from the timer.
    let due = start + FIRST_DELAY;
    *rig.clock.lock().map_err(|_| "poisoned")? = due;
    ladder.wake_at(due);
    let latency = observed(
        || rig.ran.load(Ordering::SeqCst) >= 1,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        latency.is_some_and(|seen| seen < Duration::from_secs(5)),
        "the probe ran on the registered due time (observed {latency:?}, sleep was {FIRST_DELAY:?})"
    );
    Ok(())
}

/// Dies with the registered-due term of `next_wake` (probe.rs): ignore it and a due time
/// inside the idle poll waits the probe's own rung out, while the loop parks on the longer
/// sleep. The wake here comes from the timer, not from a stored permit: the registration
/// happens while the loop is already parked on the probe's ten seconds.
#[tokio::test]
async fn a_due_time_inside_the_idle_poll_wakes_the_loop_on_its_own_time() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("false"))?;
    let start = *rig.clock.lock().map_err(|_| "poisoned")?;
    // The slot is armed by a direct tick, so the loop's first sleep is the rung, not a permit.
    assert!(ladder.tick(start).is_empty());
    let ladder = Arc::new(ladder);
    *rig.clock.lock().map_err(|_| "poisoned")? = start + FIRST_DELAY - Duration::from_secs(10);
    spawn(Arc::clone(&ladder));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let due = start + FIRST_DELAY - Duration::from_secs(9);
    ladder.wake_at(due);
    *rig.clock.lock().map_err(|_| "poisoned")? = due;
    assert_eq!(ladder.due_times(), 1);
    let taken = observed(|| ladder.due_times() == 0, Duration::from_secs(4)).await;
    assert!(
        taken.is_some(),
        "the loop woke on the registered due time within its own second, not the probe's ten"
    );
    assert_eq!(
        rig.ran.load(Ordering::SeqCst),
        0,
        "the probe's own rung has not come"
    );
    Ok(())
}

/// Dies with the set of due times (probe.rs): keep one slot and an earlier registration
/// forgets a later one, which then waits the idle poll out (the R9 latency the Notify removes).
#[tokio::test]
async fn two_registered_due_times_both_wake_the_loop() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("false"))?;
    let start = *rig.clock.lock().map_err(|_| "poisoned")?;
    assert!(ladder.tick(start).is_empty());
    let ladder = Arc::new(ladder);
    *rig.clock.lock().map_err(|_| "poisoned")? = start + FIRST_DELAY - Duration::from_secs(10);
    spawn(Arc::clone(&ladder));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let later = start + FIRST_DELAY - Duration::from_secs(8);
    let earlier = start + FIRST_DELAY - Duration::from_secs(9);
    ladder.wake_at(later);
    ladder.wake_at(earlier);
    assert_eq!(
        ladder.due_times(),
        2,
        "an earlier registration keeps the later one"
    );
    *rig.clock.lock().map_err(|_| "poisoned")? = earlier;
    assert!(
        observed(|| ladder.due_times() == 1, Duration::from_secs(4))
            .await
            .is_some(),
        "the earlier due time is taken on its own second"
    );
    *rig.clock.lock().map_err(|_| "poisoned")? = later;
    assert!(
        observed(|| ladder.due_times() == 0, Duration::from_secs(4))
            .await
            .is_some(),
        "the later due time is taken on its own second, not at the idle poll"
    );
    Ok(())
}

/// Dies with where `wake_once` runs the lease job (probe.rs): put it behind the in-flight
/// check, or on the probe's own thread, and a cancel's grace ends only when a slow probe does.
#[tokio::test]
async fn a_revoke_due_time_wakes_the_loop_past_a_slow_probe() -> TestResult {
    let (rig, ladder) = rig()?;
    open(&rig, "wait for the deploy", Some("false"))?;
    let start = *rig.clock.lock().map_err(|_| "poisoned")?;
    assert!(
        ladder.tick(start).is_empty(),
        "the slot is armed one rung out"
    );
    let (probing, release) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (entered, released) = (Arc::clone(&probing), Arc::clone(&release));
    let expiries = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&expiries);
    let ladder = Arc::new(
        ladder
            .with_run(Arc::new(move |_command: &str| {
                entered.store(true, Ordering::SeqCst);
                while !released.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err("still deploying".to_owned())
            }))
            .with_leases(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
    );
    *rig.clock.lock().map_err(|_| "poisoned")? = start + FIRST_DELAY;
    ladder.wake_at(start + FIRST_DELAY);
    spawn(Arc::clone(&ladder));
    let held = observed(|| probing.load(Ordering::SeqCst), Duration::from_secs(5)).await;
    assert!(held.is_some(), "the probe is in flight and will not return");

    // A revoke registers its grace's due time now, mid-probe and mid-sleep.
    let before = expiries.load(Ordering::SeqCst);
    let due = start + FIRST_DELAY + Duration::from_secs(5);
    *rig.clock.lock().map_err(|_| "poisoned")? = due;
    ladder.wake_at(due);
    let latency = observed(
        || expiries.load(Ordering::SeqCst) > before,
        Duration::from_secs(4),
    )
    .await;
    let still_probing = !release.swap(true, Ordering::SeqCst);
    assert!(
        latency.is_some() && still_probing,
        "the lease job ran on the due time while the probe was still out (observed {latency:?})"
    );
    Ok(())
}

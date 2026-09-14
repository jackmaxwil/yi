//! The §5 saturating probe ladder: a `Blocked{on: External}` todo returns on
//! its own when its probe passes, and a probeless one nudges its owner.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, PlanEngine, TodoSpec};
use yi_runtime::plan::probe::{FIRST_DELAY, MAX_DELAY, ProbeLadder, Rung, Verdict};
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

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

struct Rig {
    store: PlanStore,
    engine: Arc<PlanEngine>,
    said: Arc<Mutex<Vec<String>>>,
    green: Arc<AtomicBool>,
    ran: Arc<AtomicU32>,
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
    }));
    Ok((
        Rig {
            store,
            engine,
            said,
            green,
            ran,
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

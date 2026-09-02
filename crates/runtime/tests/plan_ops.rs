//! Engine-level tests for [`yi_runtime::plan::ops::PlanEngine`]: the op suite
//! that used to live beside the engine, plus one regression per confirmed
//! defect of the 2026-08-31 review — each was watched failing on the unfixed
//! engine before its fix landed.

use std::error::Error;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Map;
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, OutputResolve, PlanEngine, PlanOpError, TodoSpec,
    dispatch_width,
};
use yi_runtime::plan::store::{FRONTMATTER_CAP_BYTES, PlanStore, StoreError};
use yi_runtime::plan::table::RETRY_CAP;
use yi_types::plan::PlanVersion;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, Delegation, GoalText, OutputSchema, PlanId, PlanState, SPAWN_CAP,
    SpawnSpec, TodoAddr, TodoLabel, TodoState, TouchCount,
};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

struct TempStore {
    dir: PathBuf,
}

impl TempStore {
    fn new() -> Result<(Self, PlanStore), Box<dyn Error>> {
        let dir = std::env::temp_dir().join(format!(
            "yi-plan-ops-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = PlanStore::open(dir.clone())?;
        Ok((Self { dir }, store))
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Default)]
struct Stub {
    next: AtomicU32,
    reaps: AtomicU32,
    fail_reap: AtomicBool,
    reap_last: Mutex<Option<Url>>,
    follow: Mutex<Vec<(Vec<TodoLabel>, usize)>>,
}

impl Delegate for Stub {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let serial = self.next.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        if self.fail_reap.load(Ordering::SeqCst) {
            return Err("child is wedged".to_owned());
        }
        self.reaps.fetch_add(1, Ordering::SeqCst);
        match self.reap_last.lock() {
            Ok(last) => Ok(last.clone()),
            Err(_) => Err("poisoned".to_owned()),
        }
    }

    fn follow_up(&self, dispatched: &[TodoLabel], held: usize) {
        if let Ok(mut log) = self.follow.lock() {
            log.push((dispatched.to_vec(), held));
        }
    }
}

fn width(value: usize) -> Result<NonZeroUsize, Box<dyn Error>> {
    NonZeroUsize::new(value).ok_or_else(|| "zero width".into())
}

type Harness = (TempStore, PlanStore, Arc<Stub>, PlanEngine);

fn harness(cap: usize) -> Result<Harness, Box<dyn Error>> {
    let (temp, store) = TempStore::new()?;
    let stub = Arc::new(Stub::default());
    let engine = PlanEngine::new(store.clone(), stub.clone()).with_width(width(cap)?);
    Ok((temp, store, stub, engine))
}

fn label(text: &str) -> Result<TodoLabel, Box<dyn Error>> {
    Ok(TodoLabel::new(text)?)
}

fn spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: label(text)?,
        after: Vec::new(),
        delegation: None,
    })
}

fn delegation() -> Delegation {
    Delegation {
        spec: SpawnSpec {
            role: None,
            model: None,
            effort: None,
            tools: Vec::new(),
            isolation: None,
            budget: None,
            extra: Map::new(),
        },
        accept: Check::Stated("it works".to_owned()),
        output: None,
        context: Vec::new(),
        note: None,
        extra: Map::new(),
    }
}

fn delegated_spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: label(text)?,
        after: Vec::new(),
        delegation: Some(delegation()),
    })
}

fn owner(op: Op) -> OpRequest {
    OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
    }
}

fn at(plan: &PlanId, op: Op) -> OpRequest {
    OpRequest {
        plan: Some(plan.clone()),
        actor: Actor::Owner,
        op,
    }
}

fn init(engine: &PlanEngine, specs: Vec<TodoSpec>) -> Result<Outcome, PlanOpError> {
    engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the widget end to end").map_err(PlanOpError::Doc)?,
        todos: specs,
    }))
}

#[test]
fn width_clamps_low_and_high() -> TestResult {
    assert_eq!(dispatch_width(width(1)?).get(), 1);
    assert_eq!(dispatch_width(width(2)?).get(), 1);
    assert_eq!(dispatch_width(width(9)?).get(), 8);
    assert_eq!(dispatch_width(width(64)?).get(), 8);
    Ok(())
}

#[test]
fn backpressure_holds_a_delegated_todo_pending() -> TestResult {
    let (_temp, store, _stub, engine) = harness(1)?;
    let out = init(
        &engine,
        vec![
            delegated_spec("first child job")?,
            delegated_spec("second child job")?,
        ],
    )?;
    assert_eq!(out.dispatched, vec![label("first child job")?]);
    assert_eq!(out.held, vec![label("second child job")?]);
    let out = engine.apply(owner(Op::Start {
        label: label("first child job")?,
    }))?;
    assert_eq!(out.spawned.len(), 1);
    assert_eq!(out.held, vec![label("second child job")?]);
    let file = store.read(&out.plan.id)?;
    let held = file
        .plan
        .todo(&label("second child job")?)
        .ok_or("held todo missing")?;
    assert_eq!(held.state, TodoState::Pending);
    let out = engine.apply(owner(Op::Start {
        label: label("second child job")?,
    }))?;
    assert_eq!(out.spawned.len(), 1);
    assert_eq!(out.plan.spawns().get(), 2);
    let out = engine.apply(owner(Op::Done {
        label: label("first child job")?,
        output: Some("kernel://main/cli_surface".parse::<Url>()?),
    }))?;
    let todo = out
        .plan
        .todo(&label("first child job")?)
        .ok_or("todo missing")?;
    assert_eq!(
        todo.state,
        TodoState::Done {
            output: Some("kernel://main/cli_surface".parse::<Url>()?),
        }
    );
    Ok(())
}

#[test]
fn fuse_refuses_at_cap_and_survives_supersede() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    let mut file = store.read(&out.plan.id)?;
    while file.plan.spawns() < SPAWN_CAP {
        file.plan.charge_spawn();
    }
    store.write(&file)?;
    let refused = engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }));
    match refused {
        Err(PlanOpError::SpawnCeilingExhausted { spent, cap }) => {
            assert_eq!(spent, SPAWN_CAP);
            assert_eq!(cap, SPAWN_CAP);
        }
        other => return Err(format!("expected ceiling refusal, got {other:?}").into()),
    }
    let out = engine.apply(owner(Op::Supersede {
        reason: "the cut was wrong".to_owned(),
        todos: vec![delegated_spec("replacement job")?],
    }))?;
    assert_eq!(out.plan.spawns(), SPAWN_CAP);
    let refused = engine.apply(owner(Op::Start {
        label: label("replacement job")?,
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::SpawnCeilingExhausted { .. })
    ));
    Ok(())
}

#[test]
fn supersede_is_atomic_when_a_reap_fails() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }))?;
    stub.fail_reap.store(true, Ordering::SeqCst);
    let refused = engine.apply(owner(Op::Supersede {
        reason: "rethink".to_owned(),
        todos: vec![spec("replacement job")?],
    }));
    assert!(matches!(refused, Err(PlanOpError::ReapFailed { .. })));
    let file = store.read(&out.plan.id)?;
    assert_eq!(file.plan.version, PlanVersion(1));
    assert_eq!(file.plan.touched, TouchCount(2));
    assert_eq!(file.plan.state, PlanState::Active);
    let todo = file
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    Ok(())
}

#[test]
fn ephemeral_urls_are_refused_in_terminal_records() -> TestResult {
    let (_temp, _store, stub, engine) = harness(8)?;
    init(&engine, vec![delegated_spec("delegated job")?])?;
    engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }))?;
    let refused = engine.apply(owner(Op::Done {
        label: label("delegated job")?,
        output: Some("agent://somewhere/live".parse::<Url>()?),
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    let refused = engine.apply(owner(Op::Done {
        label: label("delegated job")?,
        output: Some("kernel://child-0/scratch".parse::<Url>()?),
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("kernel://child/scratch".parse::<Url>()?);
    }
    let refused = engine.apply(owner(Op::Fail {
        label: label("delegated job")?,
        cause: "went sideways".to_owned(),
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::EphemeralTerminal { .. })
    ));
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("kernel://main/report".parse::<Url>()?);
    }
    let out = engine.apply(owner(Op::Fail {
        label: label("delegated job")?,
        cause: "went sideways".to_owned(),
    }))?;
    assert_eq!(out.reaped.len(), 1);
    let todo = out
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert_eq!(
        todo.state,
        TodoState::Failed {
            cause: "went sideways".to_owned(),
            last: Some("kernel://main/report".parse::<Url>()?),
        }
    );
    assert_eq!(out.plan.state, PlanState::Done);
    let found = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(
        found.plan.id, out.plan.id,
        "a finished plan with a failed todo stays reachable unnamed"
    );
    Ok(())
}

#[test]
fn a_cycle_is_refused_at_insert() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let second = TodoSpec {
        label: label("second job")?,
        after: vec![label("first job")?],
        delegation: None,
    };
    let out = init(&engine, vec![spec("first job")?, second])?;
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("first job")?,
        after: label("second job")?,
    }));
    match refused {
        Err(PlanOpError::Invalid { issue }) => {
            assert!(issue.to_string().contains("cycle"));
        }
        other => return Err(format!("expected cycle refusal, got {other:?}").into()),
    }
    let file = store.read(&out.plan.id)?;
    assert_eq!(file.plan.touched, TouchCount(1));
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("no such job")?,
        after: label("first job")?,
    }));
    assert!(matches!(refused, Err(PlanOpError::UnknownLabel { .. })));
    Ok(())
}

#[test]
fn a_partial_reorder_is_refused() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?, spec("second job")?])?;
    let refused = engine.apply(owner(Op::Reorder {
        labels: vec![label("first job")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::NotAPermutation {
            got: 1,
            expected: 2
        })
    ));
    let out = engine.apply(owner(Op::Reorder {
        labels: vec![label("second job")?, label("first job")?],
    }))?;
    let first = out.plan.todos.first().ok_or("empty plan")?;
    assert_eq!(first.label, label("second job")?);
    Ok(())
}

#[test]
fn mutation_is_owner_gated_and_unblock_is_open_to_user_and_host() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?])?;
    let refused = engine.apply(OpRequest {
        plan: None,
        actor: Actor::Child(AgentId::new("child-0")?),
        op: Op::Start {
            label: label("first job")?,
        },
    });
    match refused {
        Err(err @ PlanOpError::NotOwner { .. }) => {
            assert!(err.to_string().contains("propose to the owner"));
        }
        other => return Err(format!("expected owner refusal, got {other:?}").into()),
    }
    engine.apply(owner(Op::Block {
        label: label("first job")?,
        on: BlockedOn::User,
        note: "needs a decision".to_owned(),
    }))?;
    let out = engine.apply(OpRequest {
        plan: None,
        actor: Actor::User("user://3".parse::<Url>()?),
        op: Op::Unblock {
            label: label("first job")?,
        },
    })?;
    let todo = out.plan.todo(&label("first job")?).ok_or("todo missing")?;
    assert_eq!(todo.state, TodoState::Pending);
    Ok(())
}

#[test]
fn a_sub_plan_dispatch_charges_the_root_fuse() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("parent job")?])?;
    let root_id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    let out = engine.apply(at(
        &sub_id,
        Op::Start {
            label: label("small piece")?,
        },
    ))?;
    assert_eq!(out.spawned.len(), 1);
    assert_eq!(store.read(&root_id)?.plan.spawns().get(), 1);
    assert!(store.read(&sub_id)?.plan.spawns().is_zero());
    let refused = engine.apply(at(
        &sub_id,
        Op::Decompose {
            label: label("small piece")?,
            todos: vec![spec("depth three")?],
        },
    ));
    assert!(matches!(refused, Err(PlanOpError::DepthExhausted { .. })));
    Ok(())
}

// Regressions from the 2026-08-31 confirmed-defect review, one per finding.

#[test]
fn blocking_a_running_delegated_todo_reaps_the_child() -> TestResult {
    let (_temp, store, stub, engine) = harness(1)?;
    init(
        &engine,
        vec![delegated_spec("first job")?, delegated_spec("second job")?],
    )?;
    engine.apply(owner(Op::Start {
        label: label("first job")?,
    }))?;
    let out = engine.apply(owner(Op::Block {
        label: label("first job")?,
        on: BlockedOn::User,
        note: "waiting on a decision".to_owned(),
    }))?;
    assert_eq!(out.reaped.len(), 1, "the block exit from Running must reap");
    assert_eq!(stub.reaps.load(Ordering::SeqCst), 1);
    let file = store.read(&out.plan.id)?;
    let blocked = file.plan.todo(&label("first job")?).ok_or("todo missing")?;
    assert!(matches!(blocked.state, TodoState::Blocked { .. }));
    Ok(())
}

#[test]
fn supersede_terminates_sub_plan_todos_and_frees_the_width() -> TestResult {
    let (_temp, store, stub, engine) = harness(1)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(at(
        &sub_id,
        Op::Start {
            label: label("small piece")?,
        },
    ))?;
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("history://small-piece/e4".parse::<Url>()?);
    }
    let out = engine.apply(owner(Op::Supersede {
        reason: "wrong cut".to_owned(),
        todos: vec![delegated_spec("fresh job")?],
    }))?;
    assert_eq!(out.reaped.len(), 1);
    let sub = store.read(&sub_id)?;
    assert_eq!(sub.plan.state, PlanState::Abandoned);
    let todo = sub
        .plan
        .todo(&label("small piece")?)
        .ok_or("sub todo missing")?;
    match &todo.state {
        TodoState::Failed { cause, last } => {
            assert!(cause.contains("superseded"), "{cause:?}");
            assert_eq!(last, &Some("history://small-piece/e4".parse::<Url>()?));
        }
        other => return Err(format!("expected a terminal reaped todo, got {other:?}").into()),
    }
    assert_eq!(
        out.dispatched,
        vec![label("fresh job")?],
        "the abandoned sub-plan must not hold a phantom width slot"
    );
    assert!(out.held.is_empty());
    Ok(())
}

#[test]
fn supersede_requires_an_active_plan() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(owner(Op::Supersede {
        reason: "restart".to_owned(),
        todos: vec![spec("new cut")?],
    }))?;
    assert_eq!(store.read(&sub_id)?.plan.state, PlanState::Abandoned);
    let refused = engine.apply(at(
        &sub_id,
        Op::Supersede {
            reason: "resurrect".to_owned(),
            todos: vec![spec("zombie job")?],
        },
    ));
    assert!(matches!(refused, Err(PlanOpError::NotActive { .. })));
    assert_eq!(store.read(&sub_id)?.plan.state, PlanState::Abandoned);
    Ok(())
}

#[test]
fn decompose_never_reissues_a_superseded_sub_plan_id() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("build the thing")?])?;
    engine.apply(owner(Op::Start {
        label: label("build the thing")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("build the thing")?,
        todos: vec![spec("first piece")?],
    }))?;
    let first_sub = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(owner(Op::Supersede {
        reason: "same label, new generation".to_owned(),
        todos: vec![spec("build the thing")?],
    }))?;
    engine.apply(owner(Op::Start {
        label: label("build the thing")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("build the thing")?,
        todos: vec![spec("second piece")?],
    }))?;
    let second_sub = out.subplan.ok_or("no second sub-plan")?;
    assert_ne!(
        second_sub, first_sub,
        "a superseded generation's ledger must never be overwritten"
    );
    let old = store.read(&first_sub)?;
    assert_eq!(old.plan.state, PlanState::Abandoned);
    assert!(old.plan.todo(&label("first piece")?).is_some());
    Ok(())
}

#[test]
fn add_edge_is_refused_on_an_abandoned_todo() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("first job")?, spec("second job")?])?;
    engine.apply(owner(Op::Drop {
        label: label("second job")?,
    }))?;
    let refused = engine.apply(owner(Op::AddEdge {
        todo: label("second job")?,
        after: label("first job")?,
    }));
    assert!(matches!(refused, Err(PlanOpError::IllegalStep { .. })));
    let file = store.read(&out.plan.id)?;
    let dropped = file
        .plan
        .todo(&label("second job")?)
        .ok_or("todo missing")?;
    assert!(
        dropped.after.is_empty(),
        "an abandoned todo must not mutate"
    );
    Ok(())
}

#[test]
fn a_failed_last_todo_reopens_on_retry() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("only job")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Fail {
        label: label("only job")?,
        cause: "first attempt sank".to_owned(),
    }))?;
    assert_eq!(store.read(&id)?.plan.state, PlanState::Done);
    engine.apply(at(
        &id,
        Op::Retry {
            label: label("only job")?,
            delegation: None,
        },
    ))?;
    let file = store.read(&id)?;
    assert_eq!(
        file.plan.state,
        PlanState::Active,
        "a retried plan must reopen"
    );
    let out = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(out.plan.id, id, "resolve(None) must find the reopened plan");
    Ok(())
}

#[test]
fn apply_refuses_while_the_store_lease_is_held() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    init(&engine, vec![spec("first job")?])?;
    let held = store.lease()?;
    let refused = engine.apply(owner(Op::Append {
        todos: vec![spec("second job")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::Store(StoreError::LeaseHeld { .. }))
    ));
    drop(held);
    engine.apply(owner(Op::Append {
        todos: vec![spec("second job")?],
    }))?;
    Ok(())
}

#[test]
fn a_hand_edit_is_folded_in_as_a_user_attributed_op() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("first job")?])?;
    let id = out.plan.id.clone();
    let mut file = store.read(&id)?;
    file.plan.goal = GoalText::new("ship the widget and the docs")?;
    store.write(&file)?;
    engine.apply(owner(Op::Append {
        todos: vec![spec("second job")?],
    }))?;
    let file = store.read(&id)?;
    assert_eq!(
        file.plan.touched,
        TouchCount(3),
        "init, one user edit, one append"
    );
    Ok(())
}

#[test]
fn a_refused_write_publishes_no_extra_files() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("big piece")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("big piece")?,
    }))?;
    let mut file = store.read(&id)?;
    let sub_id = file.plan.id.child(&label("big piece")?)?;
    let mut probe = file.clone();
    for todo in &mut probe.plan.todos {
        todo.subplan = Some(sub_id.clone());
    }
    let seed = "x".repeat(10);
    probe
        .plan
        .extra
        .insert("pad".to_owned(), seed.clone().into());
    let rendered = PlanStore::render(&probe.plan)?;
    let filler = FRONTMATTER_CAP_BYTES
        .saturating_add(1)
        .saturating_sub(rendered.len().saturating_sub(seed.len()));
    file.plan
        .extra
        .insert("pad".to_owned(), "x".repeat(filler).into());
    store.write(&file)?;
    let refused = engine.apply(owner(Op::Decompose {
        label: label("big piece")?,
        todos: vec![spec("small piece")?],
    }));
    assert!(matches!(
        refused,
        Err(PlanOpError::Store(StoreError::FrontmatterOverCap { .. }))
    ));
    assert!(
        !store.exists(&sub_id),
        "a refusal that says nothing was written must have written nothing"
    );
    Ok(())
}

#[test]
fn start_refuses_unmet_after_edges() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let follows = TodoSpec {
        label: label("second job")?,
        after: vec![label("first job")?],
        delegation: None,
    };
    let out = init(&engine, vec![spec("first job")?, follows])?;
    let refused = engine.apply(owner(Op::Start {
        label: label("second job")?,
    }));
    match refused {
        Err(PlanOpError::UnmetEdge { label: at, after }) => {
            assert_eq!(at, label("second job")?);
            assert_eq!(after, label("first job")?);
        }
        other => return Err(format!("expected an unmet-edge refusal, got {other:?}").into()),
    }
    let file = store.read(&out.plan.id)?;
    let second = file
        .plan
        .todo(&label("second job")?)
        .ok_or("todo missing")?;
    assert_eq!(second.state, TodoState::Pending);
    Ok(())
}

#[test]
fn retry_refuses_past_the_cap() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("flaky job")?])?;
    let id = out.plan.id.clone();
    let mut file = store.read(&id)?;
    for todo in &mut file.plan.todos {
        todo.state = TodoState::Failed {
            cause: "worn out".to_owned(),
            last: None,
        };
        todo.retries = RETRY_CAP;
    }
    store.write(&file)?;
    let refused = engine.apply(at(
        &id,
        Op::Retry {
            label: label("flaky job")?,
            delegation: None,
        },
    ));
    match refused {
        Err(PlanOpError::RetriesExhausted { spent, cap, .. }) => {
            assert_eq!(spent, RETRY_CAP);
            assert_eq!(cap, RETRY_CAP);
        }
        other => return Err(format!("expected a retry-cap refusal, got {other:?}").into()),
    }
    let file = store.read(&id)?;
    let todo = file.plan.todo(&label("flaky job")?).ok_or("todo missing")?;
    assert_eq!(
        todo.retries, RETRY_CAP,
        "the counter must not move on refusal"
    );
    Ok(())
}

// Regressions from the 2026-09-01 recheck, one per confirmed defect.

#[test]
fn an_abandoned_sub_plan_is_not_drivable() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    init(&engine, vec![spec("parent job")?])?;
    engine.apply(owner(Op::Start {
        label: label("parent job")?,
    }))?;
    let out = engine.apply(owner(Op::Decompose {
        label: label("parent job")?,
        todos: vec![delegated_spec("small piece")?],
    }))?;
    let sub_id = out.subplan.ok_or("no sub-plan opened")?;
    engine.apply(at(
        &sub_id,
        Op::Start {
            label: label("small piece")?,
        },
    ))?;
    if let Ok(mut last) = stub.reap_last.lock() {
        *last = Some("history://small-piece/e4".parse::<Url>()?);
    }
    engine.apply(owner(Op::Supersede {
        reason: "wrong cut".to_owned(),
        todos: vec![spec("fresh job")?],
    }))?;
    assert_eq!(store.read(&sub_id)?.plan.state, PlanState::Abandoned);
    let spawned_before = stub.next.load(Ordering::SeqCst);
    let refused = engine.apply(at(
        &sub_id,
        Op::Retry {
            label: label("small piece")?,
            delegation: None,
        },
    ));
    assert!(
        matches!(refused, Err(PlanOpError::NotActive { .. })),
        "retry drove an abandoned sub-plan: {refused:?}"
    );
    let refused = engine.apply(at(
        &sub_id,
        Op::Start {
            label: label("small piece")?,
        },
    ));
    assert!(
        matches!(refused, Err(PlanOpError::NotActive { .. })),
        "start drove an abandoned sub-plan: {refused:?}"
    );
    assert_eq!(
        stub.next.load(Ordering::SeqCst),
        spawned_before,
        "a ghost child was spawned into abandoned work"
    );
    let piece = store
        .read(&sub_id)?
        .plan
        .todo(&label("small piece")?)
        .cloned()
        .ok_or("sub todo missing")?;
    assert!(matches!(piece.state, TodoState::Failed { .. }));
    engine.apply(at(&sub_id, Op::View { full: false }))?;
    Ok(())
}

#[test]
fn a_hand_edit_out_of_running_reaps_through_the_choke_point() -> TestResult {
    let (_temp, store, stub, engine) = harness(1)?;
    let out = init(
        &engine,
        vec![delegated_spec("first job")?, delegated_spec("second job")?],
    )?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("first job")?,
    }))?;
    let mut file = store.read(&id)?;
    for todo in &mut file.plan.todos {
        if todo.label == label("first job")? {
            todo.state = TodoState::Done { output: None };
        }
    }
    store.write(&file)?;
    let out = engine.apply(owner(Op::Start {
        label: label("second job")?,
    }))?;
    assert_eq!(
        stub.reaps.load(Ordering::SeqCst),
        1,
        "the folded edit out of Running must reap the live child"
    );
    assert_eq!(
        out.reaped.len(),
        1,
        "the fold's reap must surface in the outcome"
    );
    assert_eq!(out.spawned.len(), 1, "the freed slot still dispatches");
    Ok(())
}

#[test]
fn a_plan_whose_last_todo_failed_stays_reachable_unnamed() -> TestResult {
    let (_temp, store, _stub, engine) = harness(8)?;
    let out = init(&engine, vec![spec("only job")?])?;
    let id = out.plan.id.clone();
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Fail {
        label: label("only job")?,
        cause: "first attempt sank".to_owned(),
    }))?;
    assert_eq!(store.read(&id)?.plan.state, PlanState::Done);
    let out = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(
        out.plan.id, id,
        "unnamed view must find the plan whose last todo failed"
    );
    let out = engine.apply(owner(Op::Retry {
        label: label("only job")?,
        delegation: None,
    }))?;
    assert_eq!(out.plan.id, id);
    assert_eq!(out.plan.state, PlanState::Active, "retry must reopen it");
    engine.apply(owner(Op::Start {
        label: label("only job")?,
    }))?;
    engine.apply(owner(Op::Done {
        label: label("only job")?,
        output: None,
    }))?;
    let refused = engine.apply(owner(Op::View { full: false }));
    assert!(
        matches!(refused, Err(PlanOpError::NoPlan)),
        "a genuinely finished plan must never be resurrected: {refused:?}"
    );
    Ok(())
}

struct Probe;

impl OutputResolve for Probe {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        if url.to_string().contains("never-written") {
            Err("not found".to_owned())
        } else {
            Ok(None)
        }
    }
}

/// Serves bytes for both halves of the check: the declared schema and the
/// product the `done` names.
struct Served(&'static str, &'static str);

impl OutputResolve for Served {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        let rendered = url.to_string();
        if rendered.contains("schema") {
            Ok(Some(self.0.to_owned()))
        } else {
            Ok(Some(self.1.to_owned()))
        }
    }
}

fn declaring(schema: &str) -> Result<Delegation, Box<dyn Error>> {
    let mut declared = delegation();
    declared.output = Some(OutputSchema {
        schema: schema.parse::<Url>()?,
        extra: Map::new(),
    });
    Ok(declared)
}

const REPORT_SCHEMA: &str =
    r#"{"type":"object","required":["passed"],"properties":{"passed":{"type":"boolean"}}}"#;

#[test]
fn a_declared_output_is_validated_against_its_schema() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served(REPORT_SCHEMA, r#"{"passed":"yes"}"#)));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
        }],
    )?;
    engine.apply(owner(Op::Start {
        label: label("write the report")?,
    }))?;
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("kernel://main/report".parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::OutputMismatch { .. }) => {
            let told = error.to_string();
            assert!(
                told.contains("expected boolean"),
                "the refusal must name the mismatch: {told}"
            );
        }
        other => return Err(format!("expected a schema mismatch, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn a_product_that_satisfies_its_schema_completes() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served(REPORT_SCHEMA, r#"{"passed":true}"#)));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
        }],
    )?;
    engine.apply(owner(Op::Start {
        label: label("write the report")?,
    }))?;
    let out = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.json".parse::<Url>()?),
    }))?;
    let todo = out
        .plan
        .todo(&label("write the report")?)
        .ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Done { output: Some(_) }));
    Ok(())
}

#[test]
fn a_schema_that_is_not_json_refuses_the_done_naming_the_schema() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Served("not json at all", "{}")));
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declaring("local://schemas/report.json")?),
        }],
    )?;
    engine.apply(owner(Op::Start {
        label: label("write the report")?,
    }))?;
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.json".parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::UnusableSchema { .. }) => {
            assert!(
                error.to_string().contains("local://schemas/report.json"),
                "the refusal must name the schema: {error}"
            );
        }
        other => return Err(format!("expected an unusable-schema refusal, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn a_hand_edit_that_swaps_the_running_agent_reaps_the_first_child() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }))?;
    let reaped_before = stub.reaps.load(Ordering::SeqCst);
    let mut file = store.read(&out.plan.id)?;
    let todo = file
        .plan
        .todos
        .get_mut(0)
        .ok_or("the plan lost its only todo")?;
    todo.state = TodoState::Running {
        by: AgentId::new("someone-else")?,
    };
    store.write(&file)?;
    engine.apply(owner(Op::View { full: false }))?;
    engine.apply(owner(Op::Append {
        todos: vec![spec("a second job")?],
    }))?;
    assert_eq!(
        stub.reaps.load(Ordering::SeqCst),
        reaped_before.saturating_add(1),
        "swapping the running agent by hand must reap the child it replaced"
    );
    Ok(())
}

#[test]
fn a_supersede_refused_on_its_new_cut_kills_no_child() -> TestResult {
    let (_temp, store, stub, engine) = harness(8)?;
    let out = init(&engine, vec![delegated_spec("delegated job")?])?;
    engine.apply(owner(Op::Start {
        label: label("delegated job")?,
    }))?;
    let reaped_before = stub.reaps.load(Ordering::SeqCst);
    let refused = engine.apply(owner(Op::Supersede {
        reason: "rethink".to_owned(),
        todos: vec![spec("same label")?, spec("same label")?],
    }));
    assert!(matches!(refused, Err(PlanOpError::LabelNotUnique { .. })));
    assert_eq!(
        stub.reaps.load(Ordering::SeqCst),
        reaped_before,
        "a supersede refused on its own new cut must not have killed anything first"
    );
    let file = store.read(&out.plan.id)?;
    let todo = file
        .plan
        .todo(&label("delegated job")?)
        .ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Running { .. }));
    Ok(())
}

#[test]
fn a_declared_output_must_resolve_at_done() -> TestResult {
    let (_temp, _store, _stub, engine) = harness(8)?;
    let engine = engine.with_output_resolve(Arc::new(Probe));
    let mut declared = delegation();
    declared.output = Some(OutputSchema {
        schema: "local://schemas/report.json".parse::<Url>()?,
        extra: Map::new(),
    });
    init(
        &engine,
        vec![TodoSpec {
            label: label("write the report")?,
            after: Vec::new(),
            delegation: Some(declared),
        }],
    )?;
    engine.apply(owner(Op::Start {
        label: label("write the report")?,
    }))?;
    let unresolved = "local://nowhere/never-written.txt";
    let refused = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some(unresolved.parse::<Url>()?),
    }));
    match refused {
        Err(error @ PlanOpError::UnresolvedOutput { .. }) => {
            assert!(
                error.to_string().contains(unresolved),
                "the refusal must name the url that did not resolve: {error}"
            );
        }
        other => {
            return Err(format!("expected an unresolved-output refusal, got {other:?}").into());
        }
    }
    let out = engine.apply(owner(Op::Done {
        label: label("write the report")?,
        output: Some("local://reports/final.txt".parse::<Url>()?),
    }))?;
    let todo = out
        .plan
        .todo(&label("write the report")?)
        .ok_or("todo missing")?;
    assert!(matches!(todo.state, TodoState::Done { output: Some(_) }));
    Ok(())
}

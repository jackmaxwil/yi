#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};

use yi_kernel::client::HostHandlers;
use yi_runtime::HostRegistry;
use yi_runtime::plan::ops::{Actor, Delegate, PlanEngine};
use yi_runtime::plan::store::{PlanFile, PlanStore};
use yi_runtime::plan::{CanonicalPlanError, PlanService};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::plan::PlanVersion;
use yi_types::plan::doc::{
    GoalText, Plan, PlanId, PlanTier, RetryCount, Todo, TodoLabel, TodoState, TouchCount,
};

type TestResult = Result<(), Box<dyn Error>>;

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "plan-view-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

fn todo(label: &str, state: TodoState) -> Result<Todo, Box<dyn Error>> {
    Ok(Todo {
        label: TodoLabel::new(label)?,
        after: Vec::new(),
        state,
        delegation: None,
        subplan: None,
        retries: RetryCount::default(),
        children: Vec::new(),
        extra: serde_json::Map::new(),
    })
}

fn doc_plan(id: &str, touched: u64, todos: Vec<Todo>) -> Result<Plan, Box<dyn Error>> {
    let mut plan = Plan::opening(
        PlanId::new(id)?,
        GoalText::new("ship the seam end to end")?,
        PlanTier::Root,
        todos,
    );
    plan.touched = TouchCount(touched);
    Ok(plan)
}

fn write_plan(dir: &Path, plan: Plan) -> TestResult {
    PlanStore::open(dir.to_path_buf())?.write(&PlanFile {
        plan,
        body: String::new(),
    })?;
    Ok(())
}

fn point_fact_at(store: &yi_session::SharedSession, id: &str) -> TestResult {
    yi_session::lock_session(store).set_plan(yi_types::plan::Plan {
        version: PlanVersion(1),
        tasks: Vec::new(),
        created: 0,
        updated: 0,
        doc: Some(id.to_owned()),
        extra: serde_json::Map::new(),
    })?;
    Ok(())
}

type Harness = (Arc<PlanService>, Arc<Mutex<Vec<AgentMessage>>>);

fn harness(dir: &Path, store: &yi_session::SharedSession, stale_turns: u64) -> Harness {
    let handle = store.clone();
    let delivered: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let service = Arc::new(
        PlanService::new(
            Arc::new(move || Some(handle.clone())),
            Arc::new(move |message, _mode| {
                if let Ok(mut queue) = sink.lock() {
                    queue.push(message);
                }
            }),
        )
        .with_plans_dir(dir.to_path_buf())
        .with_stale_turns(Some(stale_turns)),
    );
    (service, delivered)
}

fn assistant_turn_end() -> AgentEvent {
    AgentEvent::MessageEnd {
        message: yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_text("worked")],
            StopReason::Stop,
        ),
    }
}

fn reminder_count(delivered: &Arc<Mutex<Vec<AgentMessage>>>) -> usize {
    delivered
        .lock()
        .map(|queue| {
            queue
                .iter()
                .filter(|message| {
                    matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "reminder")
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn the_doc_pointer_resolves_to_the_canonical_file() -> TestResult {
    let dir = Scratch::new("yi-plan-view-pointer")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan("pointed-at", 3, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    // A second Active root proves the pointer wins over the scan.
    write_plan(
        &dir,
        doc_plan("a-decoy", 1, vec![todo("noise", TodoState::Pending)?])?,
    )?;
    point_fact_at(&store, "pointed-at")?;
    let (service, _delivered) = harness(&dir, &store, 12);
    let plan = service.read_plan()?;
    assert_eq!(plan.id.as_str(), "pointed-at");
    assert_eq!(plan.touched, TouchCount(3));
    Ok(())
}

#[test]
fn without_a_pointer_the_active_root_is_the_plan() -> TestResult {
    let dir = Scratch::new("yi-plan-view-fallback")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan("only-root", 2, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    let (service, _delivered) = harness(&dir, &store, 12);
    assert_eq!(service.read_plan()?.id.as_str(), "only-root");
    Ok(())
}

#[test]
fn a_bad_pointer_is_a_typed_refusal_and_no_plan_is_named() -> TestResult {
    let dir = Scratch::new("yi-plan-view-bad-pointer")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan("real-plan", 1, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    point_fact_at(&store, "NOT_a.plan.id")?;
    let (service, _delivered) = harness(&dir, &store, 12);
    match service.read_plan() {
        Err(CanonicalPlanError::Pointer { id, .. }) => assert_eq!(id, "NOT_a.plan.id"),
        other => return Err(format!("expected a pointer refusal, got {other:?}").into()),
    }
    let empty = Scratch::new("yi-plan-view-bad-pointer-empty")?;
    let (unpointed, _delivered) = harness(&empty, &memory_store(), 12);
    match unpointed.read_plan() {
        Err(CanonicalPlanError::NoPlanOpen { dir }) => assert_eq!(dir, *empty),
        other => return Err(format!("expected no-plan, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn summary_and_frontier_render_the_document() -> TestResult {
    let by = yi_types::plan::doc::AgentId::new("kid")?;
    let plan = doc_plan(
        "render-me",
        4,
        vec![
            todo("cut the seam", TodoState::Done { output: None })?,
            todo("build it", TodoState::Running { by })?,
            todo("ship it", TodoState::Pending)?,
        ],
    )?;
    let summary = yi_runtime::plan::summary_line(&plan);
    assert!(
        summary.contains("plan render-me v1 touched 4")
            && summary.contains("1 ready, 1 running, 0 blocked, 1 done of 3"),
        "{summary}"
    );
    let frontier = yi_runtime::plan::frontier_text(&plan);
    assert!(
        frontier.contains("- ship it") && frontier.contains("In progress: build it (by kid)"),
        "{frontier}"
    );
    Ok(())
}

#[test]
fn stale_plan_reminds_once_then_latches_until_touched_moves() -> TestResult {
    let dir = Scratch::new("yi-plan-view-stale")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan("goes-stale", 1, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    let (service, delivered) = harness(&dir, &store, 2);
    for _ in 0..6 {
        service.observe(&assistant_turn_end());
    }
    assert_eq!(
        reminder_count(&delivered),
        1,
        "one reminder, then the latch"
    );
    // touched moves (version does not) and the latch clears for a fresh streak.
    write_plan(
        &dir,
        doc_plan("goes-stale", 2, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    for _ in 0..4 {
        service.observe(&assistant_turn_end());
    }
    assert_eq!(
        reminder_count(&delivered),
        2,
        "the moved counter re-arms it"
    );
    Ok(())
}

#[test]
fn a_touched_move_pokes_the_change_hook_with_the_new_document() -> TestResult {
    let dir = Scratch::new("yi-plan-view-hook")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan("watched", 1, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    let (service, _delivered) = harness(&dir, &store, 12);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    service.set_on_change(Arc::new(move |plan| {
        if let Ok(mut lines) = sink.lock() {
            lines.push(yi_runtime::plan::summary_line(plan));
        }
    }));
    service.observe(&assistant_turn_end());
    service.observe(&assistant_turn_end());
    write_plan(
        &dir,
        doc_plan("watched", 2, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    service.observe(&assistant_turn_end());
    let lines = seen.lock().map_err(|error| error.to_string())?.clone();
    assert_eq!(
        lines,
        vec![
            "plan watched v1 touched 1: 1 ready, 0 running, 0 blocked, 0 done of 1".to_owned(),
            "plan watched v1 touched 2: 1 ready, 0 running, 0 blocked, 0 done of 1".to_owned(),
        ],
        "the hook fires once per touched move, never per turn"
    );
    Ok(())
}

#[test]
fn plan_get_serializes_the_document_with_ready_and_finished() -> TestResult {
    let dir = Scratch::new("yi-plan-view-get")?;
    let store = memory_store();
    write_plan(
        &dir,
        doc_plan(
            "served",
            1,
            vec![
                todo("first", TodoState::Done { output: None })?,
                todo("second", TodoState::Pending)?,
            ],
        )?,
    )?;
    let (service, _delivered) = harness(&dir, &store, 12);
    let value = service.get().map_err(|error| error.to_string())?;
    assert_eq!(value["plan"], serde_json::json!("served"));
    assert_eq!(value["ready"], serde_json::json!(["second"]));
    assert_eq!(value["finished"], serde_json::json!(false));
    Ok(())
}

struct NoChildren;

impl Delegate for NoChildren {
    fn spawn(
        &self,
        _at: &yi_types::plan::doc::TodoAddr,
        _delegation: &yi_types::plan::doc::Delegation,
    ) -> Result<yi_types::plan::doc::AgentId, String> {
        Err("no children in this test".to_owned())
    }

    fn reap(
        &self,
        _agent: &yi_types::plan::doc::AgentId,
        _supplied: &[yi_types::url::Url],
    ) -> Result<Option<yi_types::url::Url>, String> {
        Ok(None)
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

async fn plan_op(
    registry: &HostRegistry,
    payload: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let payload = payload
        .as_object()
        .cloned()
        .ok_or("payload is not an object")?;
    registry
        .dispatch("plan.op", payload)
        .ok_or("plan.op is not registered")?
        .await
}

/// Guards `check_actor`'s child arm: let `Actor::Child` past `View` and the `done` below
/// lands on the parent's plan.
#[tokio::test]
async fn a_child_kernels_plan_op_is_refused_beyond_view() -> TestResult {
    let dir = Scratch::new("yi-plan-op-child")?;
    write_plan(
        &dir,
        doc_plan(
            "parents",
            3,
            vec![todo(
                "cut",
                TodoState::Running {
                    by: yi_types::plan::doc::AgentId::new("main")?,
                },
            )?],
        )?,
    )?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(NoChildren),
    ));
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(
        engine,
        Actor::Child(yi_types::plan::doc::AgentId::new("helper")?),
        &mut registry,
    );
    let viewed = plan_op(
        &registry,
        serde_json::json!({"request_id": "r1", "op": "view", "args": {}}),
    )
    .await?;
    assert_eq!(viewed["ok"], serde_json::json!(true), "{viewed:?}");
    assert_eq!(viewed["revision"], serde_json::json!(3));
    assert!(
        viewed["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("plan parents")),
        "{viewed:?}"
    );
    let stepped = plan_op(
        &registry,
        serde_json::json!({"request_id": "r2", "op": "done", "args": {"label": "cut"}}),
    )
    .await?;
    assert_eq!(stepped["ok"], serde_json::json!(false), "{stepped:?}");
    assert_eq!(stepped["refusal"]["code"], serde_json::json!("not_owner"));
    assert!(
        stepped["refusal"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("only the plan owner")),
        "{stepped:?}"
    );
    let file = PlanStore::open(dir.to_path_buf())?.read(&PlanId::new("parents")?)?;
    assert!(
        matches!(file.plan.todos[0].state, TodoState::Running { .. }),
        "a child's done must not land"
    );
    assert_eq!(file.plan.touched, TouchCount(3));
    let again = plan_op(
        &registry,
        serde_json::json!({"request_id": "r2", "op": "view", "args": {}}),
    )
    .await?;
    assert_eq!(
        again, stepped,
        "a duplicate request_id returns the cached reply"
    );
    let stale = plan_op(
        &registry,
        serde_json::json!({"request_id": "r3", "op": "view", "args": {"actor": "agent://main"}}),
    )
    .await?;
    assert_eq!(
        stale["refusal"]["code"],
        serde_json::json!("bad_args"),
        "{stale:?}"
    );
    let shapeless = plan_op(&registry, serde_json::json!({"op": "view"})).await;
    assert!(
        shapeless.is_err(),
        "a missing request_id is a transport error"
    );
    Ok(())
}

/// Guards the revision guard's target: `expected_revision` is compared on the plan the op
/// runs on, which `args.plan` may name, never on the session's default plan.
#[tokio::test]
async fn expected_revision_is_compared_on_the_plan_the_op_names() -> TestResult {
    let dir = Scratch::new("yi-plan-op-revision")?;
    write_plan(&dir, doc_plan("alpha", 3, Vec::new())?)?;
    write_plan(&dir, doc_plan("beta", 7, Vec::new())?)?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(NoChildren),
    ));
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(engine, Actor::Owner, &mut registry);
    let append = serde_json::json!({"plan": "beta", "todos": [{"label": "cut"}]});
    let stale = plan_op(
        &registry,
        serde_json::json!({"request_id": "g1", "expected_revision": 3, "op": "append", "args": append}),
    )
    .await?;
    assert_eq!(stale["ok"], serde_json::json!(false), "{stale:?}");
    assert_eq!(
        stale["refusal"]["code"],
        serde_json::json!("stale_revision")
    );
    assert_eq!(
        stale["revision"],
        serde_json::json!(7),
        "beta's revision, not alpha's"
    );
    let store = PlanStore::open(dir.to_path_buf())?;
    assert!(
        store.read(&PlanId::new("beta")?)?.plan.todos.is_empty(),
        "a stale append must not land"
    );
    let landed = plan_op(
        &registry,
        serde_json::json!({"request_id": "g2", "expected_revision": 7, "op": "append", "args": append}),
    )
    .await?;
    assert_eq!(landed["ok"], serde_json::json!(true), "{landed:?}");
    assert_eq!(landed["revision"], serde_json::json!(8));
    assert_eq!(store.read(&PlanId::new("beta")?)?.plan.todos.len(), 1);
    assert_eq!(
        store.read(&PlanId::new("alpha")?)?.plan.touched,
        TouchCount(3)
    );
    Ok(())
}

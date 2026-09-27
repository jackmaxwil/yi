use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};

use yi_kernel::client::HostHandlers;
use yi_runtime::HostRegistry;
use yi_runtime::plan::ops::{Actor, Delegate, PlanEngine};
use yi_runtime::plan::store::PlanStore;
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
        note: None,
        attempt: yi_types::plan::doc::AttemptId::FIRST,
        refusals: 0,
        contract: None,
        contract_hash: None,
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
    PlanStore::open(dir.to_path_buf())?.write(&plan)?;
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
            todo(
                "cut the seam",
                TodoState::Done {
                    output: None,
                    resolution: None,
                },
            )?,
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
                todo(
                    "first",
                    TodoState::Done {
                        output: None,
                        resolution: None,
                    },
                )?,
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

/// A child spawned for a todo: its agent id is the todo's own label.
struct Named;

impl Delegate for Named {
    fn spawn(
        &self,
        at: &yi_types::plan::doc::TodoAddr,
        _delegation: &yi_types::plan::doc::Delegation,
    ) -> Result<yi_types::plan::doc::AgentId, String> {
        yi_types::plan::doc::AgentId::new(at.todo.as_str()).map_err(|error| error.to_string())
    }

    fn reap(
        &self,
        _agent: &yi_types::plan::doc::AgentId,
        _supplied: &[yi_types::url::Url],
    ) -> Result<Option<yi_types::url::Url>, String> {
        Ok(None)
    }
}

fn delegated(label: &str) -> Result<yi_runtime::plan::ops::TodoSpec, Box<dyn Error>> {
    Ok(yi_runtime::plan::ops::TodoSpec {
        label: TodoLabel::new(label)?,
        after: Vec::new(),
        delegation: Some(yi_types::plan::doc::Delegation {
            spec: yi_types::plan::doc::SpawnSpec {
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
            accept: yi_types::plan::doc::Check::Command("true".to_owned()),
            output: None,
            context: Vec::new(),
            note: None,
            extra: serde_json::Map::new(),
        }),
        contract: None,
        children: Vec::new(),
    })
}

/// Guards `check_actor`'s `Submit` grant and `done::admit`: a child's `plan.op` may submit an
/// output for the attempt it is running and for nothing else.
#[tokio::test]
async fn a_child_may_submit_only_for_its_own_attempt() -> TestResult {
    let dir = Scratch::new("yi-plan-op-submit")?;
    let engine = Arc::new(
        PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Named))
            .with_width(std::num::NonZeroUsize::MIN.saturating_add(1)),
    );
    let owner = |op: yi_runtime::plan::ops::Op| yi_runtime::plan::ops::OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    };
    engine.apply(owner(yi_runtime::plan::ops::Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![delegated("cut")?, delegated("ship")?],
    }))?;
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(
        Arc::clone(&engine),
        Actor::Child(yi_types::plan::doc::AgentId::new("cut")?),
        &mut registry,
    );
    let submit = |request: &str, label: &str, attempt: u32| {
        serde_json::json!({
            "request_id": request, "op": "submit",
            "args": {"label": label, "attempt": attempt, "output": "local://out/cut.json"}
        })
    };
    let own = plan_op(&registry, submit("s1", "cut", 1)).await?;
    assert_eq!(own["ok"], serde_json::json!(true), "{own:?}");
    assert!(
        own["text"]
            .as_str()
            .is_some_and(|text| text.contains("submitted local://out/cut.json")),
        "{own:?}"
    );
    let theirs = plan_op(&registry, submit("s2", "ship", 1)).await?;
    assert_eq!(theirs["ok"], serde_json::json!(false), "{theirs:?}");
    assert!(
        theirs["refusal"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("not running by cut")),
        "{theirs:?}"
    );
    let stale = plan_op(&registry, submit("s3", "cut", 2)).await?;
    assert_eq!(stale["ok"], serde_json::json!(false), "{stale:?}");
    assert!(
        stale["refusal"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("on attempt 1, not attempt 2")),
        "{stale:?}"
    );
    let done = plan_op(
        &registry,
        serde_json::json!({"request_id": "s4", "op": "done", "args": {"label": "cut"}}),
    )
    .await?;
    assert_eq!(done["refusal"]["code"], serde_json::json!("not_owner"));
    let file =
        PlanStore::open(dir.to_path_buf())?.read(&PlanId::slug("ship the seam end to end")?)?;
    let cut = file.todo(&TodoLabel::new("cut")?).ok_or("cut")?;
    assert!(matches!(cut.state, TodoState::Running { .. }));
    assert_eq!(
        cut.extra.get("submitted"),
        Some(&serde_json::json!("local://out/cut.json"))
    );
    let ship = file.todo(&TodoLabel::new("ship")?).ok_or("ship")?;
    assert!(ship.extra.get("submitted").is_none(), "{ship:?}");
    Ok(())
}

/// Incident: `Todo.submit(artifact)` sends its product as a blob, and the artifacts rider took
/// the owner alone, so the one agent a submit is for could not make the documented call (#478).
#[tokio::test]
async fn a_child_stores_the_product_of_the_attempt_it_submits() -> TestResult {
    let dir = Scratch::new("yi-plan-op-product")?;
    let engine = Arc::new(
        PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Named))
            .with_width(std::num::NonZeroUsize::MIN.saturating_add(1)),
    );
    let owner = |op: yi_runtime::plan::ops::Op| yi_runtime::plan::ops::OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    };
    engine.apply(owner(yi_runtime::plan::ops::Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![delegated("cut")?, delegated("ship")?],
    }))?;
    let id = PlanId::slug("ship the seam end to end")?;
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(
        Arc::clone(&engine),
        Actor::Child(yi_types::plan::doc::AgentId::new("cut")?),
        &mut registry,
    );
    let product = "{\"passed\":12}";
    let digest = yi_types::plan::canonical::Digest::of(product.as_bytes());
    let url = format!("plan://{id}/artifacts/{}", digest.hex());
    let blob = serde_json::json!({"media_type": "application/json", "text": product});
    let send = |request: &str, op: &str, args: serde_json::Value, blobs: serde_json::Value| serde_json::json!({"request_id": request, "op": op, "artifacts": blobs, "args": args});
    let submitting = |label: &str| serde_json::json!({"label": label, "attempt": 1, "output": url});
    let own = plan_op(
        &registry,
        send("p1", "submit", submitting("cut"), serde_json::json!([blob])),
    )
    .await?;
    assert_eq!(own["ok"], serde_json::json!(true), "{own:?}");
    assert_eq!(
        PlanStore::open(dir.to_path_buf())?
            .artifacts(&id)
            .get(&digest)?,
        product.as_bytes(),
        "the child's product is not in the plan store"
    );
    // Every other blob write a non-owner can try: another todo's attempt, a batch beside the
    // product, an op past a view, which authority refuses before any parse, and a view.
    let stores = "only the plan owner stores artifacts";
    for (request, op, args, blobs, said) in [
        (
            "p2",
            "submit",
            submitting("ship"),
            serde_json::json!([blob]),
            stores,
        ),
        (
            "p3",
            "submit",
            submitting("cut"),
            serde_json::json!([blob, blob]),
            stores,
        ),
        (
            "p4",
            "done",
            serde_json::json!({"label": "cut", "output": url}),
            serde_json::json!([blob]),
            "only the plan owner may done; your plan tool only views",
        ),
        (
            "p5",
            "view",
            serde_json::json!({}),
            serde_json::json!([blob]),
            stores,
        ),
    ] {
        let refused = plan_op(&registry, send(request, op, args, blobs)).await?;
        assert_eq!(refused["ok"], serde_json::json!(false), "{refused:?}");
        assert_eq!(refused["refusal"]["code"], serde_json::json!("not_owner"));
        assert!(
            refused["refusal"]["message"]
                .as_str()
                .is_some_and(|text| text.starts_with(said)),
            "{refused:?}"
        );
    }
    Ok(())
}

/// Dies with `ops::runs`: compare `Running { by }` to the actor's rendered word and a child
/// named `main` submits, and stores a blob, on the attempt the owner runs inline.
#[tokio::test]
async fn a_child_named_main_is_not_the_owner_of_an_inline_todo() -> TestResult {
    let dir = Scratch::new("yi-plan-op-main")?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(Named),
    ));
    let owner = |op: yi_runtime::plan::ops::Op| yi_runtime::plan::ops::OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    };
    let inline = yi_runtime::plan::ops::TodoSpec {
        delegation: None,
        ..delegated("cut")?
    };
    engine.apply(owner(yi_runtime::plan::ops::Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![inline],
    }))?;
    engine.apply(owner(yi_runtime::plan::ops::Op::Start {
        label: TodoLabel::new("cut")?,
    }))?;
    let id = PlanId::slug("ship the seam end to end")?;
    let mut registry = HostRegistry::default();
    let main = yi_types::plan::doc::AgentId::new("main")?;
    yi_runtime::plan::request::register(Arc::clone(&engine), Actor::Child(main), &mut registry);
    let product = "{\"forged\":true}";
    let digest = yi_types::plan::canonical::Digest::of(product.as_bytes());
    let url = format!("plan://{id}/artifacts/{}", digest.hex());
    let args = serde_json::json!({"label": "cut", "attempt": 1, "output": url});
    let blob = serde_json::json!([{"media_type": "application/json", "text": product}]);
    for (request, blobs) in [("m1", blob), ("m2", serde_json::json!([]))] {
        let payload = serde_json::json!({"request_id": request, "op": "submit", "args": args, "artifacts": blobs});
        let refused = plan_op(&registry, payload).await?;
        assert_eq!(refused["ok"], serde_json::json!(false), "{refused:?}");
    }
    let store = PlanStore::open(dir.to_path_buf())?;
    assert!(
        store.artifacts(&id).get(&digest).is_err(),
        "no blob lands on the owner's attempt"
    );
    let cut = store.read(&id)?;
    let cut = cut.todo(&TodoLabel::new("cut")?).ok_or("cut")?;
    assert!(cut.extra.get("submitted").is_none(), "{cut:?}");
    Ok(())
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
    // Dies with the control: drop the owner check in `store_artifacts` and a child writes
    // a blob into its parent's plan, whatever the op it rides does.
    let blob = "a child's bytes";
    let smuggled = plan_op(
        &registry,
        serde_json::json!({"request_id": "r3", "op": "view", "args": {},
            "artifacts": [{"media_type": "text/plain", "text": blob}]}),
    )
    .await?;
    assert_eq!(smuggled["ok"], serde_json::json!(false), "{smuggled:?}");
    let id = PlanId::new("parents")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    assert!(
        store
            .artifacts(&id)
            .get(&yi_types::plan::canonical::Digest::of(blob.as_bytes()))
            .is_err(),
        "only the owner stores artifacts"
    );
    let file = store.read(&id)?;
    assert!(
        matches!(file.todos[0].state, TodoState::Running { .. }),
        "a child's done must not land"
    );
    assert_eq!(file.touched, TouchCount(3));
    let again = plan_op(
        &registry,
        serde_json::json!({"request_id": "r2", "op": "done", "args": {"label": "cut"}}),
    )
    .await?;
    assert_eq!(
        again, stepped,
        "a duplicate of a refusal that never reached the journal returns the cached reply"
    );
    let other = plan_op(
        &registry,
        serde_json::json!({"request_id": "r2", "op": "view", "args": {}}),
    )
    .await?;
    assert_eq!(
        other["ok"],
        serde_json::json!(true),
        "the same id with other args is not the cached refusal: {other:?}"
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

/// The journal is the deduplicator (plan section 5.3): a reused `request_id` with other args
/// is refused, the same args replay, and an id the journal could not hold is refused at the
/// boundary rather than swapped for a minted one.
#[tokio::test]
async fn a_reused_request_id_is_refused_by_the_journal_and_a_bad_one_by_the_parser() -> TestResult {
    let dir = Scratch::new("yi-plan-op-dedup")?;
    write_plan(
        &dir,
        doc_plan("dedup", 1, vec![todo("cut", TodoState::Pending)?])?,
    )?;
    let engine = Arc::new(PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(NoChildren),
    ));
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(engine, Actor::Owner, &mut registry);
    let first = plan_op(
        &registry,
        serde_json::json!({"request_id": "dup", "op": "append", "args": {"todos": [{"label": "one"}]}}),
    )
    .await?;
    assert_eq!(first["ok"], serde_json::json!(true), "{first:?}");
    let reused = plan_op(
        &registry,
        serde_json::json!({"request_id": "dup", "op": "append", "args": {"todos": [{"label": "two"}]}}),
    )
    .await?;
    assert_eq!(reused["ok"], serde_json::json!(false), "{reused:?}");
    assert_eq!(
        reused["refusal"]["code"],
        serde_json::json!("request_id_reused"),
        "{reused:?}"
    );
    let replayed = plan_op(
        &registry,
        serde_json::json!({"request_id": "dup", "op": "append", "args": {"todos": [{"label": "one"}]}}),
    )
    .await?;
    assert_eq!(replayed["ok"], serde_json::json!(true), "{replayed:?}");
    assert_eq!(replayed["revision"], first["revision"], "nothing ran twice");
    let labels: Vec<String> = PlanStore::open(dir.to_path_buf())?
        .read(&PlanId::new("dedup")?)?
        .todos
        .iter()
        .map(|todo| todo.label.as_str().to_owned())
        .collect();
    assert_eq!(labels, vec!["cut".to_owned(), "one".to_owned()]);
    let spaced = plan_op(
        &registry,
        serde_json::json!({"request_id": "req 1", "op": "view", "args": {}}),
    )
    .await?;
    assert_eq!(spaced["ok"], serde_json::json!(false), "{spaced:?}");
    assert_eq!(spaced["refusal"]["code"], serde_json::json!("bad_args"));
    assert_eq!(spaced["request_id"], serde_json::json!("req 1"));
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
        serde_json::json!("stale_revision"),
        "{stale:?}"
    );
    assert_eq!(
        stale["revision"],
        serde_json::json!(7),
        "beta's revision, not alpha's"
    );
    let store = PlanStore::open(dir.to_path_buf())?;
    assert!(
        store.read(&PlanId::new("beta")?)?.todos.is_empty(),
        "a stale append must not land"
    );
    let landed = plan_op(
        &registry,
        serde_json::json!({"request_id": "g2", "expected_revision": 7, "op": "append", "args": append}),
    )
    .await?;
    assert_eq!(landed["ok"], serde_json::json!(true), "{landed:?}");
    assert_eq!(landed["revision"], serde_json::json!(8));
    assert_eq!(store.read(&PlanId::new("beta")?)?.todos.len(), 1);
    assert_eq!(store.read(&PlanId::new("alpha")?)?.touched, TouchCount(3));
    Ok(())
}

/// Guards the step table against a library scheduler (plan section 8.5): the ops a restart
/// strategy is tempted by are refused as `illegal_step` through `plan.op`, and the states stand.
#[tokio::test]
async fn a_shape_cannot_retry_a_done_or_drop_a_running_todo() -> TestResult {
    let dir = Scratch::new("yi-plan-op-shape-steps")?;
    let engine = Arc::new(
        PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Named))
            .with_width(std::num::NonZeroUsize::MIN.saturating_add(1)),
    );
    let mut registry = HostRegistry::default();
    yi_runtime::plan::request::register(Arc::clone(&engine), Actor::Owner, &mut registry);
    let op = |request: &str, op: &str, label: &str| serde_json::json!({"request_id": request, "op": op, "args": {"label": label}});
    let init = serde_json::json!({"request_id": "i", "op": "init", "args": {
        "goal": "fork and join", "todos": [{"label": "finished"}, {"label": "busy"}]}});
    assert_eq!(plan_op(&registry, init).await?["ok"], true);
    for (request, step, label) in [
        ("s1", "start", "finished"),
        ("d1", "done", "finished"),
        ("s2", "start", "busy"),
    ] {
        let reply = plan_op(&registry, op(request, step, label)).await?;
        assert_eq!(reply["ok"], true, "{reply:?}");
    }
    for (request, step, label) in [("r1", "retry", "finished"), ("x1", "drop", "busy")] {
        let reply = plan_op(&registry, op(request, step, label)).await?;
        assert_eq!(reply["ok"], false, "{step} {label}: {reply:?}");
        assert_eq!(reply["refusal"]["kind"], "illegal_step", "{reply:?}");
    }
    let file = PlanStore::open(dir.to_path_buf())?.read(&PlanId::slug("fork and join")?)?;
    let state = |label: &str| -> Result<TodoState, Box<dyn Error>> {
        Ok(file
            .todo(&TodoLabel::new(label)?)
            .ok_or("todo")?
            .state
            .clone())
    };
    assert!(matches!(state("finished")?, TodoState::Done { .. }));
    assert!(matches!(state("busy")?, TodoState::Running { .. }));
    Ok(())
}

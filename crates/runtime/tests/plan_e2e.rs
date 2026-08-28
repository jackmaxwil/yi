use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_runtime::goal::{DeliverFn, continuation_text};
use yi_runtime::plan::{NO_PLAN_ERROR, PLAN_EXISTS_ERROR, PlanService, SHRINK_ERROR};
use yi_types::event::AgentEvent;
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, StopReason, Usage};
use yi_types::plan::TaskState;
use yi_types::schedule::DeliveryMode;

type TestResult = Result<(), Box<dyn Error>>;

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "plan-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
        },
    )))
}

fn service_with_store() -> (
    Arc<PlanService>,
    yi_session::SharedSession,
    Arc<Mutex<Vec<AgentMessage>>>,
) {
    let store = memory_store();
    let handle = store.clone();
    let delivered: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let deliver: DeliverFn = Arc::new(move |message, _mode: DeliveryMode| {
        if let Ok(mut queue) = sink.lock() {
            queue.push(message);
        }
    });
    let service = Arc::new(PlanService::new(
        Arc::new(move || Some(handle.clone())),
        deliver,
    ));
    (service, store, delivered)
}

fn two_task_specs() -> Value {
    json!([
        {"title": "Parse config", "acceptance": "config tests pass", "check": "true"},
        {"title": "Wire flag", "acceptance": "--dry-run accepted", "deps": ["t1"]},
    ])
}

#[test]
fn create_validates_ids_deps_and_cycles() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    let dup = json!([
        {"id": "a", "title": "x", "acceptance": "y"},
        {"id": "a", "title": "z", "acceptance": "w"},
    ]);
    let error = service.create(&dup).err().ok_or("dup ids must fail")?;
    assert!(error.contains("duplicate task id a"), "{error}");

    let unknown = json!([{"id": "a", "title": "x", "acceptance": "y", "deps": ["ghost"]}]);
    let error = service
        .create(&unknown)
        .err()
        .ok_or("unknown dep must fail")?;
    assert!(error.contains("unknown task ghost"), "{error}");

    let cycle = json!([
        {"id": "a", "title": "x", "acceptance": "y", "deps": ["b"]},
        {"id": "b", "title": "z", "acceptance": "w", "deps": ["a"]},
    ]);
    let error = service.create(&cycle).err().ok_or("cycle must fail")?;
    assert!(error.contains("dependency cycle"), "{error}");
    Ok(())
}

#[test]
fn frontier_is_derived_from_deps_and_survives_the_store() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    let created = service.create(&two_task_specs())?;
    assert_eq!(created["frontier"], json!(["t1"]), "t2 waits on t1");

    let stored = yi_session::lock_session(&store)
        .plan()
        .ok_or("plan fact must persist in the store")?;
    assert_eq!(stored.tasks.len(), 2);
    assert_eq!(stored.frontier().len(), 1);

    service.update("t1", "running", None, None)?;
    let after = service.update("t1", "done", None, None)?;
    assert_eq!(after["frontier"], json!(["t2"]), "t1 done frees t2");
    Ok(())
}

#[test]
fn transitions_follow_the_table() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let error = service
        .update("t2", "blocked", None, None)
        .err()
        .ok_or("blocked without reason must fail")?;
    assert!(error.contains("requires a reason"), "{error}");

    service.update("t1", "done", None, None)?;
    let error = service
        .update("t1", "running", None, None)
        .err()
        .ok_or("done must be a sink")?;
    assert!(
        error.contains("reopen"),
        "the error must name the way back: {error}"
    );

    let error = service
        .update("ghost", "running", None, None)
        .err()
        .ok_or("unknown task must fail")?;
    assert!(error.contains("no task ghost"), "{error}");
    Ok(())
}

#[test]
fn failing_check_blocks_the_task_with_evidence() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(&json!([
        {"title": "impossible", "acceptance": "never true", "check": "echo case 7 diverges; exit 2"},
    ]))?;
    let error = service
        .update("t1", "done", None, None)
        .err()
        .ok_or("failing check must reject the claim")?;
    assert!(
        error.contains("rejected") && error.contains("case 7 diverges"),
        "{error}"
    );
    let stored = yi_session::lock_session(&store).plan().ok_or("plan")?;
    let task = stored
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?;
    assert_eq!(task.state, TaskState::Blocked);
    assert!(
        task.blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("case 7 diverges")),
        "evidence must persist on the task"
    );
    Ok(())
}

#[test]
fn schema_gates_the_done_claim() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&json!([
        {"title": "extract", "acceptance": "returns rows",
         "schema": {"type": "object", "required": ["rows"]}},
    ]))?;
    let error = service
        .update("t1", "done", None, None)
        .err()
        .ok_or("schema without evidence must fail")?;
    assert!(error.contains("evidence"), "{error}");

    // The rejection blocked the task; a fresh claim must come from a live state.
    service.update("t1", "running", None, None)?;
    let bad = json!({"count": 3});
    let error = service
        .update("t1", "done", Some(&bad), None)
        .err()
        .ok_or("mismatching evidence must fail")?;
    assert!(error.contains("missing required property rows"), "{error}");

    service.update("t1", "running", None, None)?;
    let good = json!({"rows": [1, 2]});
    let after = service.update("t1", "done", Some(&good), None)?;
    assert_eq!(after["finished"], json!(true));
    Ok(())
}

#[test]
fn plan_grows_freely_and_refuses_to_shrink() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let error = service
        .create(&two_task_specs())
        .err()
        .ok_or("second create must fail")?;
    assert_eq!(error, PLAN_EXISTS_ERROR);

    service.edit(
        "add",
        &json!({"tasks": [{"title": "Docs", "acceptance": "README names the flag"}]}),
    )?;
    let plan = service.get()?;
    assert_eq!(plan["tasks"].as_array().map(Vec::len), Some(3));

    let error = service
        .edit("remove", &json!({"task_id": "t1"}))
        .err()
        .ok_or("remove must be refused")?;
    assert_eq!(error, SHRINK_ERROR);

    service.update("t1", "done", None, None)?;
    service.edit("reopen", &json!({"task_id": "t1"}))?;
    let plan = service.get()?;
    assert_eq!(
        plan["frontier"],
        json!(["t1", "t3"]),
        "reopened t1 is pending again, beside the dep-free t3"
    );
    Ok(())
}

#[test]
fn continuation_prompt_carries_the_frontier() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let goal = Goal {
        objective: "ship it".to_owned(),
        status: GoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        created: 0,
        updated: 0,
        check: None,
        check_timeout_ms: None,
        check_failure: None,
        extra: serde_json::Map::new(),
    };
    let plan = yi_session::lock_session(&store).plan().ok_or("plan")?;
    let text = continuation_text(&goal, Some(&plan))?;
    assert!(
        text.contains("Ready tasks") && text.contains("t1: Parse config"),
        "frontier must reach the continuation prompt: {text}"
    );
    let no_plan = continuation_text(&goal, None)?;
    assert!(!no_plan.contains("Ready tasks"));
    Ok(())
}

fn assistant_turn_end() -> AgentEvent {
    AgentEvent::MessageEnd {
        message: AgentMessage::Assistant {
            content: Vec::new(),
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::zero(),
            stop_reason: StopReason::Stop,
            raw_stop_reason: None,
            end_turn: None,
            deferred: None,
            error_message: None,
            timestamp: 0,
        },
    }
}

#[test]
fn stale_plan_reminds_once_then_latches_until_it_moves() -> TestResult {
    let (service, _store, delivered) = service_with_store();
    service.create(&two_task_specs())?;
    for _turn in 0..40 {
        service.observe(&assistant_turn_end());
    }
    let count = delivered
        .lock()
        .map(|queue| {
            queue
                .iter()
                .filter(|message| {
                    matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "reminder")
                })
                .count()
        })
        .map_err(|_| "lock")?;
    assert_eq!(
        count, 1,
        "one reminder per staleness episode, latched after"
    );

    service.update("t1", "running", None, None)?;
    for _turn in 0..40 {
        service.observe(&assistant_turn_end());
    }
    let count = delivered
        .lock()
        .map(|queue| {
            queue
                .iter()
                .filter(|message| {
                    matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "reminder")
                })
                .count()
        })
        .map_err(|_| "lock")?;
    assert_eq!(count, 2, "a version move re-arms the reminder");
    Ok(())
}

#[test]
fn no_plan_is_a_named_error() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    let error = service.get().err().ok_or("must fail")?;
    assert_eq!(error, NO_PLAN_ERROR);
    Ok(())
}

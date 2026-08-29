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

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yi-ladder-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// A done claim on a blocked task is an illegal transition, so a streak is
/// scripted as claim -> running -> claim, exactly as the model must drive it.
fn claim_again(service: &PlanService, task: &str) -> Result<String, Box<dyn Error>> {
    if let Some(plan) = service.read_plan()
        && plan
            .task(&yi_types::plan::TaskId(task.to_owned()))
            .is_some_and(|task| task.state == TaskState::Blocked)
    {
        service.update(task, "running", None, None)?;
    }
    Ok(service
        .update(task, "done", None, None)
        .err()
        .ok_or("a red check must refuse the done claim")?)
}

#[test]
fn consecutive_reds_walk_the_escalation_ladder() -> TestResult {
    let (service, _store, delivered) = service_with_store();
    service.create(&json!([
        {"title": "impossible", "acceptance": "never true", "check": "echo case 7 diverges; exit 2"},
    ]))?;

    let first = claim_again(&service, "t1")?;
    assert!(
        !first.contains("stayed red through"),
        "one red is a retry, not an escalation: {first}"
    );
    let second = claim_again(&service, "t1")?;
    assert!(
        second.contains("stayed red through 2 attempts")
            && second.contains("do not retry the same approach"),
        "the second red must demand a structural change: {second}"
    );
    let third = claim_again(&service, "t1")?;
    assert!(
        third.contains("stayed red through 3 attempts") && third.contains("ask the user"),
        "the third red must demand abstain-and-ask: {third}"
    );

    let reminders: Vec<String> = delivered
        .lock()
        .map(|queue| {
            queue
                .iter()
                .filter_map(|message| match message {
                    AgentMessage::Custom {
                        custom_type,
                        content: yi_types::message::UserContent::Text(text),
                        ..
                    } if custom_type == "reminder" => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .map_err(|_| "lock")?;
    assert_eq!(
        reminders.len(),
        2,
        "the demand reaches the transcript once per escalated red: {reminders:?}"
    );
    assert!(
        reminders
            .first()
            .is_some_and(|text| text.starts_with("t1:") && text.contains("split it")),
        "the reminder names the task and the demand: {reminders:?}"
    );
    Ok(())
}

#[test]
fn a_green_check_resets_the_streak() -> TestResult {
    let dir = scratch("reset");
    let flag = dir.join("ok");
    let (service, _store, _delivered) = service_with_store();
    service.create(&json!([
        {"title": "gated", "acceptance": "flag exists",
         "check": format!("test -f {} || {{ echo flag missing; exit 3; }}", flag.display())},
    ]))?;

    claim_again(&service, "t1")?;
    let escalated = claim_again(&service, "t1")?;
    assert!(
        escalated.contains("stayed red through 2 attempts"),
        "{escalated}"
    );

    std::fs::write(&flag, "ok")?;
    service.update("t1", "running", None, None)?;
    service.update("t1", "done", None, None)?;
    std::fs::remove_file(&flag)?;
    service.edit("reopen", &json!({"task_id": "t1"}))?;

    let after = claim_again(&service, "t1")?;
    assert!(
        !after.contains("stayed red through"),
        "a task that went green starts its next streak at rung one: {after}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_repeated_failure_demands_a_premise_recheck() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&json!([
        {"title": "assume", "acceptance": "the premise holds"},
        {"title": "build", "acceptance": "suite green", "deps": ["t1"],
         "check": "echo case 7 diverges; exit 2"},
    ]))?;
    let first = claim_again(&service, "t2")?;
    assert!(
        !first.contains("same failure twice"),
        "one red says nothing about the premise: {first}"
    );
    let second = claim_again(&service, "t2")?;
    assert!(
        second.contains("same failure twice") && second.contains("(t1)"),
        "an identical failure must name the assumption tasks to re-verify: {second}"
    );
    Ok(())
}

#[test]
fn a_changing_failure_does_not_demand_a_premise_recheck() -> TestResult {
    let dir = scratch("drift");
    let counter = dir.join("n");
    let (service, _store, _delivered) = service_with_store();
    let counter = counter.display();
    service.create(&json!([
        {"title": "drifting", "acceptance": "suite green",
         "check": format!("n=$(cat {counter} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {counter}; echo attempt $n failed; exit 2")},
    ]))?;
    claim_again(&service, "t1")?;
    let second = claim_again(&service, "t1")?;
    assert!(
        second.contains("stayed red through 2 attempts"),
        "the ladder still counts a drifting failure: {second}"
    );
    assert!(
        !second.contains("same failure twice"),
        "a different failure is not a false premise: {second}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn the_ladder_is_stationary_under_spend() -> TestResult {
    let spent = Goal {
        objective: "ship it".to_owned(),
        status: GoalStatus::Active,
        token_budget: Some(10),
        tokens_used: 4_000_000,
        time_used_seconds: 90_000,
        created: 0,
        updated: 0,
        check: None,
        check_timeout_ms: None,
        check_failure: None,
        discoveries: Vec::new(),
        extra: serde_json::Map::new(),
    };
    let streak = |goal: Option<Goal>| -> Result<Vec<String>, Box<dyn Error>> {
        let (service, store, _delivered) = service_with_store();
        if let Some(goal) = goal {
            yi_session::lock_session(&store).set_goal(goal)?;
        }
        service.create(&json!([
            {"title": "impossible", "acceptance": "never true",
             "check": "echo case 7 diverges; exit 2"},
        ]))?;
        Ok(vec![
            claim_again(&service, "t1")?,
            claim_again(&service, "t1")?,
            claim_again(&service, "t1")?,
        ])
    };
    assert_eq!(
        streak(None)?,
        streak(Some(spent))?,
        "a consumed budget must not move a rung: the ladder reads the streak only"
    );
    Ok(())
}

#[test]
fn summary_line_counts_done_claims_that_ran_no_check() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let read = || -> Result<String, Box<dyn Error>> {
        let plan = yi_session::lock_session(&store).plan().ok_or("plan")?;
        Ok(yi_runtime::plan::summary_line(&plan))
    };
    assert!(
        !read()?.contains("unchecked"),
        "nothing is done yet: {}",
        read()?
    );

    service.update("t1", "done", None, None)?;
    assert!(
        !read()?.contains("unchecked"),
        "t1 carries a check, so its completion was verified: {}",
        read()?
    );

    service.edit(
        "add",
        &json!({"tasks": [{"title": "Docs", "acceptance": "README names the flag"}]}),
    )?;
    service.update("t2", "done", None, None)?;
    service.update("t3", "done", None, None)?;
    assert!(
        read()?.contains("2 done unchecked"),
        "the digest header must count checkless done claims: {}",
        read()?
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
        discoveries: Vec::new(),
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

#[test]
fn plan_transition_pokes_a_forced_review_with_the_summary_line() -> TestResult {
    use yi_runtime::advisor::{AdvisorConfig, AdvisorRuntime};

    let (service, _store, _delivered) = service_with_store();
    let advisor = Arc::new(AdvisorRuntime::new(
        AdvisorConfig::default(), // no cadence: only the poke can force a review
        Arc::new(|_message| {}),
        None,
    ));
    let observer = Arc::clone(&advisor);
    service.set_on_change(Arc::new(move |plan| {
        observer.request_review(Some(yi_runtime::plan::summary_line(plan)));
    }));

    service.create(&two_task_specs())?;
    // create writes directly; only update/edit notify. Baseline: no review due.
    assert!(
        advisor
            .observe(
                &yi_types::message::AgentMessage::User {
                    content: yi_types::message::UserContent::Text("hi".to_owned()),
                    timestamp: 0,
                },
                0,
            )
            .is_none(),
        "without a poke or cadence the advisor stays silent"
    );

    service.update("t1", "running", None, None)?;
    let chunk = advisor
        .observe(
            &yi_types::message::AgentMessage::User {
                content: yi_types::message::UserContent::Text("go on".to_owned()),
                timestamp: 0,
            },
            0,
        )
        .ok_or("a plan transition must force the next review")?;
    assert!(
        chunk.contains("context: plan v2: 0 ready, 1 running, 0 blocked, 0 done of 2"),
        "the digest carries the frontier summary: {chunk}"
    );
    assert!(
        advisor
            .observe(
                &yi_types::message::AgentMessage::User {
                    content: yi_types::message::UserContent::Text("more".to_owned()),
                    timestamp: 0,
                },
                0,
            )
            .is_none(),
        "the poke is consumed by one review"
    );
    Ok(())
}

#[test]
fn stale_turns_knob_overrides_the_default() -> TestResult {
    let (fast, _store2, delivered2) = {
        let store = memory_store();
        let handle = store.clone();
        let delivered: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&delivered);
        let deliver: DeliverFn = Arc::new(move |message, _mode: DeliveryMode| {
            if let Ok(mut queue) = sink.lock() {
                queue.push(message);
            }
        });
        (
            Arc::new(
                yi_runtime::plan::PlanService::new(Arc::new(move || Some(handle.clone())), deliver)
                    .with_stale_turns(Some(2)),
            ),
            store,
            delivered,
        )
    };
    fast.create(&two_task_specs())?;
    for _turn in 0..3 {
        fast.observe(&assistant_turn_end());
    }
    let count = delivered2
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
    assert_eq!(count, 1, "a 2-turn knob reminds by the third turn");
    Ok(())
}

fn split_payload(task_id: &str, subtasks: Value) -> Value {
    json!({"task_id": task_id, "subtasks": subtasks})
}

#[test]
fn split_refusals_arrive_in_one_pass() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let error = service
        .split(&split_payload(
            "t1",
            json!([
                {"title": "a", "acceptance": "ok", "writes": ["ast"]},
                {"title": "b", "acceptance": "ok", "writes": ["ast"]},
                {"title": "", "acceptance": "ok"},
                {"title": "d", "acceptance": "ok", "reads": ["rows"]},
                {"title": "e", "acceptance": "ok", "writes": ["rows"]},
            ]),
        ))
        .err()
        .ok_or("a guaranteed-wrong split must be refused")?;
    for expected in [
        "5 subtasks exceeds the maximum 4",
        "subtasks 0 and 1 both write \"ast\"",
        "subtask 2: title must be non-empty",
        "subtask 3: reads \"rows\" that only a sibling writes",
    ] {
        assert!(
            error.contains(expected),
            "one round trip names every failure; missing {expected} in: {error}"
        );
    }
    let stored = yi_session::lock_session(&_store).plan().ok_or("plan")?;
    assert_eq!(stored.tasks.len(), 2, "a refused split writes nothing");
    Ok(())
}

#[test]
fn split_refuses_a_second_generation() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    service.split(&split_payload(
        "t1",
        json!([{"title": "parse", "acceptance": "parses", "check": "true"}]),
    ))?;
    let error = service
        .split(&split_payload(
            "t1.1",
            json!([{"title": "deeper", "acceptance": "still parses"}]),
        ))
        .err()
        .ok_or("depth 2 must be refused")?;
    assert!(
        error.contains("split depth 2 exceeds the maximum 1"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_valid_split_lowers_onto_the_dag() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    let before = yi_session::lock_session(&store).plan().ok_or("plan")?;
    let acceptance = before
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?
        .acceptance
        .clone();

    let after = service.split(&split_payload(
        "t1",
        json!([
            {"title": "read the file", "acceptance": "bytes in hand", "check": "true", "writes": ["raw"]},
            {"title": "ask the user which dialect", "acceptance": "dialect named", "reads": ["cfg"], "writes": ["dialect"]},
        ]),
    ))?;
    assert_eq!(
        after["frontier"],
        json!(["t1.1", "t1.2"]),
        "children are the new frontier, not the parent"
    );

    let stored = yi_session::lock_session(&store).plan().ok_or("plan")?;
    assert_eq!(
        stored.version,
        before.version.bump(),
        "a split bumps the version"
    );
    let parent = stored
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?;
    assert_eq!(
        parent.deps,
        vec![
            yi_types::plan::TaskId("t1.1".to_owned()),
            yi_types::plan::TaskId("t1.2".to_owned())
        ],
        "the parent now waits on its children"
    );
    assert_eq!(
        parent.acceptance, acceptance,
        "a split never weakens the parent standard"
    );
    let checkless = stored
        .task(&yi_types::plan::TaskId("t1.2".to_owned()))
        .ok_or("t1.2")?;
    assert_eq!(
        checkless.check, None,
        "a checkless ask leaf is admitted, not refused"
    );
    assert_eq!(checkless.state, TaskState::Pending);
    assert!(
        checkless.deps.is_empty(),
        "the host writes the topology; siblings are unordered"
    );
    Ok(())
}

#[test]
fn a_split_resets_the_red_streak() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create(&json!([
        {"title": "impossible", "acceptance": "never true", "check": "echo boom; exit 2"},
    ]))?;
    service
        .update("t1", "done", None, None)
        .err()
        .ok_or("red 1")?;
    service.update("t1", "running", None, None)?;
    let escalated = service
        .update("t1", "done", None, None)
        .err()
        .ok_or("red 2")?;
    assert!(escalated.contains("2 attempts"), "{escalated}");

    service.update("t1", "running", None, None)?;
    service.split(&split_payload(
        "t1",
        json!([{"title": "narrow it", "acceptance": "one case green", "check": "true"}]),
    ))?;
    service.update("t1", "running", None, None)?;
    let fresh = service
        .update("t1", "done", None, None)
        .err()
        .ok_or("red 3")?;
    assert!(
        !fresh.contains("attempts") && !fresh.contains("same failure twice"),
        "restructuring resets the ladder: {fresh}"
    );
    Ok(())
}

#[test]
fn a_done_task_is_not_split_behind_its_own_back() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(&two_task_specs())?;
    service.update("t1", "done", None, None)?;
    let error = service
        .split(&split_payload(
            "t1",
            json!([{"title": "late", "acceptance": "still true"}]),
        ))
        .err()
        .ok_or("splitting a done task must be refused")?;
    assert!(error.contains("reopen it with plan.edit"), "{error}");
    let stored = yi_session::lock_session(&store).plan().ok_or("plan")?;
    let parent = stored
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?;
    assert!(
        parent.deps.is_empty(),
        "a done task never gains unfinished deps"
    );
    Ok(())
}

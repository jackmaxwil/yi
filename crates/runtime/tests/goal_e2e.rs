use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Number;
use yi_ai::faux::{faux_assistant_message, faux_text};

use yi_loop::ExecutionMode;
use yi_runtime::goal::{
    GOAL_EXISTS_ERROR, GoalService, StoreHandle, attach_goal, continuation_text, record_discovery,
    template,
};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, Cost, StopReason, Usage, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::schedule::DeliveryMode;
use yi_types::subagent::Discovery;

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "goal-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
        },
    )))
}

fn service_with_store() -> (
    Arc<GoalService>,
    yi_session::SharedSession,
    Arc<Mutex<Vec<AgentMessage>>>,
) {
    let store = memory_store();
    let handle = store.clone();
    let delivered: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let service = Arc::new(GoalService::new(
        Arc::new(move || Some(handle.clone())),
        Arc::new(move |message, _mode: DeliveryMode| {
            if let Ok(mut queue) = sink.lock() {
                queue.push(message);
            }
        }),
    ));
    (service, store, delivered)
}

#[test]
fn template_rejects_extra_and_missing_values() {
    let error = template::render(
        "Hello {{ name }}",
        vec![("name", "a".to_owned()), ("stray", "b".to_owned())],
    );
    assert_eq!(
        error,
        Err(template::TemplateError::ExtraValue {
            name: "stray".to_owned()
        }),
        "a renamed placeholder must not silently drop content"
    );
    let error = template::render("Hello {{ name }}", Vec::new());
    assert_eq!(
        error,
        Err(template::TemplateError::MissingValue {
            name: "name".to_owned()
        })
    );
    let escaped = template::render("literal {{{{ braces }}}}", Vec::new());
    assert_eq!(escaped, Ok("literal {{ braces }}".to_owned()));
}

#[test]
fn create_fails_while_unfinished_and_survives_the_store_fact() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create("ship phase 6", Some(1000), None, None)?;
    let error = service
        .create("another", None, None, None)
        .err()
        .ok_or("must fail")?;
    assert_eq!(error, GOAL_EXISTS_ERROR);
    service.update("complete")?;
    service.create("next objective", None, None, None)?;
    let stored = yi_session::lock_session(&store)
        .goal()
        .ok_or("goal fact must persist in the store")?;
    assert_eq!(stored.objective, "next objective");
    assert_eq!(stored.status, GoalStatus::Active);
    Ok(())
}

#[test]
fn budget_crossing_limits_the_goal_and_delivers_one_reminder() -> TestResult {
    let (service, store, delivered) = service_with_store();
    service.create("bounded work", Some(100), None, None)?;
    let zero = || Number::from(0u64);
    let usage = Usage {
        input: 80,
        output: 40,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: 120,
        cost: Cost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            total: zero(),
        },
    };
    let with_usage = AgentMessage::Assistant {
        content: vec![faux_text("work")],
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        raw_stop_reason: None,
        end_turn: None,
        deferred: None,
        error_message: None,
        timestamp: 0,
    };
    service.observe(&yi_types::event::AgentEvent::MessageEnd {
        message: with_usage.clone(),
    });
    let goal = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(goal.tokens_used, 120, "uncached input + output");
    assert_eq!(
        goal.status,
        GoalStatus::BudgetLimited,
        "crossing the budget must limit the goal"
    );
    service.observe(&yi_types::event::AgentEvent::MessageEnd {
        message: with_usage,
    });
    let reminders = delivered
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "goal_prompt")
        })
        .count();
    assert_eq!(
        reminders, 1,
        "the budget reminder is one-shot on the Active->BudgetLimited transition"
    );
    Ok(())
}

#[tokio::test]
async fn active_goal_continues_past_idle_until_a_failing_turn_blocks_it() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("first turn")],
        StopReason::Stop,
    )]);
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("second turn")],
        StopReason::Stop,
    )]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let store = memory_store();
    session.attach_store(store.clone())?;
    let service = attach_goal(&session);
    service.create("keep going until proven done", None, None, None)?;

    session.prompt("start")?;
    let mut blocked = false;
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let status = yi_session::lock_session(&store)
            .goal()
            .map(|goal| goal.status);
        if status == Some(GoalStatus::Blocked) {
            blocked = true;
            break;
        }
    }
    assert!(
        blocked,
        "an exhausted faux provider errors the turn and the error must block the goal"
    );
    let continuations = session
        .messages()
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, content: UserContent::Text(text), .. }
                if custom_type == "goal_prompt" && text.contains("Continue working toward the active thread goal"))
        })
        .count();
    assert!(
        continuations >= 2,
        "each idle with an Active goal must re-inject the continuation prompt: {continuations}"
    );
    Ok(())
}

#[test]
fn continuation_prompt_interpolates_budgets() -> TestResult {
    let goal = Goal {
        objective: "finish the port".to_owned(),
        status: GoalStatus::Active,
        token_budget: Some(1000),
        tokens_used: 250,
        time_used_seconds: 60,
        created: 0,
        updated: 0,
        check: None,
        check_timeout_ms: None,
        check_failure: None,
        discoveries: Vec::new(),
        extra: serde_json::Map::new(),
    };
    let text = continuation_text(&goal, None)?;
    assert!(text.contains("<untrusted_objective>\nfinish the port\n</untrusted_objective>"));
    assert!(text.contains("Tokens remaining: 750"));
    assert!(
        text.contains("goal.update"),
        "the prompt must name Yi's tool surface"
    );
    assert!(
        !text.contains("update_plan"),
        "the update_plan paragraph is adapted out (D26)"
    );
    Ok(())
}

#[test]
fn check_gate_rejects_completion_and_persists_the_evidence() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create(
        "provably done work",
        None,
        Some("echo unit A missing; exit 3".to_owned()),
        None,
    )?;
    let error = service.update("complete").err().ok_or("must reject")?;
    assert!(
        error.contains("completion rejected") && error.contains("exited 3"),
        "rejection must carry the exit evidence: {error}"
    );
    assert!(
        error.contains("unit A missing"),
        "tail must survive: {error}"
    );
    let stored = yi_session::lock_session(&store)
        .goal()
        .ok_or("goal fact must persist")?;
    assert_eq!(
        stored.status,
        GoalStatus::Active,
        "a rejected claim stays active"
    );
    let failure = stored
        .check_failure
        .ok_or("audit must persist on rejection")?;
    assert!(failure.contains("unit A missing"));
    let text = continuation_text(
        &yi_session::lock_session(&store).goal().ok_or("goal")?,
        None,
    )?;
    assert!(
        text.contains("rejected by the goal check") && text.contains("unit A missing"),
        "continuation must carry the failure: {text}"
    );
    Ok(())
}

#[test]
fn check_gate_passes_and_clears_the_failure() -> TestResult {
    let (service, store, _delivered) = service_with_store();
    service.create("done when true", None, Some("false".to_owned()), None)?;
    assert!(service.update("complete").is_err());
    let store_goal = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert!(store_goal.check_failure.is_some());
    // The check itself changed state (here: the command), so the same claim now verifies.
    let mut goal = store_goal;
    goal.check = Some("true".to_owned());
    yi_session::lock_session(&store).set_goal(goal)?;
    service.update("complete")?;
    let stored = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(stored.status, GoalStatus::Complete);
    assert!(
        stored.check_failure.is_none(),
        "a verified claim clears the audit"
    );
    Ok(())
}

#[test]
fn check_gate_timeout_rejects_with_the_timeout_named() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create("slow check", None, Some("sleep 5".to_owned()), Some(100))?;
    let error = service.update("complete").err().ok_or("must time out")?;
    assert!(
        error.contains("timed out after 100 ms"),
        "timeout must be named: {error}"
    );
    Ok(())
}

fn seed_plan(store: &yi_session::SharedSession, check: &str) -> TestResult {
    seed_plan_of(store, "t1", check)
}

fn seed_plan_of(store: &yi_session::SharedSession, id: &str, check: &str) -> TestResult {
    yi_session::lock_session(store).set_plan(yi_types::plan::Plan {
        version: yi_types::plan::PlanVersion(1),
        tasks: vec![yi_types::plan::Task {
            id: yi_types::plan::TaskId(id.to_owned()),
            title: "hold the invariant".to_owned(),
            acceptance: "the check is green".to_owned(),
            schema: None,
            check: Some(check.to_owned()),
            deps: Vec::new(),
            state: yi_types::plan::TaskState::Done,
            blocked_reason: None,
            assignee: None,
            red_count: None,
            red_fingerprint: None,
            extra: serde_json::Map::new(),
        }],
        created: 0,
        updated: 0,
        extra: serde_json::Map::new(),
    })?;
    Ok(())
}

/// Rows are written by a child, and the check they name is a shell command:
/// adjudicating per row lets a ledger multiply one command by its row count.
#[test]
fn rows_naming_one_task_adjudicate_on_a_single_check_run() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-drain-once-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let tally = dir.join("runs");
    let (service, store, _delivered) = service_with_store();
    service.create("ship the fix", None, None, None)?;
    seed_plan(&store, &format!("echo run >> {}", tally.display()))?;
    let handle = store_handle(&store);
    for fingerprint in ["aaa", "bbb", "ccc"] {
        let mut row = high_row();
        row.fingerprint = fingerprint.to_owned();
        record_discovery(&handle, &row)?;
    }

    service.update("complete")?;
    assert_eq!(
        std::fs::read_to_string(&tally)?.lines().count(),
        1,
        "three rows naming t1 must cost one check run, not one each"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn high_row() -> Discovery {
    let mut extra = serde_json::Map::new();
    extra.insert("source".to_owned(), serde_json::json!("child finder"));
    Discovery {
        text: "the retry loop double-counts".to_owned(),
        violates_check_of: Some(yi_types::plan::TaskId("t1".to_owned())),
        fingerprint: "aaa".to_owned(),
        extra,
    }
}

fn store_handle(store: &yi_session::SharedSession) -> StoreHandle {
    let store = store.clone();
    Arc::new(move || Some(store.clone()))
}

#[test]
fn an_undrained_discovery_refuses_completion_until_its_check_goes_green() -> TestResult {
    let (service, store, delivered) = service_with_store();
    service.create("ship the fix", None, None, None)?;
    seed_plan(&store, "echo t1 still broken; exit 4")?;
    let handle = store_handle(&store);
    record_discovery(&handle, &high_row())?;
    record_discovery(&handle, &high_row())?;

    let error = service.update("complete").err().ok_or("must refuse")?;
    assert!(
        error.contains("undrained HIGH discovery aaa")
            && error.contains("retry loop double-counts")
            && error.contains("task t1")
            && error.contains("t1 still broken"),
        "the refusal names the row and the evidence that keeps it open: {error}"
    );
    let stored = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(
        stored.status,
        GoalStatus::Active,
        "a refused claim stays open"
    );
    assert_eq!(
        stored.discoveries.len(),
        1,
        "the row survives the refusal (and a repeat record), so the gate keeps holding"
    );

    seed_plan(&store, "true")?;
    service.update("complete")?;
    let stored = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(stored.status, GoalStatus::Complete);
    assert!(
        stored.discoveries.is_empty(),
        "a drained row leaves the ledger"
    );
    let details = delivered
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details: Some(details),
                ..
            } if custom_type == "discovery" => Some(details.clone()),
            _ => None,
        })
        .ok_or("a drained row must land as a discovery entry the mining board reads")?;
    assert_eq!(details["drained"], serde_json::json!(true));
    assert_eq!(details["task"], serde_json::json!("t1"));
    assert_eq!(
        details["discovery"]["fingerprint"],
        serde_json::json!("aaa")
    );
    assert_eq!(
        details["discovery"]["source"],
        serde_json::json!("child finder"),
        "unknown discovery fields ride through the ledger verbatim"
    );
    Ok(())
}

#[test]
fn a_row_whose_check_left_the_plan_drains_instead_of_wedging_the_goal() -> TestResult {
    let (service, store, delivered) = service_with_store();
    service.create("ship the fix", None, None, None)?;
    seed_plan(&store, "exit 4")?;
    record_discovery(&store_handle(&store), &high_row())?;
    // The plan the row named is replaced by one that no longer carries t1.
    seed_plan_of(&store, "t9", "true")?;
    service.update("complete")?;
    let stored = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(stored.status, GoalStatus::Complete);
    assert!(stored.discoveries.is_empty());
    let reasons = delivered
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                details: Some(details),
                ..
            } => Some(details["reason"].clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        vec![serde_json::json!(
            "the check it named is no longer in the plan"
        )],
        "an unresolvable row is drained with its reason, never silently dropped: {reasons:?}"
    );
    Ok(())
}

#[test]
fn a_recorded_row_survives_a_plan_the_gate_cannot_read() -> TestResult {
    let (service, store, delivered) = service_with_store();
    service.create("ship the fix", None, None, None)?;
    record_discovery(&store_handle(&store), &high_row())?;

    let error = service.update("complete").err().ok_or("must refuse")?;
    assert!(
        error.contains("cannot adjudicate 1 recorded discovery row"),
        "an unreadable plan is named as an adjudication failure, not treated as a retired check: {error}"
    );
    let stored = yi_session::lock_session(&store).goal().ok_or("goal")?;
    assert_eq!(
        stored.status,
        GoalStatus::Active,
        "a refused claim stays open"
    );
    assert_eq!(
        stored.discoveries.len(),
        1,
        "a row confirmed red at record time is never drained by a reader that cannot see the plan"
    );
    let drained: Vec<AgentMessage> = delivered
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == "discovery")
        })
        .cloned()
        .collect();
    assert!(
        drained.is_empty(),
        "nothing is reported as drained when nothing could be adjudicated: {drained:?}"
    );
    Ok(())
}

#[test]
fn check_gate_ignores_blocked_and_checkless_goals() -> TestResult {
    let (service, _store, _delivered) = service_with_store();
    service.create("blocked path", None, Some("exit 1".to_owned()), None)?;
    // blocked is a report, not a completion claim: no check runs.
    service.update("blocked")?;
    let (service, _store, _delivered) = service_with_store();
    service.create("no check", None, None, None)?;
    service.update("complete")?;
    Ok(())
}

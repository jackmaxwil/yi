use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Number;
use yi_ai::faux::{faux_assistant_message, faux_text};

use yi_loop::ExecutionMode;
use yi_runtime::goal::{GOAL_EXISTS_ERROR, GoalService, attach_goal, continuation_text, template};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{AgentMessage, Cost, StopReason, Usage, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::schedule::DeliveryMode;

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
    service.create("ship phase 6", Some(1000))?;
    let error = service.create("another", None).err().ok_or("must fail")?;
    assert_eq!(error, GOAL_EXISTS_ERROR);
    service.update("complete")?;
    service.create("next objective", None)?;
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
    service.create("bounded work", Some(100))?;
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
    service.create("keep going until proven done", None)?;

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
        extra: serde_json::Map::new(),
    };
    let text = continuation_text(&goal)?;
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

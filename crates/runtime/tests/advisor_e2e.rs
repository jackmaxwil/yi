use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::advisor::guard::EmissionGuard;
use yi_runtime::advisor::{
    AdvisorConfig, AdvisorDeps, advisory_text, attach_advisor, digest, review::LlmReviewer,
};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::advisor::{Advice, AdviceKind, AdvisorySeverity};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
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

fn session(provider: Arc<ProviderStream>) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

fn tool_result(name: &str, is_error: bool) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "t1".to_owned(),
        tool_name: name.to_owned(),
        content: vec![Content::Text {
            text: if is_error { "boom" } else { "ok" }.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error,
        timestamp: 0,
    }
}

#[test]
fn guard_dedupes_blocks_noise_and_rate_limits() {
    let mut guard = EmissionGuard::default();
    guard.begin_cycle();
    assert!(
        !guard.accept("Stop."),
        "content-free filler must be dropped"
    );
    assert!(
        guard.accept("Missing await on writeStream.end() loses buffered writes"),
        "a real note passes and noise must not burn the slot"
    );
    assert!(
        !guard.accept("Another real note in the same cycle"),
        "one accepted note per cycle"
    );
    guard.begin_cycle();
    assert!(
        !guard.accept("missing await on WriteStream end() loses buffered writes!"),
        "punctuation/casing variants must dedupe"
    );
    guard.begin_cycle();
    assert!(
        guard.accept("A different note"),
        "fresh cycles accept fresh notes"
    );
}

#[test]
fn digest_keeps_constraints_first_and_extracts_directives() {
    let filler = "This sentence is ordinary filler about the weather. ".repeat(40);
    let text = format!("{filler}Never push to main without review. {filler}");
    let truncated = digest::truncate_user_text(&text, 200, "m7");
    assert!(
        truncated.contains("Never push to main"),
        "constraint sentences survive truncation first: {truncated}"
    );
    assert!(
        truncated.contains("pull m7"),
        "elided spans must carry the entry id as the pull handle"
    );
    let directives = digest::directives("Fix the bug. Only touch crates/loop. Thanks!", "m2");
    assert_eq!(directives, vec!["m2: Only touch crates/loop.".to_owned()]);
}

#[tokio::test]
async fn a_failure_streak_reaches_the_primary_through_nothing() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(Arc::clone(&provider));
    let advisor = attach_advisor(&session, AdvisorConfig::default(), AdvisorDeps::default());

    for _ in 0..3 {
        assert!(
            advisor.observe(&tool_result("bash", true), 0).is_none(),
            "no cadence is set, so nothing may trigger a review"
        );
    }
    let stats = advisor.stats();
    assert!(
        stats.contains("0 reviews"),
        "a run with no reviewer must review nothing: {stats}"
    );

    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("acknowledged")],
        StopReason::Stop,
    )]);
    session.prompt("continue")?;
    session.wait_idle().await;
    let spoken: Vec<String> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "advisory" => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        spoken.is_empty(),
        "with the deterministic reviewer gone nothing may reach the primary: {spoken:?}"
    );
    Ok(())
}

#[test]
fn headless_hold_degrades_to_warn() -> TestResult {
    let advice = Advice {
        severity: AdvisorySeverity::Hold,
        kind: AdviceKind::Stop,
        target: Some("migrations".to_owned()),
        text: "Schema migrations need review".to_owned(),
    };
    let rendered = advisory_text(&Advice {
        severity: AdvisorySeverity::Warn,
        ..advice.clone()
    });
    assert!(rendered.contains("severity=\"warn\"") && rendered.contains("target=\"migrations\""));

    let delivered: Arc<std::sync::Mutex<Vec<AgentMessage>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let runtime = yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig::default(),
        Arc::new(move |message| {
            if let Ok(mut queue) = sink.lock() {
                queue.push(message);
            }
        }),
        None,
    );
    runtime.deliver_reviewed(vec![advice], 0);
    let queue = delivered.lock().map_err(|error| error.to_string())?;
    let text = queue
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                content: UserContent::Text(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        text.contains("severity=\"warn\""),
        "with no hold sink an advisor Hold must degrade to Warn (D28): {text}"
    );
    Ok(())
}

#[tokio::test]
async fn llm_reviewer_advises_through_the_advise_tool() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut advise_args = Map::new();
    advise_args.insert(
        "note".to_owned(),
        Value::String("Cite the failing test before claiming green".to_owned()),
    );
    advise_args.insert("severity".to_owned(), Value::String("warn".to_owned()));
    advise_args.insert("kind".to_owned(), Value::String("risk".to_owned()));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("a1", "advise", advise_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("review done")], StopReason::Stop),
    ]);
    let runtime = Arc::new(yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig::default(),
        Arc::new(|_message| {}),
        None,
    ));
    let reviewer = LlmReviewer::new(Arc::clone(&provider), faux_model(), None);
    let advices = reviewer.review(&runtime, "m1 user: fix the tests").await;
    assert_eq!(advices.len(), 1, "the advise tool call must be captured");
    assert_eq!(advices[0].severity, AdvisorySeverity::Warn);
    assert_eq!(advices[0].kind, AdviceKind::Risk);
    assert!(advices[0].text.contains("failing test"));
    Ok(())
}

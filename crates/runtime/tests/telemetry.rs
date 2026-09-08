//! The spans a run is judged by, driven by a faux session: a request's timings and usage, a
//! turn, and a tool call's duration — each on disk beside the session file.

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Telemetry};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};
use yi_types::telemetry::{Span, SpanKind};

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
        context_window: 200_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn reply_with_usage(text: &str, input: i64, output: i64) -> AgentMessage {
    let mut message = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.input = input;
        usage.output = output;
        usage.total_tokens = input.saturating_add(output);
    }
    message
}

fn spans_in(path: &std::path::Path) -> Result<Vec<Span>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(serde_json::from_str::<Span>)
        .collect::<Result<_, _>>()?)
}

#[tokio::test]
async fn a_faux_turn_leaves_a_request_and_a_turn_span_beside_the_session() -> TestResult {
    let root = std::env::temp_dir().join(format!("yi-telemetry-turn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut repo = JsonlRepo::new(root.clone(), "/tmp/yi-telemetry".to_owned());
    let store = repo.create(CreateOptions {
        id: Some("t-1".to_owned()),
        ..CreateOptions::default()
    })?;
    let telemetry = Arc::new(Telemetry::default());
    let provider = ProviderStream::new(None, None).with_telemetry(Some(Arc::clone(&telemetry)));
    provider.queue_faux(vec![reply_with_usage("hello", 120, 30)]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::new(provider),
    );
    session.set_telemetry(Arc::clone(&telemetry));
    session.attach_store(Arc::clone(&store))?;
    session.prompt("hi")?;
    session.wait_idle().await;
    let path = telemetry.path().ok_or("no sidecar bound")?;
    assert!(
        path.to_string_lossy().ends_with(".telemetry.jsonl"),
        "{}",
        path.display()
    );
    let spans = spans_in(&path)?;
    let request = spans
        .iter()
        .find(|s| s.span == SpanKind::Request)
        .ok_or("no request span")?;
    assert_eq!(request.session, "t-1");
    assert_eq!(request.provider.as_deref(), Some("faux"));
    assert_eq!(request.input, Some(120));
    assert_eq!(request.output, Some(30));
    assert!(
        request.ttft_ms.is_some(),
        "the first delta stamps the clock"
    );
    assert!(request.total_ms.is_some());
    assert!(request.class.is_none(), "a clean stream carries no class");
    let turn = spans
        .iter()
        .find(|s| s.span == SpanKind::Turn)
        .ok_or("no turn span")?;
    assert_eq!(turn.turn, Some(1));
    assert_eq!(turn.input, Some(120));
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// A tool span is a projection of the end event: its duration is the one the loop stamped.
#[test]
fn a_tool_end_event_becomes_a_tool_span_with_the_loops_duration() -> TestResult {
    let root = std::env::temp_dir().join(format!("yi-telemetry-tool-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let telemetry = Telemetry::default();
    telemetry.bind(&root.join("1_t-2.jsonl"), "t-2");
    telemetry.on_event(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "call-1".to_owned(),
        tool_name: "read".to_owned(),
        result: ToolResult {
            content: Vec::new(),
            details: serde_json::json!({"durationMs": 41}),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    });
    telemetry.on_event(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "call-2".to_owned(),
        tool_name: "bash".to_owned(),
        result: ToolResult {
            content: Vec::new(),
            details: serde_json::json!({"durationMs": 7, "errorKind": "denied"}),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: true,
    });
    let spans = spans_in(&root.join("1_t-2.telemetry.jsonl"))?;
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].tool.as_deref(), Some("read"));
    assert_eq!(spans[0].ms, Some(41));
    assert_eq!(spans[0].ok, Some(true));
    assert!(spans[0].class.is_none());
    assert_eq!(spans[1].ok, Some(false));
    assert_eq!(
        spans[1].class.as_deref(),
        Some("tool:denied"),
        "the loop's kind is the class"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// The provider's error text names its transport class; the vocabulary is closed and printable.
#[test]
fn error_classes_render_a_closed_vocabulary() {
    use yi_types::telemetry::ErrorClass;
    assert_eq!(
        ErrorClass::from_provider_text("HTTP 503: overloaded").to_string(),
        "transport:http_503"
    );
    assert_eq!(
        ErrorClass::from_provider_text("request timed out after 60s").to_string(),
        "transport:timeout"
    );
    assert_eq!(
        ErrorClass::from_provider_text("invalid proxy x").to_string(),
        "transport:proxy"
    );
    assert_eq!(
        ErrorClass::from_provider_text("model went away").to_string(),
        "provider:error"
    );
    // Ledger row 0017's three dead streams, each its own class (issue #256).
    assert_eq!(
        ErrorClass::from_provider_text("Bad address (os error 14)").to_string(),
        "transport:os_14"
    );
    assert_eq!(
        ErrorClass::from_provider_text(
            "https://openrouter.ai/api/v1/chat/completions: Connection Failed: tls connection init failed: invalid peer certificate: UnknownIssuer"
        )
        .to_string(),
        "transport:tls"
    );
    assert_eq!(
        ErrorClass::from_provider_text("API Error: stream closed before completion").to_string(),
        "transport:closed"
    );
    assert_eq!(
        ErrorClass::from_provider_text("Error while decoding chunks").to_string(),
        "transport:closed"
    );
    assert!(ErrorClass::TransportClosed.is_transport());
    assert!(!ErrorClass::Provider("error".to_owned()).is_transport());
    assert_eq!(
        ErrorClass::Tool("denied".to_owned()).to_string(),
        "tool:denied"
    );
    assert_eq!(
        ErrorClass::RefusalUnknownModel.to_string(),
        "refusal:unknown_model"
    );
    assert_eq!(
        ErrorClass::Invariant("lanes".to_owned()).to_string(),
        "invariant:lanes"
    );
}

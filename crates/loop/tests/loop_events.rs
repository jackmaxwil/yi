use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::sync::mpsc::Receiver;
use yi_ai::faux::{
    FAUX_API, FAUX_MODEL_ID, FAUX_PROVIDER, faux_assistant_message, faux_text, faux_tool_call,
    stream_with_deltas, zero_usage,
};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{AgentTool, ToolFuture, ToolOutcome, error_tool_result};
use yi_loop::{ExecutionMode, LoopConfig, LoopContext, run_loop};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: FAUX_MODEL_ID.to_owned(),
        name: "Faux Model".to_owned(),
        api: FAUX_API.to_owned(),
        provider: FAUX_PROVIDER.to_owned(),
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

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: yi_types::message::UserContent::Text(text.to_owned()),
        timestamp: 0,
    }
}

struct Scripted {
    responses: Mutex<Vec<AgentMessage>>,
}

impl Scripted {
    fn new(responses: Vec<AgentMessage>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl yi_loop::run::StreamFn for Scripted {
    fn stream(
        &self,
        _model: &Model,
        _context: &LlmContext,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let events = match self.responses.lock() {
            Ok(mut queue) if !queue.is_empty() => stream_with_deltas(&queue.remove(0)),
            _ => {
                let mut error = faux_assistant_message(Vec::new(), StopReason::Error);
                if let AgentMessage::Assistant { error_message, .. } = &mut error {
                    *error_message = Some("No more faux responses queued".to_owned());
                }
                vec![AssistantMessageEvent::Error {
                    reason: StopReason::Error,
                    error,
                }]
            }
        };
        for event in events {
            let _ = sender.try_send(event);
        }
        receiver
    }
}

struct EchoTool;

impl AgentTool for EchoTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "echo".to_owned(),
            description: "echoes".to_owned(),
            parameters: json!({"type": "object"}),
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let mut result = error_tool_result(&format!("echo: {}", Value::Object(args)));
            result.usage = None;
            let _ = &mut result;
            ToolOutcome {
                result,
                is_error: false,
            }
        })
    }
}

fn kinds(events: &[AgentEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AgentEvent::AgentStart => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            AgentEvent::PermissionRequested { .. } => "permission_requested",
            AgentEvent::PermissionResolved { .. } => "permission_resolved",
        })
        .collect()
}

fn collector() -> (Arc<Mutex<Vec<AgentEvent>>>, impl FnMut(AgentEvent)) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (events, move |event| {
        if let Ok(mut sunk) = sink.lock() {
            sunk.push(event);
        }
    })
}

#[tokio::test]
async fn simple_text_turn_matches_pi_event_order() {
    let stream = Scripted::new(vec![faux_assistant_message(
        vec![faux_text("hello there friend")],
        StopReason::Stop,
    )]);
    let mut context = LoopContext {
        system_prompt: "sys".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        kinds(&events),
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_update",
            "message_update",
            "message_update",
            "message_update",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );
    assert_eq!(collected.len(), 2);
    assert_eq!(context.messages.len(), 2);
}

#[tokio::test]
async fn tool_turn_executes_and_continues() {
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("marco"));
    let stream = Scripted::new(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "echo", arguments)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("polo")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let kind_list = kinds(&events);
    assert!(kind_list.contains(&"tool_execution_start"));
    assert!(kind_list.contains(&"tool_execution_end"));
    assert_eq!(
        kind_list
            .iter()
            .filter(|kind| **kind == "turn_start")
            .count(),
        2
    );
    let tool_results: Vec<_> = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1);
    if let AgentMessage::ToolResult { is_error, .. } = tool_results[0] {
        assert!(!is_error);
    }
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Stop,
            ..
        })
    ));
}

#[tokio::test]
async fn length_stop_fails_every_tool_call() {
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("truncated"));
    let stream = Scripted::new(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "echo", arguments)],
            StopReason::Length,
        ),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let error_result = collected.iter().find_map(|message| match message {
        AgentMessage::ToolResult {
            is_error, content, ..
        } => Some((is_error, content)),
        _ => None,
    });
    let (is_error, content) = error_result.expect("tool result present");
    assert!(is_error);
    if let yi_types::message::Content::Text { text, .. } = &content[0] {
        assert!(text.contains("output token limit"));
    }
    assert!(kinds(&events).contains(&"tool_execution_end"));
}

#[tokio::test]
async fn error_stop_ends_turn_without_tools() {
    let mut error = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut error {
        *error_message = Some("boom".to_owned());
    }
    let stream = Scripted::new(vec![error]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let kind_list = kinds(&events);
    assert_eq!(kind_list.last(), Some(&"agent_end"));
    assert!(!kind_list.contains(&"tool_execution_start"));
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Error,
            ..
        })
    ));
}

#[tokio::test]
async fn follow_up_messages_restart_the_loop() {
    let stream = Scripted::new(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let mut config = LoopConfig::new(faux_model());
    let follow_ups = Arc::new(Mutex::new(vec![user("and then?")]));
    let queue = Arc::clone(&follow_ups);
    config.get_follow_up_messages = Some(Box::new(move || {
        queue
            .lock()
            .map(|mut queued| std::mem::take(&mut *queued))
            .unwrap_or_default()
    }));
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let assistant_count = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(assistant_count, 2);
    assert_eq!(
        kinds(&events)
            .iter()
            .filter(|kind| **kind == "turn_start")
            .count(),
        2
    );
    let _ = zero_usage();
    let _ = ExecutionMode::Parallel;
}

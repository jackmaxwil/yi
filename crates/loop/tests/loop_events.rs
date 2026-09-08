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
use yi_types::model::{LlmContext, Model, ModelCost, ToolChoice, ToolDef};

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
    AgentMessage::host_user(yi_types::message::UserContent::Text(text.to_owned()), 0)
}

struct Scripted {
    responses: Mutex<Vec<AgentMessage>>,
    choices: Mutex<Vec<Option<ToolChoice>>>,
}

impl Scripted {
    fn new(responses: Vec<AgentMessage>) -> Self {
        Self {
            responses: Mutex::new(responses),
            choices: Mutex::new(Vec::new()),
        }
    }

    fn choices(&self) -> Vec<Option<ToolChoice>> {
        self.choices
            .lock()
            .map(|choices| choices.clone())
            .unwrap_or_default()
    }
}

impl yi_loop::run::StreamFn for Scripted {
    fn stream(
        &self,
        _model: &Model,
        context: &LlmContext,
        _effort: yi_types::model::Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut choices) = self.choices.lock() {
            choices.push(context.tool_choice.clone());
        }
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
            freeform: None,
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

struct BashTool;

impl AgentTool for BashTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "bash".to_owned(),
            description: "runs a program".to_owned(),
            parameters: json!({"type": "object"}),
            freeform: None,
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            ToolOutcome {
                result: error_tool_result("bash: unused"),
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
            AgentEvent::ChildUpdate { .. } => "child_update",
            AgentEvent::LandingState { .. } => "landing_state",
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
async fn a_turn_repeated_verbatim_is_steered_once_then_ended() {
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("again"));
    let same = || {
        faux_assistant_message(
            vec![faux_tool_call("c", "echo", arguments.clone())],
            StopReason::ToolUse,
        )
    };
    let stream = Scripted::new((0..10).map(|_| same()).collect());
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let breaks = collected
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::REPEAT_BREAK_CUSTOM_TYPE)
        })
        .count();
    assert_eq!(breaks, 1, "one steer at the third identical batch");
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(
        answers,
        yi_loop::REPEAT_STOP_AT as usize,
        "the sixth identical batch ends the run"
    );

    let mut poll = Map::new();
    poll.insert("job".to_owned(), json!(1));
    let polls = Scripted::new(
        (0..8)
            .map(|_| {
                faux_assistant_message(
                    vec![faux_tool_call("p", "bash", poll.clone())],
                    StopReason::ToolUse,
                )
            })
            .chain(std::iter::once(faux_assistant_message(
                vec![faux_text("done")],
                StopReason::Stop,
            )))
            .collect(),
    );
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("wait")],
        &config,
        &signal,
        &mut emit,
        &polls,
    )
    .await;
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(answers, 9, "a job poll repeats as long as it likes");
}

fn three_bare_length_stops() -> Scripted {
    Scripted::new(vec![
        faux_assistant_message(vec![faux_text("thinking, thinking")], StopReason::Length),
        faux_assistant_message(vec![faux_text("still thinking")], StopReason::Length),
        faux_assistant_message(vec![faux_text("and still")], StopReason::Length),
        faux_assistant_message(vec![faux_text("never reached")], StopReason::Stop),
    ])
}

async fn drive_length_ladder(
    stream: &Scripted,
    tools: Vec<Arc<dyn AgentTool>>,
) -> (Vec<Value>, usize, Vec<AgentEvent>) {
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools,
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
        stream,
    )
    .await;
    let details: Vec<Value> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content,
                details,
                ..
            } if custom_type == yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE => {
                let text = match content {
                    yi_types::message::UserContent::Text(text) => text.clone(),
                    yi_types::message::UserContent::Blocks(_) => String::new(),
                };
                assert!(text.ends_with(yi_loop::LENGTH_FORCE_TEXT));
                details.clone()
            }
            _ => None,
        })
        .collect();
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    let events = events
        .lock()
        .map(|events| events.clone())
        .unwrap_or_default();
    (details, answers, events)
}

#[tokio::test]
async fn a_bare_length_stop_forces_bash_then_ends_on_the_third() {
    let stream = three_bare_length_stops();
    let (details, answers, events) =
        drive_length_ladder(&stream, vec![Arc::new(EchoTool), Arc::new(BashTool)]).await;
    assert_eq!(
        details,
        vec![
            json!({"rung": 1, "forced": true}),
            json!({"rung": 2, "forced": true})
        ]
    );
    assert_eq!(answers, 3, "the third bare length stop ends the run");
    let forced = ToolChoice::Tool(yi_types::model::ForcedTool::new("bash").expect("valid name"));
    assert_eq!(
        stream.choices(),
        vec![None, Some(forced.clone()), Some(forced)],
        "the two re-driven turns are forced to bash"
    );
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
}

#[tokio::test]
async fn without_a_bash_tool_the_message_rides_unforced() {
    let stream = three_bare_length_stops();
    let (details, answers, events) = drive_length_ladder(&stream, vec![Arc::new(EchoTool)]).await;
    assert_eq!(
        details,
        vec![
            json!({"rung": 1, "forced": false}),
            json!({"rung": 2, "forced": false})
        ]
    );
    assert_eq!(answers, 3);
    assert_eq!(stream.choices(), vec![None, None, None]);
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
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

fn dropped_stream() -> AgentMessage {
    let mut error = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut error {
        *error_message = Some("boom (upstream Wafer)".to_owned());
    }
    error
}

#[tokio::test]
async fn a_dropped_stream_that_showed_nothing_is_retried_once() {
    let stream = Scripted::new(vec![
        dropped_stream(),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let retries = collected
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom { custom_type, details, .. }
                if custom_type == yi_loop::STREAM_RETRY_CUSTOM_TYPE
                && details.as_ref().and_then(|d| d["error"].as_str()) == Some("boom (upstream Wafer)"))
        })
        .count();
    assert_eq!(
        retries, 1,
        "the dropped stream rides once as a hidden retry"
    );
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Stop,
            ..
        })
    ));

    let twice = Scripted::new(vec![dropped_stream(), dropped_stream()]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &twice,
    )
    .await;
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(answers, 2, "a second dropped stream ends the run");
}

#[tokio::test]
async fn error_stop_ends_turn_without_tools() {
    let mut error = faux_assistant_message(vec![faux_text("half an answer")], StopReason::Error);
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

#[tokio::test]
async fn a_misspelled_tool_name_is_repaired_once() {
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("marco"));
    let stream = Scripted::new(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "functions.Echo_tool", arguments)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: "sys".to_owned(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let _ = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let results: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::ToolResult { content, .. },
            } => Some(
                content
                    .iter()
                    .map(|block| match block {
                        yi_types::message::Content::Text { text, .. } => text.clone(),
                        _ => String::new(),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        results.iter().any(|text| text.contains("echo: ")),
        "{results:?}"
    );
    assert!(
        !results.iter().any(|text| text.contains("not found")),
        "{results:?}"
    );
}

#[tokio::test]
async fn an_unrepairable_tool_name_still_fails() {
    let stream = Scripted::new(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "teleport", Map::new())],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: "sys".to_owned(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let _ = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::MessageEnd {
                message: AgentMessage::ToolResult { content, .. }
            } if content.iter().any(|block| matches!(
                block,
                yi_types::message::Content::Text { text, .. } if text.contains("Tool teleport not found")
            ))
        )),
        "an unknown tool must still fail with its own name"
    );
}

/// A stream still in flight: it emits a delta, fires the interrupt the way a
/// user pressing Esc does, then goes quiet with the channel still open — which
/// is what an uncancelled provider stream looks like from the loop's side.
struct Trickle {
    signal: Arc<InterruptSignal>,
}

impl yi_loop::run::StreamFn for Trickle {
    fn stream(
        &self,
        _model: &Model,
        _context: &LlmContext,
        _effort: yi_types::model::Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let signal = Arc::clone(&self.signal);
        tokio::spawn(async move {
            let partial =
                |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
            let _ = sender
                .send(AssistantMessageEvent::Start {
                    partial: partial(""),
                })
                .await;
            let _ = sender
                .send(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: "one ".to_owned(),
                })
                .await;
            signal.fire();
            // The sender stays open and silent: nothing cancels a provider
            // stream, so after the interrupt the HTTP body is still hanging and
            // no further event is ever going to arrive. The turn has to end on
            // the signal alone or it does not end at all.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        receiver
    }
}

#[tokio::test]
async fn an_interrupt_mid_stream_ends_the_turn_instead_of_riding_it_out() {
    let signal = Arc::new(InterruptSignal::default());
    let stream = Trickle {
        signal: Arc::clone(&signal),
    };
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let config = LoopConfig::new(faux_model());
    let (events, mut emit) = collector();
    let collected = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_loop(
            &mut context,
            vec![user("stream something long")],
            &config,
            &signal,
            &mut emit,
            &stream,
        ),
    )
    .await
    .expect("a fired interrupt must end the turn; without a checkpoint in the stream consumer the loop waits on a stream that will never speak again");

    let reasons: Vec<StopReason> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Assistant { stop_reason, .. } => Some(*stop_reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons.contains(&StopReason::Aborted),
        "a fired interrupt must end the turn as aborted, not error or run to \
         completion: {reasons:?}"
    );
    // The turn is over: nothing kept streaming past the interrupt.
    let ends = events
        .lock()
        .map(|events| {
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::AgentEnd { .. }))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(ends, 1, "the turn ends exactly once");
}

/// A tool that sleeps, and reports Parallel or Sequential mode.
struct SleepTool {
    name: &'static str,
    mode: ExecutionMode,
    delay_ms: u64,
}

impl AgentTool for SleepTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name.to_owned(),
            description: "sleeps".to_owned(),
            parameters: json!({"type": "object"}),
            freeform: None,
        }
    }

    fn execution_mode(&self, _args: &Map<String, Value>) -> ExecutionMode {
        self.mode
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        let delay = self.delay_ms;
        Box::pin(async move {
            // The shipped path is `ToolAdapter`'s `spawn_blocking`, so the
            // sleep blocks a pool thread rather than yielding: an overlap here
            // is the overlap a real read-kind batch gets.
            let _ = tokio::task::spawn_blocking(move || {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            })
            .await;
            ToolOutcome {
                result: error_tool_result("slept"),
                is_error: false,
            }
        })
    }
}

/// P4: a batch of parallel-mode (read-kind) calls overlaps — the wall clock
/// comes in under half the summed per-call durations, which scales with load
/// where a fixed millisecond bound does not — and results keep call order.
#[tokio::test]
async fn parallel_read_batch_overlaps_and_stamps_duration() {
    let calls: Vec<yi_types::message::Content> = (0..4)
        .map(|i| faux_tool_call(&format!("call-{i}"), "sleeper", Map::new()))
        .collect();
    let stream = Scripted::new(vec![
        faux_assistant_message(calls, StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(SleepTool {
            name: "sleeper",
            mode: ExecutionMode::Parallel,
            delay_ms: 80,
        })],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let started = std::time::Instant::now();
    let _ = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let elapsed = started.elapsed().as_millis();
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let mut end_order = Vec::new();
    let mut summed = 0_u128;
    for event in events.iter() {
        if let AgentEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            ..
        } = event
        {
            end_order.push(tool_call_id.clone());
            let stamped = result
                .details
                .get("durationMs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            assert!(stamped > 0, "durationMs stamped: {result:?}");
            summed += u128::from(stamped);
        }
    }
    assert!(
        elapsed * 2 < summed,
        "batch was serial: {elapsed}ms wall against {summed}ms summed"
    );
    assert_eq!(end_order, ["call-0", "call-1", "call-2", "call-3"]);
}

/// A sequential-mode (mutating) call in the middle splits the batch: the
/// writes never overlap with anything.
#[tokio::test]
async fn sequential_tool_splits_the_batch() {
    let calls = vec![
        faux_tool_call("call-a", "sleeper", Map::new()),
        faux_tool_call("call-b", "writer", Map::new()),
        faux_tool_call("call-c", "sleeper", Map::new()),
    ];
    let stream = Scripted::new(vec![
        faux_assistant_message(calls, StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(SleepTool {
                name: "sleeper",
                mode: ExecutionMode::Parallel,
                delay_ms: 40,
            }),
            Arc::new(SleepTool {
                name: "writer",
                mode: ExecutionMode::Sequential,
                delay_ms: 40,
            }),
        ],
    };
    let config = LoopConfig::new(faux_model());
    let signal = InterruptSignal::default();
    let (events, mut emit) = collector();
    let started = std::time::Instant::now();
    let _ = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    // Three serial 40ms stretches (each single-item batch) stay ordered; a
    // lower bound only tightens under load, so it cannot flake high.
    assert!(started.elapsed() >= std::time::Duration::from_millis(110));
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    let ends: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ends, ["call-a", "call-b", "call-c"]);
}

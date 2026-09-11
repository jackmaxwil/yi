use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::sync::mpsc::Receiver;
use yi_ai::faux::{
    FAUX_API, FAUX_MODEL_ID, FAUX_PROVIDER, faux_assistant_message, faux_text, faux_thinking,
    faux_tool_call, stream_with_deltas, zero_usage,
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
                assert_eq!(text, yi_loop::LENGTH_REDRIVE_TEXT);
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
async fn a_bare_length_stop_is_re_driven_twice_then_ends_on_the_third() {
    let stream = three_bare_length_stops();
    let (details, answers, events) =
        drive_length_ladder(&stream, vec![Arc::new(EchoTool), Arc::new(BashTool)]).await;
    assert_eq!(
        details,
        vec![
            json!({"rung": 1, "cut": false}),
            json!({"rung": 2, "cut": false})
        ]
    );
    assert_eq!(answers, 3, "the third bare length stop ends the run");
    assert_eq!(
        stream.choices(),
        vec![None, None, None],
        "no turn is forced to a tool (D163 removed the forced bash)"
    );
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
}

#[tokio::test]
async fn a_tool_call_between_length_stops_starts_the_count_over() {
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("x"));
    let work = || {
        faux_assistant_message(
            vec![faux_tool_call("c", "echo", arguments.clone())],
            StopReason::ToolUse,
        )
    };
    let spiral = || faux_assistant_message(vec![faux_text("")], StopReason::Length);
    let stream = Scripted::new(vec![
        spiral(),
        work(),
        spiral(),
        work(),
        spiral(),
        work(),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
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
    let rungs: Vec<i64> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details,
                ..
            } if custom_type == yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE => {
                details.as_ref().and_then(|d| d["rung"].as_i64())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        rungs,
        vec![1, 1, 1],
        "each spiral after work is a first stop"
    );
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Stop,
            ..
        })
    ));
}

/// A stream that hands every delta over in order, however many: the scripted stream's
/// try_send into a channel of 64 would drop a 60k-char thinking block on the floor.
struct Spiral {
    responses: Mutex<Vec<AgentMessage>>,
}

impl yi_loop::run::StreamFn for Spiral {
    fn stream(
        &self,
        _model: &Model,
        _context: &LlmContext,
        _effort: yi_types::model::Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let events = match self.responses.lock() {
            Ok(mut queue) if !queue.is_empty() => stream_with_deltas(&queue.remove(0)),
            _ => Vec::new(),
        };
        // 32-char deltas: well over 256 events sit between the cut and the `Done`, as they
        // do behind a real channel of 256 (#325).
        let events: Vec<AssistantMessageEvent> = events
            .into_iter()
            .flat_map(|event| match event {
                AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta,
                } => delta
                    .as_bytes()
                    .chunks(32)
                    .map(|piece| AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: String::from_utf8_lossy(piece).into_owned(),
                    })
                    .collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect();
        tokio::spawn(async move {
            for event in events {
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        receiver
    }
}

/// D168: a cut is not a length strike. Four consecutive cuts re-drive four times, the second
/// and later ones naming the write, and the run reaches the answer; three bare length stops
/// still end it (the test above this one).
#[tokio::test]
async fn consecutive_cuts_re_drive_past_the_third_and_name_the_write() {
    let spiral = || {
        let mut message = faux_assistant_message(
            vec![faux_thinking(
                &"the router must rise at y=2, no, y=3, ".repeat(2_000),
            )],
            StopReason::Stop,
        );
        if let AgentMessage::Assistant { usage, .. } = &mut message {
            usage.unknown = true;
        }
        message
    };
    let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    let stream = Spiral {
        responses: Mutex::new(vec![spiral(), spiral(), spiral(), spiral(), answer]),
    };
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
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(answers, 5, "four cuts and the answer: {collected:?}");
    let redrives: Vec<(u64, String)> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details: Some(details),
                content: yi_types::message::UserContent::Text(text),
                ..
            } if custom_type == yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE => Some((
                details
                    .get("rung")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                text.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        redrives.iter().map(|(rung, _)| *rung).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(redrives[0].1, yi_loop::LENGTH_REDRIVE_TEXT);
    assert!(
        redrives[1..]
            .iter()
            .all(|(_, text)| text.contains("write the first version")),
        "{redrives:?}"
    );
}

/// D178: the audit slice's photonic trials were cut 22 times, never more than four in a row,
/// because a tool call between cuts started the count over, so twelve was never approached.
/// The sixth cut of a prompt ends the run, calls or not.
#[tokio::test]
async fn cut_turns_are_counted_per_prompt_not_per_tool_call() {
    let spiral = || {
        faux_assistant_message(
            vec![faux_thinking(
                &"the router must rise at y=2, no, y=3, ".repeat(2_000),
            )],
            StopReason::Stop,
        )
    };
    let mut arguments = Map::new();
    arguments.insert("word".to_owned(), json!("x"));
    let work = || {
        faux_assistant_message(
            vec![faux_tool_call("c", "echo", arguments.clone())],
            StopReason::ToolUse,
        )
    };
    let mut script: Vec<AgentMessage> = (0..6).flat_map(|_| [spiral(), work()]).collect();
    script.push(faux_assistant_message(
        vec![faux_text("never reached")],
        StopReason::Stop,
    ));
    let stream = Spiral {
        responses: Mutex::new(script),
    };
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
    let rungs: Vec<i64> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details,
                ..
            } if custom_type == yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE => {
                details.as_ref().and_then(|d| d["rung"].as_i64())
            }
            _ => None,
        })
        .collect();
    assert_eq!(rungs, vec![1, 2, 3, 4, 5], "a tool call keeps the count");
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(answers, 11, "the sixth cut ends the run: {collected:?}");
}

/// D163 amended: the provider settles a cut turn from the generation record and its `Done`
/// carries the measured usage; the cut message keeps it instead of the chars/4 estimate.
#[tokio::test]
async fn a_cut_turn_keeps_the_usage_the_provider_settled() {
    let mut spiral = faux_assistant_message(
        vec![faux_thinking(
            &"the router must rise at y=2, no, y=3, ".repeat(2_000),
        )],
        StopReason::Stop,
    );
    if let AgentMessage::Assistant { usage, .. } = &mut spiral {
        usage.input = 7_642;
        usage.output = 12_000;
        usage.reasoning = Some(12_000);
        usage.cost.total = serde_json::Number::from_f64(0.0035).unwrap();
        usage.unknown = false;
    }
    let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    let stream = Spiral {
        responses: Mutex::new(vec![spiral, answer]),
    };
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
    let cut = collected
        .iter()
        .find(|message| matches!(message, AgentMessage::Assistant { .. }))
        .expect("the cut turn is kept");
    if let AgentMessage::Assistant {
        content,
        stop_reason,
        usage,
        ..
    } = cut
    {
        assert_eq!(*stop_reason, StopReason::Length);
        assert!(content.is_empty(), "{content:?}");
        assert_eq!(
            (usage.input, usage.output, usage.reasoning),
            (7_642, 12_000, Some(12_000))
        );
        assert_eq!(usage.cost.total.as_f64(), Some(0.0035));
        assert!(!usage.unknown, "{usage:?}");
    }
}

/// Row 0023's photonic attempts reasoned to the 32k cap with no tool call; the loop now cuts
/// the request at the reasoning budget, keeps a bare length stop with no thinking block, and
/// re-drives.
#[tokio::test]
async fn a_reasoning_spiral_is_cut_at_the_char_budget_and_re_driven() {
    let mut spiral = faux_assistant_message(
        vec![faux_thinking(
            &"the router must rise at y=2, no, y=3, ".repeat(2_000),
        )],
        StopReason::Stop,
    );
    // a stream that closes with no usage chunk: the estimate stands, marked unknown
    if let AgentMessage::Assistant { usage, .. } = &mut spiral {
        usage.unknown = true;
    }
    let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    let stream = Spiral {
        responses: Mutex::new(vec![spiral, answer]),
    };
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
    let answers: Vec<&AgentMessage> = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .collect();
    assert_eq!(answers.len(), 2, "{collected:?}");
    if let AgentMessage::Assistant {
        content,
        stop_reason,
        usage,
        ..
    } = answers[0]
    {
        assert_eq!(*stop_reason, StopReason::Length);
        assert!(
            content.is_empty(),
            "the runaway thinking is not kept: {content:?}"
        );
        assert!(usage.reasoning.unwrap_or(0) >= 12_000, "{usage:?}");
        assert!(
            usage.unknown,
            "an estimate is never a measurement: {usage:?}"
        );
    }
    let redrive = collected
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                details,
                ..
            } if custom_type == yi_loop::LENGTH_REDRIVE_CUSTOM_TYPE => details.clone(),
            _ => None,
        })
        .expect("one re-drive");
    assert_eq!(redrive["rung"], 1);
    assert_eq!(redrive["cut"], true);
    assert!(redrive["reasoningChars"].as_u64().unwrap_or(0) >= yi_loop::REASONING_CHAR_CAP as u64);
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

/// D178: a call too long for the output ceiling is re-issued whole and truncated again, its
/// partial arguments different each time, so neither the repeat breaker nor a reset length
/// count ended it; each truncated call is a length strike and the third ends the run.
#[tokio::test]
async fn a_length_stop_carrying_a_truncated_call_is_a_strike() {
    let truncated = |n: usize| {
        let mut arguments = Map::new();
        arguments.insert("content".to_owned(), json!("x".repeat(n)));
        faux_assistant_message(
            vec![faux_tool_call("w", "echo", arguments)],
            StopReason::Length,
        )
    };
    let stream = Scripted::new(vec![
        truncated(1),
        truncated(2),
        truncated(3),
        faux_assistant_message(vec![faux_text("never reached")], StopReason::Stop),
    ]);
    let (_details, answers, events) = drive_length_ladder(&stream, vec![Arc::new(EchoTool)]).await;
    assert_eq!(answers, 3, "the third truncated call ends the run");
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
}

fn dropped_stream() -> AgentMessage {
    let mut error = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant {
        error_message,
        usage,
        ..
    } = &mut error
    {
        *error_message = Some("boom (upstream Wafer)".to_owned());
        usage.reasoning = Some(9163);
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

    let synthetic = Scripted::new(vec![faux_assistant_message(Vec::new(), StopReason::Error)]);
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
        &synthetic,
    )
    .await;
    assert!(
        !collected.iter().any(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::STREAM_RETRY_CUSTOM_TYPE)),
        "an error with zero usage never reached a provider and is not retried"
    );

    let mut wire = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut wire {
        *error_message = Some("Error while decoding chunks".to_owned());
    }
    let cut = Scripted::new(vec![
        wire,
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);
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
        &cut,
    )
    .await;
    assert!(
        collected.iter().any(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::STREAM_RETRY_CUSTOM_TYPE)),
        "a wire failure with zero usage is a dropped stream and is retried"
    );
}

/// OpenRouter's in-band error chunk as the mapper leaves it: thinking shown, no usage, the
/// raw stop marked; the text is the 2026-09-09 night's Wafer 502.
fn in_band_error() -> AgentMessage {
    let mut error = faux_assistant_message(vec![faux_thinking("hmm")], StopReason::Error);
    if let AgentMessage::Assistant {
        error_message,
        raw_stop_reason,
        usage,
        ..
    } = &mut error
    {
        *error_message =
            Some("Internal Server Error (upstream Wafer, code 502, server_error)".to_owned());
        *raw_stop_reason = Some(yi_types::message::RAW_STOP_IN_BAND_ERROR.to_owned());
        *usage = yi_types::message::Usage::unknown();
    }
    error
}

/// Runs the script against the echo tool: the hidden stream retries that rode it, and the
/// stop reason the run ended on.
async fn run_script(script: Vec<AgentMessage>) -> (usize, Option<StopReason>) {
    let stream = Scripted::new(script);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let config = LoopConfig::new(faux_model());
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("hi")],
        &config,
        &InterruptSignal::default(),
        &mut emit,
        &stream,
    )
    .await;
    let retries = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::STREAM_RETRY_CUSTOM_TYPE))
        .count();
    let last = match collected.last() {
        Some(AgentMessage::Assistant { stop_reason, .. }) => Some(*stop_reason),
        _ => None,
    };
    (retries, last)
}

#[tokio::test]
async fn an_in_band_provider_error_is_retried_once() {
    let recovered = || faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop);
    assert_eq!(
        run_script(vec![in_band_error(), recovered()]).await,
        (1, Some(StopReason::Stop)),
        "an in-band error that showed nothing runs again"
    );
    assert_eq!(
        run_script(vec![in_band_error(), in_band_error()]).await,
        (1, Some(StopReason::Error)),
        "a second in a row ends the run"
    );
    let tool_turn = faux_assistant_message(
        vec![faux_tool_call("call-1", "echo", Map::new())],
        StopReason::ToolUse,
    );
    assert_eq!(
        run_script(vec![
            in_band_error(),
            tool_turn,
            in_band_error(),
            recovered()
        ])
        .await,
        (2, Some(StopReason::Stop)),
        "a clean turn between two errors gives the second its own retry"
    );
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

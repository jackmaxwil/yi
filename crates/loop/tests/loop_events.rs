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
    assert_eq!(breaks, 1, "one steer, at the fourth identical batch");
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

async fn run_with<S: yi_loop::run::StreamFn>(stream: &S, config: &LoopConfig) -> Vec<AgentMessage> {
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let (_events, mut emit) = collector();
    let signal = InterruptSignal::default();
    run_loop(
        &mut context,
        vec![user("go")],
        config,
        &signal,
        &mut emit,
        stream,
    )
    .await
}

/// D179: `bash ls` and `todo view` taking turns never repeated the previous turn, and a
/// text-only turn had no signature, so neither loop was ever broken (audit B5, G6). The breaker
/// now counts a batch over the last six turns and fingerprints the text of a tool-less turn.
#[tokio::test]
async fn an_alternating_pair_and_a_text_only_spiral_are_broken() {
    let breaks = |collected: &[AgentMessage]| {
        collected
            .iter()
            .filter(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::REPEAT_BREAK_CUSTOM_TYPE))
            .count()
    };
    let answers = |collected: &[AgentMessage]| {
        collected
            .iter()
            .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
            .count()
    };
    let call = |word: &str| {
        let mut arguments = Map::new();
        arguments.insert("word".to_owned(), json!(word));
        faux_assistant_message(
            vec![faux_tool_call("c", "echo", arguments)],
            StopReason::ToolUse,
        )
    };
    let reply = |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);

    let mut script: Vec<AgentMessage> = (0..4).flat_map(|_| [call("ls"), call("view")]).collect();
    script.push(reply("done"));
    let collected = run_with(&Scripted::new(script), &LoopConfig::new(faux_model())).await;
    assert_eq!(
        breaks(&collected),
        1,
        "one steer, at the third ls in five turns"
    );
    assert_eq!(answers(&collected), 9);

    let spiral = (0..10)
        .map(|n| {
            reply(if n % 2 == 0 {
                "All  green."
            } else {
                "All green.\n"
            })
        })
        .collect();
    let mut config = LoopConfig::new(faux_model());
    config.intercept_stop = Some(Box::new(|_| Some(user("keep going"))));
    let collected = run_with(&Scripted::new(spiral), &config).await;
    assert_eq!(breaks(&collected), 1, "{collected:?}");
    assert_eq!(
        answers(&collected),
        yi_loop::REPEAT_STOP_AT as usize,
        "the sixth identical reply ends the run"
    );

    let cut = || {
        faux_assistant_message(
            vec![faux_thinking(
                &"the router must rise at y=2, no, y=3, ".repeat(2_000),
            )],
            StopReason::Stop,
        )
    };
    let stream = Spiral::new(vec![cut(), cut(), cut(), cut(), reply("done")]);
    let collected = run_with(&stream, &LoopConfig::new(faux_model())).await;
    assert_eq!(breaks(&collected), 0, "a cut turn has no text to repeat");
    assert_eq!(answers(&collected), 5);
}

/// D179: a prompt queued behind a running one starts its own repeat window, as it starts its
/// own cut count (D178): the same short answer to three prompts is not a spiral.
#[tokio::test]
async fn a_follow_up_prompt_starts_its_own_repeat_window() {
    let done = || faux_assistant_message(vec![faux_text("Done.")], StopReason::Stop);
    let mut config = LoopConfig::new(faux_model());
    let follow_ups = Arc::new(Mutex::new(vec![user("and this?"), user("and this?")]));
    config.get_follow_up_messages = Some(Box::new(move || {
        follow_ups
            .lock()
            .map(|mut queued| queued.pop().into_iter().collect())
            .unwrap_or_default()
    }));
    let collected = run_with(&Scripted::new(vec![done(), done(), done()]), &config).await;
    let steers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::REPEAT_BREAK_CUSTOM_TYPE))
        .count();
    assert_eq!(steers, 0, "{collected:?}");
}

/// D179 review: a fresh edit before each run of the same test is progress, yet each new edit
/// re-armed the steer, so every check from the third on was told to stop calling tools.
#[tokio::test]
async fn an_edit_then_check_loop_is_not_steered() {
    let call = |name: &str, key: &str, value: String| {
        let mut arguments = Map::new();
        arguments.insert(key.to_owned(), json!(value));
        faux_assistant_message(
            vec![faux_tool_call("c", name, arguments)],
            StopReason::ToolUse,
        )
    };
    let mut script: Vec<AgentMessage> = (0..5)
        .flat_map(|n| {
            [
                call("edit", "patch", format!("fix {n}")),
                call("bash", "command", "cargo test".to_owned()),
            ]
        })
        .collect();
    script.push(faux_assistant_message(
        vec![faux_text("fixed")],
        StopReason::Stop,
    ));
    let collected = run_with(&Scripted::new(script), &LoopConfig::new(faux_model())).await;
    let steers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::REPEAT_BREAK_CUSTOM_TYPE))
        .count();
    assert_eq!(steers, 0, "five edits, each checked once");
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Stop,
            ..
        })
    ));
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
            json!({"rung": 1, "cut": false, "signal": "length_redrive"}),
            json!({"rung": 2, "cut": false, "signal": "length_redrive"})
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
    requests: Mutex<Vec<(yi_types::model::Effort, LlmContext)>>,
}

impl Spiral {
    fn new(responses: Vec<AgentMessage>) -> Self {
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Each request's effort and context, in the order the loop sent them.
    fn requests(&self) -> Vec<(yi_types::model::Effort, LlmContext)> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    fn efforts(&self) -> Vec<yi_types::model::Effort> {
        self.requests()
            .into_iter()
            .map(|(effort, _)| effort)
            .collect()
    }
}

impl yi_loop::run::StreamFn for Spiral {
    fn stream(
        &self,
        _model: &Model,
        context: &LlmContext,
        effort: yi_types::model::Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push((effort, context.clone()));
        }
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
    let stream = Spiral::new(vec![spiral(), spiral(), spiral(), spiral(), answer]);
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

/// `z-ai/glm-5.3-flash` from the bundled catalog: the model and route of the 2026-09-11 sweep.
fn glm_5_3_flash() -> Option<Model> {
    yi_ai::catalog::Catalog::bundled()
        .get("openrouter", "z-ai/glm-5.3-flash")
        .cloned()
}

/// A recorded cut turn of `chars` reasoning chars (the transcript's `reasoningChars`; the text
/// itself was dropped at the cut, so filler stands in) whose last line names `label`.
fn recorded_cut(chars: usize, label: usize) -> AgentMessage {
    let last = format!("\nwhere turn {label} stopped: the upper bound still needs its induction");
    let filler = "the bound holds at n=4, check n=5 again, ".repeat(chars / 40 + 1);
    let thinking: String = filler
        .chars()
        .take(chars.saturating_sub(last.chars().count()))
        .chain(last.chars())
        .collect();
    let mut message = faux_assistant_message(vec![faux_thinking(&thinking)], StopReason::Stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message {
        usage.unknown = true;
    }
    message
}

/// The body the runtime's OpenRouter provider builds for a request, with its options.
fn openrouter_body(model: &Model, effort: yi_types::model::Effort, context: &LlmContext) -> Value {
    yi_ai::openai::build_params(
        model,
        context,
        &yi_ai::openai::OpenAiOptions {
            reasoning_effort: (effort != yi_types::model::Effort::Off).then_some(effort),
            ..yi_ai::openai::OpenAiOptions::default()
        },
    )
}

fn last_message(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or(Value::Null)
}

async fn run_on_glm(
    model: &Model,
    script: Vec<AgentMessage>,
    follow_up: Option<&str>,
) -> (Spiral, Vec<Value>) {
    let stream = Spiral::new(script);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let mut config = LoopConfig::new(model.clone());
    config.convert_to_llm = Box::new(yi_context::convert_to_llm);
    if let Some(text) = follow_up {
        let queued = Arc::new(Mutex::new(vec![user(text)]));
        config.get_follow_up_messages = Some(Box::new(move || {
            queued
                .lock()
                .map(|mut queued| std::mem::take(&mut *queued))
                .unwrap_or_default()
        }));
    }
    let signal = InterruptSignal::default();
    let (_events, mut emit) = collector();
    run_loop(
        &mut context,
        vec![user("Prove the lemma in Bound.v.")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let bodies = stream
        .requests()
        .iter()
        .map(|(effort, context)| openrouter_body(model, *effort, context))
        .collect();
    (stream, bodies)
}

/// coq-block-bound on glm-5.3-flash (the 2026-09-11 sweep) was cut six times in one prompt, at
/// 48,002, 48,002, 48,001, 48,019 and 48,006 chars, and called no tool: each request grew by
/// the re-drive's words alone (11,148 to 11,380 input tokens), so each turn derived from zero.
#[tokio::test]
async fn from_the_second_cut_glm_5_3_flash_is_shown_where_its_reasoning_stopped() {
    let recorded = [48_002, 48_002, 48_001, 48_019, 48_006, 48_006];
    let script = recorded
        .iter()
        .enumerate()
        .map(|(index, chars)| recorded_cut(*chars, index + 1))
        .collect();
    let model = glm_5_3_flash().expect("the bundled catalog carries glm-5.3-flash");
    let (_stream, bodies) = run_on_glm(&model, script, None).await;
    assert_eq!(
        bodies.len(),
        6,
        "the sixth cut ends the run, as it ended coq-block-bound"
    );
    for body in &bodies {
        assert_eq!(
            body["reasoning"],
            json!({"effort": "low"}),
            "the route refuses none"
        );
        assert!(body.get("tool_choice").is_none(), "{body}");
    }
    assert_eq!(
        last_message(&bodies[1]),
        json!({"role": "user", "content": yi_loop::LENGTH_REDRIVE_TEXT}),
        "a first cut is re-driven as before"
    );
    for (cut, body) in bodies.iter().enumerate().skip(2) {
        let text = last_message(body)["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            text.contains(&format!("where turn {cut} stopped")),
            "the request after cut {cut} ends: {text}"
        );
        assert!(text.starts_with(yi_loop::CUT_REDRIVE_TEXT), "{text}");
        assert!(text.chars().count() < 10_000, "a quote, not the cut");
    }
}

/// A prompt queued behind a spiral runs in the same loop (ACP `session/prompt` while busy):
/// it goes out as the first prompt went out, and its first cut is re-driven as a first cut.
#[tokio::test]
async fn a_follow_up_after_three_cuts_sends_what_the_first_prompt_sent() {
    let answer = |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    let script = vec![
        recorded_cut(48_002, 1),
        recorded_cut(48_002, 2),
        recorded_cut(48_001, 3),
        answer("first"),
        recorded_cut(48_019, 4),
        answer("second"),
    ];
    let model = glm_5_3_flash().expect("the bundled catalog carries glm-5.3-flash");
    let (stream, bodies) = run_on_glm(&model, script, Some("and then?")).await;
    let efforts = stream.efforts();
    assert_eq!(bodies.len(), 6, "{efforts:?}");
    let configured = model.clamp_effort(yi_types::model::Effort::default());
    assert!(
        efforts.iter().all(|effort| *effort == configured),
        "every request at the configured effort: {efforts:?}"
    );
    assert_eq!(bodies[4]["reasoning"], bodies[0]["reasoning"]);
    assert_eq!(
        last_message(&bodies[4]),
        json!({"role": "user", "content": "and then?"})
    );
    assert_eq!(
        last_message(&bodies[5]),
        json!({"role": "user", "content": yi_loop::LENGTH_REDRIVE_TEXT}),
        "the follow-up's first cut quotes nothing"
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
    let stream = Spiral::new(script);
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

/// D178: a prompt queued behind a running one (ACP `session/prompt` while busy) arrives as a
/// follow-up in the same loop; it starts its own cut count, not the fifth cut of the last one.
#[tokio::test]
async fn a_follow_up_prompt_starts_its_own_cut_count() {
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
    let answer = |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    let mut script: Vec<AgentMessage> = (0..5).flat_map(|_| [spiral(), work()]).collect();
    script.extend([answer("first"), spiral(), answer("second")]);
    let stream = Spiral::new(script);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(EchoTool)],
    };
    let mut config = LoopConfig::new(faux_model());
    let follow_ups = Arc::new(Mutex::new(vec![user("and then?")]));
    config.get_follow_up_messages = Some(Box::new(move || {
        follow_ups
            .lock()
            .map(|mut queued| std::mem::take(&mut *queued))
            .unwrap_or_default()
    }));
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
    assert_eq!(rungs, vec![1, 2, 3, 4, 5, 1], "{collected:?}");
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    assert_eq!(answers, 13, "the follow-up reaches its answer");
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
    let stream = Spiral::new(vec![spiral, answer]);
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
    let stream = Spiral::new(vec![spiral, answer]);
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

/// Markup the mapper could not read as a call is not an answer either: the turn ends as an
/// error the surface can show, not as prose that says `<tool_call>`.
#[tokio::test]
async fn unparsed_call_markup_ends_the_turn_as_an_error() -> Result<(), Box<dyn std::error::Error>>
{
    let stream = Scripted::new(vec![faux_assistant_message(
        vec![faux_text(
            "<tool_call>read<arg_key>path</arg_key><arg_value>a.rs",
        )],
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
        vec![user("read it")],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let last = collected.last();
    let Some(AgentMessage::Assistant {
        stop_reason,
        error_message,
        ..
    }) = last
    else {
        return Err(format!("the turn ends on the assistant message: {last:?}").into());
    };
    assert_eq!(*stop_reason, StopReason::Error);
    assert_eq!(
        error_message.as_deref(),
        Some(yi_loop::run::UNPARSED_MARKUP)
    );
    let events = events.lock().unwrap_or_else(|error| error.into_inner());
    assert!(
        !kinds(&events).contains(&"tool_execution_start"),
        "nothing ran: {:?}",
        kinds(&events)
    );
    Ok(())
}

/// A kernel cell the host saw block in a family wait (`rlm.wait`, `rlm.request`), or not.
struct Cell {
    waits: Arc<std::sync::atomic::AtomicU64>,
    blocks: bool,
}

impl AgentTool for Cell {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "ipython".to_owned(),
            description: "runs a cell".to_owned(),
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
            if self.blocks {
                self.waits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            ToolOutcome {
                result: error_tool_result("cell ran"),
                is_error: false,
            }
        })
    }
}

/// The repeat breaks and assistant turns of eight identical calls to `tool` then an answer,
/// under a host whose wait count moves only when a cell blocks.
async fn repeated(tool: &str, code: &str, blocks: bool) -> (usize, usize) {
    let mut arguments = Map::new();
    arguments.insert("code".to_owned(), json!(code));
    let mut script: Vec<_> = (0..8)
        .map(|_| {
            faux_assistant_message(
                vec![faux_tool_call("c", tool, arguments.clone())],
                StopReason::ToolUse,
            )
        })
        .collect();
    script.push(faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    ));
    let waits = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(EchoTool),
            Arc::new(Cell {
                waits: Arc::clone(&waits),
                blocks,
            }),
        ],
    };
    let mut config = LoopConfig::new(faux_model());
    config.waiting = Some(Arc::new(move || {
        waits.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let (_events, mut emit) = collector();
    let collected = run_loop(
        &mut context,
        vec![user("go")],
        &config,
        &InterruptSignal::default(),
        &mut emit,
        &Scripted::new(script),
    )
    .await;
    let breaks = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == yi_loop::REPEAT_BREAK_CUSTOM_TYPE))
        .count();
    let answers = collected
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    (breaks, answers)
}

/// Incident: a parent waiting on its children sent the same wait cell each turn, the breaker
/// told it nothing was changing, and its session ended with the children in it. Dies with the
/// breaker reading the cell's text: `print(await rlm.status())` beside the wait read as work.
#[tokio::test]
async fn a_cell_the_host_saw_wait_is_never_a_repeat() {
    let code = "print(await rlm.status())\nr = await rlm.wait(30)\nprint(r['changed'])";
    assert_eq!(
        repeated("ipython", code, true).await,
        (0, 9),
        "every wait ran and the run ended on its own answer"
    );
}

/// Dies with the exemption read off the cell's text: a wait-shaped cell the host never saw
/// block is a repeat, and a repeated call beside live work is steered as before.
#[tokio::test]
async fn a_batch_the_host_saw_no_wait_in_is_a_repeat_whatever_it_says() {
    let (breaks, _) = repeated("ipython", "r = await rlm.wait(300)\nprint(r)", false).await;
    assert_eq!(breaks, 1, "the text of a cell is not a wait");
    let (breaks, _) = repeated("echo", "make check", false).await;
    assert_eq!(
        breaks, 1,
        "a repeat that is not a wait is steered as before"
    );
}

/// A mutating tool whose run is interrupted: it fires the signal, as Esc does mid-call.
struct Stopper;

impl AgentTool for Stopper {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "stopper".to_owned(),
            description: "is interrupted while it runs".to_owned(),
            parameters: json!({"type": "object"}),
            freeform: None,
        }
    }

    fn execution_mode(&self, _args: &Map<String, Value>) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: Map<String, Value>,
        signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            signal.fire();
            ToolOutcome {
                result: error_tool_result("stopper: interrupted"),
                is_error: true,
            }
        })
    }
}

/// Incident: an interrupt during the first of three sequential calls left two calls with no
/// result on disk, and the loop then sent one more billed request before settling as aborted.
#[tokio::test]
async fn an_interrupt_answers_every_call_and_sends_no_request()
-> Result<(), Box<dyn std::error::Error>> {
    let calls = (0..3)
        .map(|index| faux_tool_call(&format!("call-{index}"), "stopper", Map::new()))
        .collect();
    let stream = Scripted::new(vec![
        faux_assistant_message(calls, StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("never requested")], StopReason::Stop),
    ]);
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(Stopper)],
    };
    let compactions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&compactions);
    let mut config = LoopConfig::new(faux_model());
    config.maybe_compact = Some(Box::new(move |_: &[AgentMessage]| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { None })
    }));
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
    let answered: Vec<&str> = collected
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        answered,
        ["call-0", "call-1", "call-2"],
        "every call has a result"
    );
    let ends = events
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. }))
        .count();
    assert_eq!(ends, 3, "the host sees every call end");
    assert_eq!(stream.choices().len(), 1, "no request after the interrupt");
    assert_eq!(
        compactions.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no summarizer request after the interrupt"
    );
    assert!(matches!(
        collected.last(),
        Some(AgentMessage::Assistant {
            stop_reason: StopReason::Aborted,
            ..
        })
    ));
    Ok(())
}

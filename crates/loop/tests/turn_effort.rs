use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::Receiver;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{AgentTool, ToolFuture, ToolOutcome, error_tool_result};
use yi_loop::{LoopConfig, LoopContext, NextTurn, run_loop};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Effort, ForcedTool, LlmContext, Model, ModelCost, ToolChoice, ToolDef};

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: String::new(),
        reasoning: true,
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

type Spied = Vec<(Effort, Option<ToolChoice>)>;

/// Records the effort and forced choice each turn's request went out with.
struct Spy {
    seen: Arc<Mutex<Spied>>,
    responses: Mutex<Vec<AgentMessage>>,
}

impl yi_loop::run::StreamFn for Spy {
    fn stream(
        &self,
        _model: &Model,
        context: &LlmContext,
        effort: Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((effort, context.tool_choice.clone()));
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let message = match self.responses.lock() {
            Ok(mut queue) if !queue.is_empty() => queue.remove(0),
            _ => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        };
        let _ = sender.try_send(AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message,
        });
        receiver
    }
}

struct Noop;

impl AgentTool for Noop {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "noop".to_owned(),
            description: "does nothing".to_owned(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            freeform: None,
        }
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: serde_json::Map<String, serde_json::Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        Box::pin(async {
            let mut result = error_tool_result("ok");
            result.usage = None;
            ToolOutcome {
                result,
                is_error: false,
            }
        })
    }
}

async fn run(config: LoopConfig, responses: Vec<AgentMessage>) -> Vec<Effort> {
    run_spied(config, responses)
        .await
        .into_iter()
        .map(|(effort, _)| effort)
        .collect()
}

async fn run_spied(config: LoopConfig, responses: Vec<AgentMessage>) -> Spied {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stream = Spy {
        seen: Arc::clone(&seen),
        responses: Mutex::new(responses),
    };
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(Noop)],
    };
    let signal = InterruptSignal::default();
    let mut emit = |_: AgentEvent| {};
    run_loop(
        &mut context,
        vec![AgentMessage::host_user(
            yi_types::message::UserContent::Text("hi".to_owned()),
            0,
        )],
        &config,
        &signal,
        &mut emit,
        &stream,
    )
    .await;
    let seen = seen.lock().unwrap_or_else(|error| error.into_inner());
    seen.clone()
}

#[tokio::test]
async fn the_configured_effort_rides_every_request() {
    let mut config = LoopConfig::new(faux_model());
    config.effort = Effort::Low;
    assert_eq!(run(config, Vec::new()).await, vec![Effort::Low]);
}

#[tokio::test]
async fn a_new_effort_lands_on_the_next_turn_not_the_one_in_flight() {
    let mut config = LoopConfig::new(faux_model());
    config.effort = Effort::Low;
    config.prepare_next_turn = Some(Box::new(|_| {
        Some(NextTurn {
            model: None,
            thinking: Some(Effort::High),
        })
    }));
    let responses = vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "noop", serde_json::Map::new())],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ];
    assert_eq!(
        run(config, responses).await,
        vec![Effort::Low, Effort::High]
    );
}

/// A model switch with no explicit level re-clamps the one in flight rather
/// than carrying a rung the new model never advertised.
#[tokio::test]
async fn a_switch_reclamps_the_current_effort_onto_the_new_model() {
    let mut capped = faux_model();
    capped.id = "capped".to_owned();
    capped.thinking_level_map = Some(serde_json::json!({"low": null, "medium": null}));

    let mut config = LoopConfig::new(faux_model());
    config.effort = Effort::Low;
    config.prepare_next_turn = Some(Box::new(move |_| {
        Some(NextTurn {
            model: Some(capped.clone()),
            thinking: None,
        })
    }));
    let responses = vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "noop", serde_json::Map::new())],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ];
    assert_eq!(
        run(config, responses).await,
        vec![Effort::Low, Effort::High]
    );
}

#[tokio::test]
async fn a_forced_choice_is_spent_on_the_first_turn_and_gone_by_the_second()
-> Result<(), Box<dyn std::error::Error>> {
    let mut config = LoopConfig::new(faux_model());
    config.first_turn_tool_choice = Some(ToolChoice::Tool(ForcedTool::new("noop")?));
    let responses = vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "noop", serde_json::Map::new())],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ];
    let choices: Vec<Option<ToolChoice>> = run_spied(config, responses)
        .await
        .into_iter()
        .map(|(_, choice)| choice)
        .collect();
    assert_eq!(
        choices,
        vec![Some(ToolChoice::Tool(ForcedTool::new("noop")?)), None]
    );
    Ok(())
}

/// Dies with the wind-down ending the run on a tool call: the stopped run gets one last
/// request, with tool choice `none`, and no second one however that turn ends.
#[tokio::test]
async fn a_stop_after_a_tool_call_asks_once_for_a_tool_free_last_word() {
    let mut config = LoopConfig::new(faux_model());
    config.should_stop_after_turn = Some(Box::new(|_| true));
    config.last_word = Some(Box::new(|_| {
        Some(AgentMessage::host_user(
            yi_types::message::UserContent::Text("[deadline] Time is up".to_owned()),
            0,
        ))
    }));
    let call = || {
        faux_assistant_message(
            vec![faux_tool_call("call-1", "noop", serde_json::Map::new())],
            StopReason::ToolUse,
        )
    };
    let choices: Vec<Option<ToolChoice>> = run_spied(config, vec![call(), call()])
        .await
        .into_iter()
        .map(|(_, choice)| choice)
        .collect();
    assert_eq!(choices, vec![None, Some(ToolChoice::None)]);
}

/// Records every request's messages and answers `done`.
struct Recorder(Arc<Mutex<Vec<Vec<AgentMessage>>>>);

impl yi_loop::run::StreamFn for Recorder {
    fn stream(
        &self,
        _model: &Model,
        context: &LlmContext,
        _effort: Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut seen) = self.0.lock() {
            seen.push(context.messages.clone());
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let message = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
        let _ = sender.try_send(AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message,
        });
        receiver
    }
}

/// The per-request tail (the environment block) trails the request and never enters the
/// history; it used to be appended to a full copy of the history on every request.
#[tokio::test]
async fn the_request_tail_trails_each_request_and_stays_out_of_the_history() {
    let text = |value: &str| {
        AgentMessage::host_user(yi_types::message::UserContent::Text(value.to_owned()), 0)
    };
    let mut config = LoopConfig::new(faux_model());
    config.request_tail = Some(Box::new(move || vec![text("tail")]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let mut emit = |_: AgentEvent| {};
    let stream = Recorder(Arc::clone(&seen));
    let signal = InterruptSignal::default();
    let prompt = vec![text("hi")];
    run_loop(&mut context, prompt, &config, &signal, &mut emit, &stream).await;
    let seen = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    assert_eq!(seen, vec![vec![text("hi"), text("tail")]]);
    assert!(
        !context.messages.contains(&text("tail")),
        "{:?}",
        context.messages
    );
}

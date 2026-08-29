use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::Receiver;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{AgentTool, ToolFuture, ToolOutcome, error_tool_result};
use yi_loop::{LoopConfig, LoopContext, NextTurn, run_loop};
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Effort, LlmContext, Model, ModelCost, ToolDef};

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

/// Records the effort each turn's request went out with.
struct Spy {
    seen: Arc<Mutex<Vec<Effort>>>,
    responses: Mutex<Vec<AgentMessage>>,
}

impl yi_loop::run::StreamFn for Spy {
    fn stream(
        &self,
        _model: &Model,
        _context: &LlmContext,
        effort: Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(effort);
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
        vec![AgentMessage::User {
            content: yi_types::message::UserContent::Text("hi".to_owned()),
            timestamp: 0,
        }],
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
    assert_eq!(run(config, responses).await, vec![Effort::Low, Effort::High]);
}

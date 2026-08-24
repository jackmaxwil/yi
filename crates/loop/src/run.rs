use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::sync::mpsc::Receiver;
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, Cost, StopReason, Usage};
use yi_types::model::{LlmContext, Model, ToolDef};

use crate::config::{ExecutionMode, LoopConfig, TurnSnapshot};
use crate::interrupt::InterruptSignal;
use crate::tool::{AgentTool, ToolOutcome, error_tool_result};

pub struct LoopContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<Arc<dyn AgentTool>>,
}

pub trait StreamFn: Send + Sync {
    fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent>;
}

fn zero_usage() -> Usage {
    let zero = || serde_json::Number::from(0u64);
    Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: Cost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            total: zero(),
        },
    }
}

fn synthesized_error_message(model: &Model, text: &str) -> AgentMessage {
    AgentMessage::Assistant {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: zero_usage(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(text.to_owned()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn stop_reason_of(message: &AgentMessage) -> StopReason {
    match message {
        AgentMessage::Assistant { stop_reason, .. } => *stop_reason,
        _ => StopReason::Stop,
    }
}

struct ExtractedCall {
    id: String,
    name: String,
    arguments: Map<String, Value>,
}

fn extract_tool_calls(message: &AgentMessage) -> Vec<ExtractedCall> {
    let AgentMessage::Assistant { content, .. } = message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|block| match block {
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } => Some(ExtractedCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            _ => None,
        })
        .collect()
}

struct Finalized {
    call: ExtractedCall,
    result: ToolResult,
    is_error: bool,
}

fn tool_result_message(finalized: &Finalized) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: finalized.call.id.clone(),
        tool_name: finalized.call.name.clone(),
        content: finalized.result.content.clone(),
        details: Some(finalized.result.details.clone()),
        usage: finalized.result.usage.clone(),
        added_tool_names: finalized
            .result
            .added_tool_names
            .as_ref()
            .filter(|names| !names.is_empty())
            .cloned(),
        is_error: finalized.is_error,
        timestamp: 0,
    }
}

fn emit_tool_batch_events(
    finalized: &[Finalized],
    emit: &mut dyn FnMut(AgentEvent),
) -> Vec<AgentMessage> {
    let mut messages = Vec::new();
    for item in finalized {
        let message = tool_result_message(item);
        emit(AgentEvent::MessageStart {
            message: message.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
        messages.push(message);
    }
    messages
}

fn should_terminate(finalized: &[Finalized]) -> bool {
    !finalized.is_empty()
        && finalized
            .iter()
            .all(|item| item.result.terminate == Some(true))
}

async fn execute_one(
    tools: &[Arc<dyn AgentTool>],
    call: ExtractedCall,
    signal: &InterruptSignal,
) -> Finalized {
    let Some(tool) = tools
        .iter()
        .find(|tool| tool.definition().name == call.name)
    else {
        let text = format!("Tool {} not found", call.name);
        return Finalized {
            call,
            result: error_tool_result(&text),
            is_error: true,
        };
    };
    if let Err(reason) = tool.validate(&call.arguments) {
        return Finalized {
            call,
            result: error_tool_result(&reason),
            is_error: true,
        };
    }
    if signal.is_fired() {
        return Finalized {
            call,
            result: error_tool_result("Operation aborted"),
            is_error: true,
        };
    }
    let ToolOutcome { result, is_error } =
        tool.execute(&call.id, call.arguments.clone(), signal).await;
    Finalized {
        call,
        result,
        is_error,
    }
}

async fn execute_tool_calls(
    context: &LoopContext,
    calls: Vec<ExtractedCall>,
    mode: ExecutionMode,
    signal: &InterruptSignal,
    emit: &mut dyn FnMut(AgentEvent),
) -> (Vec<Finalized>, bool) {
    let sequential = mode == ExecutionMode::Sequential
        || calls.iter().any(|call| {
            context.tools.iter().any(|tool| {
                tool.definition().name == call.name
                    && tool.execution_mode() == ExecutionMode::Sequential
            })
        });
    let mut finalized = Vec::new();
    for call in calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            args: Value::Object(call.arguments.clone()),
        });
        let item = execute_one(&context.tools, call, signal).await;
        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: item.call.id.clone(),
            tool_name: item.call.name.clone(),
            result: item.result.clone(),
            is_error: item.is_error,
        });
        let aborted = signal.is_fired();
        finalized.push(item);
        if aborted {
            break;
        }
    }
    let _ = sequential;
    let terminate = should_terminate(&finalized);
    (finalized, terminate)
}

fn fail_truncated_calls(
    calls: Vec<ExtractedCall>,
    emit: &mut dyn FnMut(AgentEvent),
) -> Vec<Finalized> {
    calls
        .into_iter()
        .map(|call| {
            emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            });
            let text = format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                call.name
            );
            let item = Finalized {
                call,
                result: error_tool_result(&text),
                is_error: true,
            };
            emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: item.call.id.clone(),
                tool_name: item.call.name.clone(),
                result: item.result.clone(),
                is_error: item.is_error,
            });
            item
        })
        .collect()
}

async fn stream_assistant_response<S: StreamFn>(
    context: &mut LoopContext,
    config: &LoopConfig,
    model: &Model,
    signal: &InterruptSignal,
    emit: &mut dyn FnMut(AgentEvent),
    stream: &S,
) -> AgentMessage {
    let mut messages = context.messages.clone();
    if let Some(transform) = &config.transform_context
        && let Some(transformed) = transform(&messages)
    {
        messages = transformed;
    }
    let llm_messages = (config.convert_to_llm)(&messages);
    let tool_defs: Vec<ToolDef> = context.tools.iter().map(|tool| tool.definition()).collect();
    let llm_context = LlmContext {
        system_prompt: context.system_prompt.clone(),
        messages: llm_messages,
        tools: if tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs)
        },
    };

    let mut receiver = stream.stream(model, &llm_context, signal);
    let mut added_partial = false;
    let mut final_message: Option<AgentMessage> = None;
    while let Some(event) = receiver.recv().await {
        match &event {
            AssistantMessageEvent::Start { partial } => {
                context.messages.push(partial.clone());
                added_partial = true;
                emit(AgentEvent::MessageStart {
                    message: partial.clone(),
                });
            }
            AssistantMessageEvent::Done { message, .. } => {
                final_message = Some(message.clone());
                break;
            }
            AssistantMessageEvent::Error { error, .. } => {
                final_message = Some(error.clone());
                break;
            }
            other => {
                if added_partial {
                    let partial = match other {
                        AssistantMessageEvent::TextStart { partial, .. }
                        | AssistantMessageEvent::TextDelta { partial, .. }
                        | AssistantMessageEvent::TextEnd { partial, .. }
                        | AssistantMessageEvent::ThinkingStart { partial, .. }
                        | AssistantMessageEvent::ThinkingDelta { partial, .. }
                        | AssistantMessageEvent::ThinkingEnd { partial, .. }
                        | AssistantMessageEvent::ToolCallStart { partial, .. }
                        | AssistantMessageEvent::ToolCallDelta { partial, .. }
                        | AssistantMessageEvent::ToolCallEnd { partial, .. } => partial.clone(),
                        AssistantMessageEvent::Start { partial } => partial.clone(),
                        AssistantMessageEvent::Done { message, .. } => message.clone(),
                        AssistantMessageEvent::Error { error, .. } => error.clone(),
                    };
                    if let Some(last) = context.messages.last_mut() {
                        *last = partial.clone();
                    }
                    emit(AgentEvent::MessageUpdate {
                        message: partial,
                        assistant_message_event: event.clone(),
                    });
                }
            }
        }
    }
    let final_message = final_message.unwrap_or_else(|| {
        synthesized_error_message(model, "Provider stream ended without a terminal event")
    });
    if added_partial {
        if let Some(last) = context.messages.last_mut() {
            *last = final_message.clone();
        }
    } else {
        context.messages.push(final_message.clone());
        emit(AgentEvent::MessageStart {
            message: final_message.clone(),
        });
    }
    emit(AgentEvent::MessageEnd {
        message: final_message.clone(),
    });
    final_message
}

pub async fn run_loop<S: StreamFn>(
    context: &mut LoopContext,
    new_messages: Vec<AgentMessage>,
    config: &LoopConfig,
    signal: &InterruptSignal,
    emit: &mut dyn FnMut(AgentEvent),
    stream: &S,
) -> Vec<AgentMessage> {
    let mut collected: Vec<AgentMessage> = Vec::new();
    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    for prompt in new_messages {
        emit(AgentEvent::MessageStart {
            message: prompt.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: prompt.clone(),
        });
        context.messages.push(prompt.clone());
        collected.push(prompt);
    }

    let mut current_model = config.model.clone();
    let mut first_turn = true;
    let mut pending: Vec<AgentMessage> = config
        .get_steering_messages
        .as_ref()
        .map_or_else(Vec::new, |get| get());

    loop {
        let mut has_more_tool_calls = true;
        while has_more_tool_calls || !pending.is_empty() {
            if !first_turn {
                emit(AgentEvent::TurnStart);
            } else {
                first_turn = false;
            }
            for message in pending.drain(..) {
                emit(AgentEvent::MessageStart {
                    message: message.clone(),
                });
                emit(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
                context.messages.push(message.clone());
                collected.push(message);
            }

            let message =
                stream_assistant_response(context, config, &current_model, signal, emit, stream)
                    .await;
            collected.push(message.clone());

            let reason = stop_reason_of(&message);
            if reason == StopReason::Error || reason == StopReason::Aborted {
                emit(AgentEvent::TurnEnd {
                    message,
                    tool_results: Vec::new(),
                });
                emit(AgentEvent::AgentEnd {
                    messages: collected.clone(),
                });
                return collected;
            }

            let calls = extract_tool_calls(&message);
            let mut tool_results: Vec<AgentMessage> = Vec::new();
            has_more_tool_calls = false;
            if !calls.is_empty() {
                let (finalized, terminate) = if reason == StopReason::Length {
                    (fail_truncated_calls(calls, emit), false)
                } else {
                    let (finalized, terminate) =
                        execute_tool_calls(context, calls, config.tool_execution, signal, emit)
                            .await;
                    (finalized, terminate)
                };
                tool_results = emit_tool_batch_events(&finalized, emit);
                has_more_tool_calls = !terminate;
                for result in &tool_results {
                    context.messages.push(result.clone());
                    collected.push(result.clone());
                }
            }

            emit(AgentEvent::TurnEnd {
                message: message.clone(),
                tool_results: tool_results.clone(),
            });

            let snapshot = TurnSnapshot {
                message: &message,
                tool_results: &tool_results,
            };
            if let Some(prepare) = &config.prepare_next_turn
                && let Some(next) = prepare(&snapshot)
                && let Some(model) = next.model
            {
                current_model = model;
            }
            if let Some(should_stop) = &config.should_stop_after_turn
                && should_stop(&snapshot)
            {
                emit(AgentEvent::AgentEnd {
                    messages: collected.clone(),
                });
                return collected;
            }
            pending = config
                .get_steering_messages
                .as_ref()
                .map_or_else(Vec::new, |get| get());
        }

        let follow_ups = config
            .get_follow_up_messages
            .as_ref()
            .map_or_else(Vec::new, |get| get());
        if follow_ups.is_empty() {
            break;
        }
        pending = follow_ups;
    }

    emit(AgentEvent::AgentEnd {
        messages: collected.clone(),
    });
    collected
}

use std::collections::VecDeque;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::sync::mpsc::Receiver;
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, StopReason, Usage};
use yi_types::model::{Effort, LlmContext, Model, ToolChoice, ToolDef};
use yi_types::subagent::LoopSignal;

use crate::config::{ExecutionMode, LoopConfig, TurnSnapshot};
use crate::interrupt::InterruptSignal;
use crate::reasoning::ReasoningBudget;
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
        effort: Effort,
        signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent>;
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
        usage: Usage::unknown(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(text.to_owned()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// The interrupt lands mid-stream, so what was already shown becomes the turn's message with
/// an aborted stop reason: the user stopped the answer, not what already streamed.
fn aborted_message(partial: Option<&AgentMessage>, model: &Model) -> AgentMessage {
    let mut message = partial
        .cloned()
        .unwrap_or_else(|| synthesized_error_message(model, ""));
    if let AgentMessage::Assistant {
        stop_reason,
        error_message,
        ..
    } = &mut message
    {
        *stop_reason = StopReason::Aborted;
        *error_message = None;
    }
    message
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

pub const UNPARSED_MARKUP: &str = "model emitted tool-call markup that could not be parsed";
const CALL_MARKUP: &str = "<tool_call>";

/// A message that opens with call markup and carries no call is a failed call, not an answer.
fn flag_unparsed_markup(mut message: AgentMessage) -> AgentMessage {
    if let AgentMessage::Assistant {
        content,
        stop_reason,
        error_message,
        ..
    } = &mut message
        && *stop_reason == StopReason::Stop
        && !content
            .iter()
            .any(|block| matches!(block, Content::ToolCall { .. }))
        && content.iter().any(|block| {
            matches!(block, Content::Text { text, .. } if text.trim_start().starts_with(CALL_MARKUP))
        })
    {
        *stop_reason = StopReason::Error;
        *error_message = Some(UNPARSED_MARKUP.to_owned());
    }
    message
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
    emit: &mut (dyn FnMut(AgentEvent) + Send),
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
    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    // One deterministic repair, then the call fails with the real error.
    let resolved = crate::repair::repair_tool_name(&call.name, &borrowed)
        .map(str::to_owned)
        .unwrap_or_else(|| call.name.clone());
    let Some(tool) = tools.iter().find(|tool| tool.definition().name == resolved) else {
        let text = format!("Tool {} not found", call.name);
        return Finalized {
            call,
            result: crate::tool::error_tool_result_kind(
                &text,
                yi_types::event::ToolErrorKind::NotFound,
            ),
            is_error: true,
        };
    };
    if let Err(reason) = tool.validate(&call.arguments) {
        return Finalized {
            call,
            result: crate::tool::error_tool_result_kind(
                &reason,
                yi_types::event::ToolErrorKind::InvalidArgs,
            ),
            is_error: true,
        };
    }
    if signal.is_fired() {
        return Finalized {
            call,
            result: crate::tool::error_tool_result_kind(
                "Operation aborted",
                yi_types::event::ToolErrorKind::Aborted,
            ),
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

/// Timing and size are stamped where every tool result funnels through, so
/// the session JSONL can answer per-tool latency without an events pipeline.
fn stamp_details(item: &mut Finalized, duration_ms: u64) {
    let out_bytes: usize = item
        .result
        .content
        .iter()
        .map(|block| match block {
            yi_types::message::Content::Text { text, .. } => text.len(),
            _ => 0,
        })
        .sum();
    if let Value::Object(details) = &mut item.result.details {
        details.insert("durationMs".to_owned(), Value::from(duration_ms));
        details
            .entry("outBytes".to_owned())
            .or_insert(Value::from(out_bytes));
    }
}

async fn execute_timed(
    tools: &[Arc<dyn AgentTool>],
    call: ExtractedCall,
    signal: &InterruptSignal,
) -> Finalized {
    let started = std::time::Instant::now();
    let mut item = execute_one(tools, call, signal).await;
    let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    stamp_details(&mut item, duration);
    item
}

/// Completed slots are never polled again, and the blocking work underneath
/// runs on tokio's blocking pool, so read-kind batches genuinely overlap.
async fn join_all<F: std::future::Future<Output = Finalized>>(
    mut futures: Vec<std::pin::Pin<Box<F>>>,
) -> Vec<Finalized> {
    use std::task::Poll;
    let mut results: Vec<Option<Finalized>> = futures.iter().map(|_| None).collect();
    let mut filled = 0_usize;
    std::future::poll_fn(|waker_context| {
        for (index, future) in futures.iter_mut().enumerate() {
            if results.get(index).is_some_and(std::option::Option::is_none)
                && let Poll::Ready(value) = future.as_mut().poll(waker_context)
                && let Some(slot) = results.get_mut(index)
            {
                *slot = Some(value);
                filled = filled.saturating_add(1);
            }
        }
        // Invariant: every slot filled is the exit condition, which is what
        // makes the flatten below total rather than a silent drop.
        if filled == futures.len() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    results.into_iter().flatten().collect()
}

async fn execute_tool_calls(
    context: &LoopContext,
    calls: Vec<ExtractedCall>,
    mode: ExecutionMode,
    signal: &InterruptSignal,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> (Vec<Finalized>, bool) {
    let parallel_ok = |call: &ExtractedCall| {
        mode == ExecutionMode::Parallel
            && context.tools.iter().any(|tool| {
                tool.definition().name == call.name
                    && tool.execution_mode(&call.arguments) == ExecutionMode::Parallel
            })
    };
    let mut finalized = Vec::new();
    let mut queue = calls.into_iter().peekable();
    while queue.peek().is_some() {
        // A maximal run of parallel-safe calls overlaps; the first mutating
        // call closes the run, so writes keep today's strict ordering.
        let mut batch: Vec<ExtractedCall> = queue.next().into_iter().collect();
        if batch.first().is_some_and(&parallel_ok) {
            while queue.peek().is_some_and(&parallel_ok) {
                batch.extend(queue.next());
            }
        }
        for call in &batch {
            emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            });
        }
        let items = join_all(
            batch
                .into_iter()
                .map(|call| Box::pin(execute_timed(&context.tools, call, signal)))
                .collect(),
        )
        .await;
        for item in items {
            emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: item.call.id.clone(),
                tool_name: item.call.name.clone(),
                result: item.result.clone(),
                is_error: item.is_error,
            });
            finalized.push(item);
        }
    }
    let terminate = should_terminate(&finalized);
    (finalized, terminate)
}

fn fail_truncated_calls(
    calls: Vec<ExtractedCall>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
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

pub const LENGTH_REDRIVE_CUSTOM_TYPE: &str = "length_redrive";
pub const LENGTH_REDRIVE_TEXT: &str = "The reply hit the output limit before any tool call. Pick the most boring viable option and act on it now: make the tool call, then explain.";
/// Consecutive length stops, bare or with a truncated call (D178); a call that runs resets them.
pub const LENGTH_STOP_AT: u32 = 3;
/// Reasoning cuts per prompt (D178, amends D168): a tool call between cuts reset the old count
/// of twelve, so the audit slice's 22 photonic cuts, four in a row at most, never came near it.
pub const CUT_STOP_AT: u32 = 6;
pub const CUT_REDRIVE_TEXT: &str = "The reply was cut at the reasoning budget again. Stop deriving: write the first version of the file the task names now, even a stub that runs, in one `write` call, and reason after it exists.";
const CUT_QUOTE_TEXT: &str = "The cut reasoning is not in this conversation; it ended with the lines below, so write from where they stop instead of deriving it again.";

struct Cut {
    chars: usize,
    tail: String,
}

/// A turn that spent its whole output budget thinking, or was cut at the reasoning budget, is
/// sent back to act; from the prompt's second cut it is told what to write and where it stopped.
fn length_redrive(rung: u32, cut: Option<&Cut>) -> AgentMessage {
    let details = match cut {
        Some(cut) => {
            json!({"rung": rung, "cut": true, "reasoningChars": cut.chars, "signal": LoopSignal::LengthRedrive})
        }
        None => json!({"rung": rung, "cut": false, "signal": LoopSignal::LengthRedrive}),
    };
    // Incident: coq-block-bound's cut requests carried only the nudge; six turns derived from zero
    let text = match cut {
        Some(cut) if rung >= 2 && !cut.tail.is_empty() => format!(
            "{CUT_REDRIVE_TEXT}\n\n{CUT_QUOTE_TEXT}\n<cut_reasoning>\n{}\n</cut_reasoning>",
            cut.tail
        ),
        Some(_) if rung >= 2 => CUT_REDRIVE_TEXT.to_owned(),
        _ => LENGTH_REDRIVE_TEXT.to_owned(),
    };
    AgentMessage::Custom {
        custom_type: LENGTH_REDRIVE_CUSTOM_TYPE.to_owned(),
        content: yi_types::message::UserContent::Text(text),
        display: false,
        details: Some(details),
        timestamp: 0,
    }
}

/// The cut turn as the record keeps it: a bare length stop with no thinking block, so at most a
/// re-drive's quote of the runaway rides a later request, and the char count as its usage.
fn cut_message(partial: Option<&AgentMessage>, model: &Model, chars: usize) -> AgentMessage {
    let mut message = partial
        .cloned()
        .unwrap_or_else(|| synthesized_error_message(model, ""));
    if let AgentMessage::Assistant {
        content,
        stop_reason,
        usage,
        error_message,
        ..
    } = &mut message
    {
        content.retain(|block| !matches!(block, Content::Thinking { .. }));
        *stop_reason = StopReason::Length;
        // a settled usage is the measurement; the estimate stands only while it is unknown
        if usage.unknown {
            usage.reasoning = Some(i64::try_from(chars / 4).unwrap_or(i64::MAX));
        }
        *error_message = None;
    }
    message
}

/// After the cut the provider settles the turn (up to about thirty seconds) and sends its
/// `Done`; a stream that closes instead leaves the estimate. The deadline is the only bound:
/// the channel holds 256 events and a fast upstream has that many deltas queued ahead of the
/// `Done` at the moment of the cut, and the pump stops reading at the flag.
const CUT_DRAIN: std::time::Duration = std::time::Duration::from_secs(45);

/// The provider's `Done` after a cut, if it comes within the bound; its usage is settled.
async fn drain_to_done(
    receiver: &mut tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
) -> Option<AgentMessage> {
    let deadline = tokio::time::Instant::now() + CUT_DRAIN;
    loop {
        match tokio::time::timeout_at(deadline, receiver.recv()).await {
            Ok(Some(AssistantMessageEvent::Done { message, .. })) => return Some(message),
            Ok(Some(AssistantMessageEvent::Error { .. }) | None) | Err(_) => return None,
            Ok(Some(_)) => {}
        }
    }
}

pub const STREAM_RETRY_CUSTOM_TYPE: &str = "stream_retry";
pub const STREAM_RETRY_TEXT: &str = "The provider dropped the stream before any output reached the transcript; the same turn runs again.";
pub const STREAM_RETRY_AT: u32 = 1;

/// A stream that was generating, died on the wire or ended on an in-band error chunk, and
/// showed nothing; a synthesized error with zero usage never went to a provider.
fn nothing_delivered(message: &AgentMessage) -> bool {
    let AgentMessage::Assistant {
        content,
        usage,
        error_message,
        raw_stop_reason,
        ..
    } = message
    else {
        return false;
    };
    let generating = usage.output > 0 || usage.reasoning.unwrap_or(0) > 0;
    let wire = error_message.as_deref().is_some_and(|text| {
        yi_types::telemetry::ErrorClass::from_provider_text(text).is_transport()
    });
    // Incident: Wafer's in-band 502 had no usage and no transport class; yi exited 0 (D175)
    let in_band = raw_stop_reason.as_deref() == Some(yi_types::message::RAW_STOP_IN_BAND_ERROR);
    (generating || wire || in_band)
        && !content.iter().any(|block| match block {
            Content::Text { text, .. } => !text.trim().is_empty(),
            Content::ToolCall { .. } => true,
            _ => false,
        })
}

/// A dropped stream that showed nothing is the one error a rerun cannot duplicate.
fn stream_retry(error: Option<&str>) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: STREAM_RETRY_CUSTOM_TYPE.to_owned(),
        content: yi_types::message::UserContent::Text(STREAM_RETRY_TEXT.to_owned()),
        display: false,
        details: Some(serde_json::json!({"error": error})),
        timestamp: 0,
    }
}

pub const REPEAT_BREAK_CUSTOM_TYPE: &str = "repeat_break";
pub const REPEAT_BREAK_TEXT: &str = "The same tool calls have run the last three turns. Nothing is changing. Write the final answer now; call no tools.";
pub const REPEAT_STEER_AT: u32 = 3;
pub const REPEAT_STOP_AT: u32 = 6;
const REPEAT_WINDOW: usize = 6;

/// A turn's tool batch by name and arguments, or a tool-less turn's text; a bash job poll (no
/// `command`) or a cut turn (no text) yields nothing, so waiting on a job never trips the breaker.
fn batch_signature(message: &AgentMessage) -> Option<String> {
    let calls = extract_tool_calls(message);
    if calls.is_empty() {
        let text = message.plain_text();
        let words: Vec<&str> = text.split_whitespace().collect();
        return (!words.is_empty()).then(|| {
            format!(
                "text:{}",
                words.join(" ").chars().take(256).collect::<String>()
            )
        });
    }
    if calls
        .iter()
        .all(|call| call.name == "bash" && !call.arguments.contains_key("command"))
    {
        return None;
    }
    Some(
        calls
            .iter()
            .map(|call| format!("{}:{}", call.name, Value::Object(call.arguments.clone())))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// A third copy in six turns, the last three all repeats, is sent back once; six end the run.
fn repeat_break() -> AgentMessage {
    AgentMessage::Custom {
        custom_type: REPEAT_BREAK_CUSTOM_TYPE.to_owned(),
        content: yi_types::message::UserContent::Text(REPEAT_BREAK_TEXT.to_owned()),
        display: false,
        details: Some(json!({"signal": LoopSignal::RepeatBreak})),
        timestamp: 0,
    }
}

struct TurnRequest<'a> {
    model: &'a Model,
    effort: Effort,
    tool_choice: Option<ToolChoice>,
    watch: bool,
}

async fn due_now(due: Option<&(dyn Fn() -> bool + Send + Sync)>, watch: bool) {
    let Some(due) = due.filter(|_| watch) else {
        return std::future::pending().await;
    };
    while !due() {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn stream_assistant_response<S: StreamFn>(
    context: &mut LoopContext,
    config: &LoopConfig,
    turn: TurnRequest<'_>,
    signal: &InterruptSignal,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
    stream: &S,
) -> (AgentMessage, Option<Cut>, bool) {
    let TurnRequest {
        model,
        effort,
        tool_choice,
        watch,
    } = turn;
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
        tool_choice,
    };

    signal.clear_cut();
    let mut receiver = if signal.is_fired() {
        tokio::sync::mpsc::channel(1).1
    } else {
        stream.stream(model, &llm_context, effort, signal)
    };
    let mut added_partial = false;
    let mut final_message: Option<AgentMessage> = None;
    let mut budget = ReasoningBudget::default();
    let mut cut: Option<usize> = None;
    let mut timed_out = false;
    loop {
        // The provider's stream takes no cancellation input, so this is a streaming answer's
        // only interrupt checkpoint; without it a tool-less turn ran to completion first.
        let event = tokio::select! {
            biased;
            () = signal.wait() => None,
            () = due_now(config.last_word_due.as_deref(), watch) => {
                timed_out = true;
                signal.cut();
                None
            }
            event = receiver.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
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
            AssistantMessageEvent::Waiting { wait } => emit(AgentEvent::Wait {
                wait: Some(wait.clone()),
            }),
            other => {
                match other {
                    AssistantMessageEvent::ThinkingDelta { delta, .. } => cut = budget.push(delta),
                    AssistantMessageEvent::TextStart { .. }
                    | AssistantMessageEvent::ToolCallStart { .. } => budget.disarm(),
                    _ => {}
                }
                if added_partial {
                    // A delta is a delta (D145): the message grows in place, once.
                    if let Some(last) = context.messages.last_mut() {
                        yi_types::event::apply(last, other);
                    }
                    emit(AgentEvent::MessageUpdate {
                        assistant_message_event: event.clone(),
                    });
                }
                if cut.is_some() {
                    signal.cut();
                    break;
                }
            }
        }
    }
    let cut = match cut {
        Some(chars) => {
            let settled = drain_to_done(&mut receiver).await;
            let source = settled
                .as_ref()
                .or_else(|| added_partial.then(|| context.messages.last()).flatten());
            let tail = source.map(crate::reasoning::tail).unwrap_or_default();
            final_message = Some(cut_message(source, model, chars));
            Some(Cut { chars, tail })
        }
        None => None,
    };
    let final_message = final_message.unwrap_or_else(|| {
        if signal.is_fired() || timed_out {
            aborted_message(
                added_partial.then(|| context.messages.last()).flatten(),
                model,
            )
        } else {
            synthesized_error_message(model, "Provider stream ended without a terminal event")
        }
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
    (final_message, cut, timed_out)
}

pub async fn run_loop<S: StreamFn>(
    context: &mut LoopContext,
    new_messages: Vec<AgentMessage>,
    config: &LoopConfig,
    signal: &InterruptSignal,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
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
    let mut current_effort = config.effort;
    let mut first_turn = true;
    let mut length_stops: u32 = 0;
    let mut cut_stops: u32 = 0;
    let mut stream_retries: u32 = 0;
    let mut recent: VecDeque<String> = VecDeque::with_capacity(REPEAT_WINDOW);
    let mut repeating: u32 = 0;
    let mut steered = false;
    let mut last_word_said = false;
    let mut tool_choice = config.first_turn_tool_choice.clone();
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

            if !signal.is_fired()
                && let Some(compact) = &config.maybe_compact
                && let Some(compacted) = compact(&context.messages).await
            {
                context.messages = compacted;
            }
            let (message, cut, timed_out) = stream_assistant_response(
                context,
                config,
                TurnRequest {
                    model: &current_model,
                    effort: current_effort,
                    tool_choice: tool_choice.take(),
                    watch: !last_word_said,
                },
                signal,
                emit,
                stream,
            )
            .await;
            let message = flag_unparsed_markup(message);
            collected.push(message.clone());

            let reason = stop_reason_of(&message);
            if timed_out {
                emit(AgentEvent::TurnEnd {
                    message: message.clone(),
                    tool_results: Vec::new(),
                });
                let snapshot = TurnSnapshot {
                    message: &message,
                    tool_results: &[],
                };
                if let Some(word) = config.last_word.as_ref().and_then(|word| word(&snapshot)) {
                    last_word_said = true;
                    tool_choice = Some(ToolChoice::None);
                    pending = vec![word];
                    has_more_tool_calls = false;
                    continue;
                }
                emit(AgentEvent::AgentEnd {
                    messages: collected.clone(),
                });
                return collected;
            }
            if reason == StopReason::Error
                && stream_retries < STREAM_RETRY_AT
                && nothing_delivered(&message)
            {
                stream_retries = stream_retries.saturating_add(1);
                emit(AgentEvent::TurnEnd {
                    message: message.clone(),
                    tool_results: Vec::new(),
                });
                let error = match &message {
                    AgentMessage::Assistant { error_message, .. } => error_message.as_deref(),
                    _ => None,
                };
                pending.push(stream_retry(error));
                continue;
            }
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
            // a clean turn ends the error streak: the next error gets its own retry
            stream_retries = 0;

            let waits_before = config.waiting.as_ref().map(|count| count());
            let calls = extract_tool_calls(&message);
            let mut tool_results: Vec<AgentMessage> = Vec::new();
            has_more_tool_calls = false;
            if !calls.is_empty() {
                let (finalized, terminate) = if reason == StopReason::Length {
                    length_stops = length_stops.saturating_add(1);
                    (fail_truncated_calls(calls, emit), false)
                } else {
                    length_stops = 0;
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
            } else if reason == StopReason::Length {
                // a cut is not a length strike: it has its own, longer count (D168)
                if cut.is_some() {
                    cut_stops = cut_stops.saturating_add(1);
                } else {
                    length_stops = length_stops.saturating_add(1);
                }
            }

            // The host saw the batch block on the family: a wait for work, never a repeat.
            let waited = waits_before
                .zip(config.waiting.as_ref())
                .is_some_and(|(before, count)| count() > before);
            let repeats = match batch_signature(&message).filter(|_| !waited) {
                Some(batch) => {
                    if recent.len() >= REPEAT_WINDOW {
                        recent.pop_front();
                    }
                    let seen = recent.iter().filter(|past| **past == batch).count();
                    recent.push_back(batch);
                    // a batch new to the window is progress: the next stretch earns its own steer
                    if seen == 0 {
                        steered = false;
                        repeating = 0;
                    } else {
                        repeating = repeating.saturating_add(1);
                    }
                    u32::try_from(seen.saturating_add(1)).unwrap_or(u32::MAX)
                }
                None => 0,
            };
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
            {
                if let Some(model) = next.model {
                    current_model = model;
                }
                current_effort =
                    current_model.clamp_effort(next.thinking.unwrap_or(current_effort));
            }
            if let Some(should_stop) = &config.should_stop_after_turn
                && should_stop(&snapshot)
            {
                let word = config.last_word.as_ref().filter(|_| has_more_tool_calls);
                if let Some(word) = word
                    .filter(|_| !last_word_said)
                    .and_then(|word| word(&snapshot))
                {
                    last_word_said = true;
                    tool_choice = Some(ToolChoice::None);
                    pending = vec![word];
                    continue;
                }
                emit(AgentEvent::AgentEnd {
                    messages: collected.clone(),
                });
                return collected;
            }
            if repeats >= REPEAT_STOP_AT
                || length_stops >= LENGTH_STOP_AT
                || cut_stops >= CUT_STOP_AT
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
            // after three repeating turns in a row, so a fresh edit before each check is progress,
            // and once per stretch: two batches taking turns hold the count at three for good
            if repeats >= REPEAT_STEER_AT && repeating >= REPEAT_STEER_AT && !steered {
                steered = true;
                pending.push(repeat_break());
            }
            if !has_more_tool_calls && pending.is_empty() && reason == StopReason::Length {
                let rung = if cut.is_some() {
                    cut_stops
                } else {
                    length_stops
                };
                pending.push(length_redrive(rung, cut.as_ref()));
            } else if !has_more_tool_calls
                && pending.is_empty()
                && let Some(intercept) = &config.intercept_stop
                && let Some(message) = intercept(&snapshot)
            {
                pending.push(message);
            }
        }

        let follow_ups = config
            .get_follow_up_messages
            .as_ref()
            .map_or_else(Vec::new, |get| get());
        if follow_ups.is_empty() {
            break;
        }
        cut_stops = 0;
        recent.clear();
        pending = follow_ups;
    }

    emit(AgentEvent::AgentEnd {
        messages: collected.clone(),
    });
    collected
}

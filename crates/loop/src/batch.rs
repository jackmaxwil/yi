//! One message's tool calls: run in order, read-kind runs overlapped, each answered once.

use std::sync::Arc;

use serde_json::Value;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::AgentMessage;

use crate::config::ExecutionMode;
use crate::interrupt::InterruptSignal;
use crate::run::{ExtractedCall, LoopContext};
use crate::tool::{AgentTool, ToolOutcome, error_tool_result};

pub(crate) struct Finalized {
    call: ExtractedCall,
    result: ToolResult,
    is_error: bool,
    stops_message: bool,
}

fn tool_result_message(finalized: &Finalized) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: finalized.call.id.clone(),
        tool_name: finalized.call.name.clone(),
        content: finalized.result.content.clone(),
        details: Some(finalized.result.details.clone()),
        usage: finalized.result.usage.clone(),
        added_tool_names: None,
        is_error: finalized.is_error,
        timestamp: 0,
    }
}

pub(crate) fn emit_tool_batch_events(
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
            stops_message: false,
        };
    };
    if let Err(reason) = tool.validate(&call.arguments) {
        let stops_message = tool.gates_message(&call.arguments);
        return Finalized {
            call,
            result: crate::tool::error_tool_result_kind(
                &reason,
                yi_types::event::ToolErrorKind::InvalidArgs,
            ),
            is_error: true,
            stops_message,
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
            stops_message: false,
        };
    }
    let gates = tool.gates_message(&call.arguments);
    let ToolOutcome { result, is_error } =
        tool.execute(&call.id, call.arguments.clone(), signal).await;
    let stops_message = gates && yi_types::event::shown_failed(is_error, &result.details);
    Finalized {
        call,
        result,
        is_error,
        stops_message,
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
    let _span = yi_types::trace::span("tool.call").arg("tool", call.name.as_str());
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

pub(crate) async fn execute_tool_calls(
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
    let mut finalized: Vec<Finalized> = Vec::new();
    let mut stop_at: Option<usize> = None;
    let mut queue = calls.into_iter().peekable();
    while queue.peek().is_some() {
        // Invariant: after an interrupt every unrun call is answered `Aborted` (D245), so the
        // guard stands aside and execute_one says so.
        if !signal.is_fired()
            && let Some(why) = stop_at.and_then(|index| finalized.get(index)).map(not_run)
        {
            let rest = fail_calls(
                queue.by_ref().collect(),
                &why,
                Some(yi_types::event::ToolErrorKind::NotRun),
                emit,
            );
            finalized.extend(rest);
            break;
        }
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
            if stop_at.is_none() && item.stops_message {
                stop_at = Some(finalized.len());
            }
            finalized.push(item);
        }
    }
    let terminate = should_terminate(&finalized);
    (finalized, terminate)
}

fn not_run(failed: &Finalized) -> String {
    format!(
        "{} ({}) failed earlier in this message, and a message stops at its first failed write or command; send it again once {} succeeds",
        failed.call.name, failed.call.id, failed.call.name
    )
}

pub(crate) fn fail_calls(
    calls: Vec<ExtractedCall>,
    why: &str,
    kind: Option<yi_types::event::ToolErrorKind>,
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
            let text = format!("Tool call \"{}\" was not executed: {why}", call.name);
            let item = Finalized {
                call,
                result: match kind {
                    Some(kind) => crate::tool::error_tool_result_kind(&text, kind),
                    None => error_tool_result(&text),
                },
                is_error: true,
                stops_message: false,
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

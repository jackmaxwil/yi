//! Spans beside the session file: a request's first token and total, a tool call's duration
//! and outcome, a turn — written as each ends, read by `yi stats telemetry` (plan 2026-09-05).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::Receiver;
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::message::{AgentMessage, Usage};
use yi_types::model::Model;
use yi_types::telemetry::{ErrorClass, Span, SpanKind};

#[derive(Default)]
pub struct Telemetry {
    path: Mutex<Option<PathBuf>>,
    session: Mutex<String>,
    requests: AtomicU64,
    turns: AtomicU64,
}

fn ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn text_of(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|part| match part {
                yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// A dead stream's class is in its error message, not its text; the text is the fallback.
fn error_class(message: &AgentMessage) -> ErrorClass {
    if let AgentMessage::Assistant {
        error_message: Some(text),
        ..
    } = message
    {
        return ErrorClass::from_provider_text(text);
    }
    ErrorClass::from_provider_text(&text_of(message))
}

fn usage_of(message: &AgentMessage) -> Option<&Usage> {
    match message {
        AgentMessage::Assistant { usage, .. } => Some(usage),
        _ => None,
    }
}

fn fill_usage(span: &mut Span, usage: &Usage) {
    span.input = Some(usage.input);
    span.output = Some(usage.output);
    span.cache_read = Some(usage.cache_read);
    span.cache_write = Some(usage.cache_write);
    span.cost_usd = usage.cost.total.as_f64();
}

impl Telemetry {
    /// The sidecar sits beside the session file, named for it, so a run's collector finds it.
    pub fn bind(&self, session_file: &Path, session_id: &str) {
        if let Ok(mut path) = self.path.lock() {
            *path = Some(session_file.with_extension("telemetry.jsonl"));
        }
        if let Ok(mut session) = self.session.lock() {
            *session = session_id.to_owned();
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path.lock().ok().and_then(|path| path.clone())
    }

    fn span(&self, kind: SpanKind) -> Span {
        let session = self.session.lock().map(|s| s.clone()).unwrap_or_default();
        Span::new(kind, session)
    }

    pub fn write(&self, span: &Span) {
        use std::io::Write;
        let Some(path) = self.path() else { return };
        let Ok(line) = serde_json::to_string(span) else {
            return;
        };
        let _unwritable_span_is_lost_not_fatal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{line}"));
    }

    /// Invariant: the wrapper adds no event and drops none; it stamps the clock at the first
    /// delta and writes the request span when the stream ends.
    pub fn wrap(
        self: &Arc<Self>,
        model: &Model,
        mut inner: Receiver<AssistantMessageEvent>,
    ) -> Receiver<AssistantMessageEvent> {
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let telemetry = Arc::clone(self);
        let provider = model.provider.clone();
        let id = model.id.clone();
        let request = self
            .requests
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        tokio::spawn(async move {
            let started = Instant::now();
            let mut ttft = None;
            while let Some(event) = inner.recv().await {
                if ttft.is_none()
                    && matches!(
                        event,
                        AssistantMessageEvent::TextDelta { .. }
                            | AssistantMessageEvent::ThinkingDelta { .. }
                            | AssistantMessageEvent::ToolCallDelta { .. }
                    )
                {
                    ttft = Some(ms(started.elapsed()));
                }
                let ended = match &event {
                    AssistantMessageEvent::Done { message, .. } => Some((usage_of(message), None)),
                    AssistantMessageEvent::Error { error, .. } => {
                        Some((usage_of(error), Some(error_class(error))))
                    }
                    _ => None,
                };
                if let Some((usage, class)) = ended {
                    let mut span = telemetry.span(SpanKind::Request);
                    span.request = Some(request);
                    span.provider = Some(provider.clone());
                    span.model = Some(id.clone());
                    span.ttft_ms = ttft;
                    span.total_ms = Some(ms(started.elapsed()));
                    if let Some(usage) = usage {
                        fill_usage(&mut span, usage);
                    }
                    span.class = class.map(|class| class.to_string());
                    telemetry.write(&span);
                }
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        receiver
    }

    /// Tool and turn spans are projections of the events the session broadcasts, written at
    /// the send so a runtime that exits right after the turn still has them on disk.
    pub fn on_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let mut span = self.span(SpanKind::Tool);
                span.call = Some(tool_call_id.clone());
                span.tool = Some(tool_name.clone());
                span.ms = result
                    .details
                    .get("durationMs")
                    .and_then(serde_json::Value::as_u64);
                span.ok = Some(!is_error);
                span.class = is_error.then(|| {
                    let kind = result
                        .details
                        .get("errorKind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool_error");
                    ErrorClass::Tool(kind.to_owned()).to_string()
                });
                self.write(&span);
            }
            AgentEvent::TurnEnd {
                message,
                tool_results,
            } => {
                let turn = self.turns.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                let mut span = self.span(SpanKind::Turn);
                span.turn = Some(turn);
                if let Some(usage) = usage_of(message) {
                    fill_usage(&mut span, usage);
                }
                span.extra.insert(
                    "toolResults".to_owned(),
                    serde_json::json!(tool_results.len()),
                );
                self.write(&span);
            }
            _ => {}
        }
    }
}

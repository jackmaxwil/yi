use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use yi_context::{
    Prefill, Preparation, Scope, Settings, Tokens, Window, compose_summary, convert_to_llm,
    drop_internal, estimate_context, prepare_compaction, prompts, should_compact,
};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::run::StreamFn;
use yi_types::entry::Entry;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{LlmContext, Model};

use crate::provider::ProviderStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactStatus {
    pub tokens: u64,
    pub context_window: u64,
    pub percent: u64,
    pub scheduled: bool,
}

pub struct Compactor {
    pub settings: Settings,
    /// §12 `summarizer` role. The window math stays on the turn's own model —
    /// a cheaper summarizer with a smaller window must not make compaction
    /// look overdue.
    pub summarizer: Option<Model>,
    scope: Scope,
    window: Mutex<Window>,
    pending: AtomicBool,
    instructions: Mutex<Option<String>>,
}

fn lock_window(window: &Mutex<Window>) -> std::sync::MutexGuard<'_, Window> {
    window
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn synthesize_entries(messages: &[AgentMessage]) -> Vec<Entry> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| Entry::Message {
            id: format!("mem-{index}"),
            message: message.clone(),
            terminate: None,
            parent_id: None,
            seq: index as u64,
            timestamp: 0,
        })
        .collect()
}

async fn complete_text(
    provider: &ProviderStream,
    model: &Model,
    context: &LlmContext,
    signal: &InterruptSignal,
) -> Result<String, String> {
    let mut receiver = provider.stream(model, context, signal);
    while let Some(event) = receiver.recv().await {
        match event {
            AssistantMessageEvent::Done { message, .. } => {
                if let AgentMessage::Assistant {
                    content,
                    stop_reason,
                    error_message,
                    ..
                } = message
                {
                    if stop_reason == StopReason::Error {
                        return Err(error_message.unwrap_or_else(|| "unknown error".to_owned()));
                    }
                    let text: String = content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(text);
                }
                return Err("summarizer returned a non-assistant message".to_owned());
            }
            AssistantMessageEvent::Error { error, .. } => {
                let text = match error {
                    AgentMessage::Assistant { error_message, .. } => {
                        error_message.unwrap_or_else(|| "unknown error".to_owned())
                    }
                    _ => "unknown error".to_owned(),
                };
                return Err(text);
            }
            _ => {}
        }
    }
    Err("summarizer stream ended without a terminal event".to_owned())
}

fn directive_message(prepared: &Preparation, instructions: Option<&str>) -> AgentMessage {
    let mut text = String::new();
    if let Some(previous) = &prepared.previous_summary {
        text.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    text.push_str(&prompts::build_summarization_prompt(
        instructions,
        prepared.previous_summary.as_deref(),
    ));
    AgentMessage::User {
        content: UserContent::Text(text),
        timestamp: 0,
    }
}

impl Compactor {
    pub fn new(initial_window_id: String) -> Self {
        Self {
            settings: Settings::default(),
            summarizer: None,
            scope: Scope::BodyAfterPrefix,
            window: Mutex::new(Window::new_initial(initial_window_id)),
            pending: AtomicBool::new(false),
            instructions: Mutex::new(None),
        }
    }

    pub fn schedule(&self) {
        self.pending.store(true, Ordering::Relaxed);
    }

    /// A later call replaces earlier instructions.
    pub fn schedule_with_instructions(&self, instructions: Option<String>) {
        if let Ok(mut slot) = self.instructions.lock()
            && instructions.is_some()
        {
            *slot = instructions;
        }
        self.pending.store(true, Ordering::Relaxed);
    }

    pub fn scheduled(&self) -> bool {
        self.pending.load(Ordering::Relaxed)
    }

    /// Input-side tokens only: the reply is body, not prefix.
    pub fn on_usage(&self, usage: &Usage) {
        let mut window = lock_window(&self.window);
        if window.prefill_tokens().is_none() {
            let input_side = usage
                .input
                .saturating_add(usage.cache_read)
                .saturating_add(usage.cache_write);
            window.observe_prefill(Prefill::ServerObserved(Tokens(
                u64::try_from(input_side).unwrap_or(0),
            )));
        }
    }

    pub fn status(&self, messages: &[AgentMessage], model: &Model) -> CompactStatus {
        let tokens = estimate_context(messages).tokens;
        let context_window = model.context_window;
        CompactStatus {
            tokens: tokens.0,
            context_window,
            percent: if context_window == 0 {
                0
            } else {
                tokens.0.saturating_mul(100) / context_window
            },
            scheduled: self.pending.load(Ordering::Relaxed),
        }
    }

    fn due(&self, messages: &[AgentMessage], model: &Model) -> bool {
        if self.pending.load(Ordering::Relaxed) {
            return true;
        }
        let estimate = estimate_context(messages);
        let prefill = lock_window(&self.window).prefill_tokens();
        let scoped = yi_context::account::scoped_tokens(estimate.tokens, self.scope, prefill);
        should_compact(scoped, Tokens(model.context_window), &self.settings)
    }

    /// Prefix-aligned: the directive appends as a trailing user message, so
    /// summarization extends the warm cache. Overflow retries once with the
    /// oldest quarter trimmed; a second failure rolls the window unsummarized.
    pub async fn maybe_compact(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        system_prompt: &str,
        provider: &ProviderStream,
        store: Option<&yi_session::SharedSession>,
        signal: &InterruptSignal,
    ) -> Option<Vec<AgentMessage>> {
        if !self.settings.enabled || !self.due(messages, model) {
            return None;
        }
        self.pending.store(false, Ordering::Relaxed);
        let instructions = self
            .instructions
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let entries: Vec<Entry> = match store {
            Some(store) => yi_session::lock_session(store)
                .find_entries_on_branch(
                    "main",
                    &yi_session::EntryQuery {
                        order: yi_session::EntryOrder::OldestFirst,
                        ..yi_session::EntryQuery::default()
                    },
                    &yi_session::BranchBounds::default(),
                )
                .ok()?,
            None => synthesize_entries(messages),
        };
        let prepared = prepare_compaction(&entries, &self.settings)?;
        let request = |window_messages: &[AgentMessage]| LlmContext {
            system_prompt: system_prompt.to_owned(),
            messages: {
                let mut converted = convert_to_llm(window_messages);
                converted.push(directive_message(&prepared, instructions.as_deref()));
                converted
            },
            tools: None,
        };
        let summarizer = self.summarizer.as_ref().unwrap_or(model);
        let summary = match complete_text(provider, summarizer, &request(messages), signal).await {
            Ok(text) => Ok(text),
            Err(_) => {
                let trimmed = &messages[messages.len() / 4..];
                complete_text(provider, summarizer, &request(trimmed), signal).await
            }
        };
        let summary_text = summary.unwrap_or_default();
        let (composed, mut details) = compose_summary(&summary_text, &prepared.file_ops);
        let retained_tail = drop_internal(&prepared.retained_tail);
        let new_window_id = match store {
            Some(store) => yi_session::lock_session(store).next_id(),
            None => format!("win-{}", yi_session::now_ms()),
        };
        details.window = Some(lock_window(&self.window).advance(new_window_id));
        let timestamp = yi_session::now_ms();
        if let Some(store) = store {
            let details_value = serde_json::to_value(&details).ok();
            let _ = yi_session::lock_session(store).append_compaction(
                "main",
                composed.clone(),
                retained_tail.clone(),
                prepared.tokens_before.0,
                details_value,
            );
        }
        let mut replacement = Vec::with_capacity(retained_tail.len().saturating_add(1));
        replacement.push(AgentMessage::CompactionSummary {
            summary: composed,
            tokens_before: prepared.tokens_before.0,
            timestamp,
        });
        replacement.extend(retained_tail);
        Some(replacement)
    }
}

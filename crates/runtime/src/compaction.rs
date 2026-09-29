use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use yi_context::{
    Prefill, Preparation, Scope, Settings, Tokens, Window, compose_summary, convert_to_llm,
    drop_internal, estimate_context, prepare_compaction, prompts, should_compact,
};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::run::StreamFn;
use yi_types::entry::Entry;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{Effort, LlmContext, Model, Reuse, ToolDef};

use crate::provider::ProviderStream;

pub type CompactFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<AgentMessage>>> + Send>>;
pub type CompactHook = Box<dyn Fn(&[AgentMessage]) -> CompactFuture + Send + Sync>;
pub type StoreOf = Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>;
pub type WaitFn = dyn Fn(Option<yi_types::event::Wait>) + Send + Sync;

#[derive(Clone)]
pub struct CompactReports {
    pub waiting: Arc<WaitFn>,
    pub compacted: Arc<dyn Fn() + Send + Sync>,
    pub unsaved: Arc<dyn Fn(&yi_session::SessionError) + Send + Sync>,
}

struct Closing(Arc<WaitFn>);

impl Drop for Closing {
    fn drop(&mut self) {
        (self.0)(None);
    }
}

/// What the loop sends beside its messages (its tool choice past the first request is none): a
/// compaction on the session's model sends the same, so its prefix is a cache read (design §7).
#[derive(Clone, Debug)]
pub struct LoopRequest {
    pub system_prompt: String,
    pub tools: Option<Vec<ToolDef>>,
    pub effort: Effort,
}

impl LoopRequest {
    /// The loop's own spelling: an empty table rides as no table.
    pub fn new(
        system_prompt: String,
        tools: &[Arc<dyn yi_loop::AgentTool>],
        effort: Effort,
    ) -> Self {
        let tools: Vec<ToolDef> = tools.iter().map(|tool| tool.definition()).collect();
        Self {
            system_prompt,
            tools: (!tools.is_empty()).then_some(tools),
            effort,
        }
    }
}

/// Built only when a compaction is due: a run's tool table is not free to render.
pub type LoopRequestOf = Arc<dyn Fn() -> LoopRequest + Send + Sync>;

pub fn loop_hook(
    compactor: Arc<Compactor>,
    provider: Arc<ProviderStream>,
    model: Model,
    request: LoopRequestOf,
    store: StoreOf,
    reports: CompactReports,
) -> CompactHook {
    Box::new(move |messages: &[AgentMessage]| {
        let _span = yi_types::trace::span("compact.hook");
        if !compactor.wanted(messages, &model) {
            return Box::pin(std::future::ready(None));
        }
        let compactor = Arc::clone(&compactor);
        let provider = Arc::clone(&provider);
        let model = model.clone();
        let request = request();
        let store = store();
        let CompactReports {
            waiting,
            compacted,
            unsaved,
        } = reports.clone();
        let messages = messages.to_vec();
        Box::pin(async move {
            let signal = InterruptSignal::default();
            let due = compactor.wanted(&messages, &model);
            let closing = due.then(|| {
                let tokens = estimate_context(&messages).tokens.0;
                waiting(Some(yi_types::event::Wait::Compaction { tokens }));
                Closing(Arc::clone(&waiting))
            });
            let replaced = compactor
                .maybe_compact(
                    &messages,
                    &model,
                    &request,
                    provider.as_ref(),
                    store.as_ref(),
                    &signal,
                )
                .await;
            drop(closing);
            match replaced {
                Ok(replaced) => {
                    if replaced.is_some() {
                        compacted();
                    }
                    replaced
                }
                Err(error) => {
                    unsaved(&error);
                    None
                }
            }
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactStatus {
    pub tokens: u64,
    pub context_window: u64,
    pub percent: u64,
    pub scheduled: bool,
}

pub struct Compactor {
    pub settings: Settings,
    /// §5 `summarizer` role. The window math stays on the turn's own model: a cheaper
    /// summarizer with a smaller window must not make compaction look overdue.
    pub summarizer: Option<Model>,
    scope: Scope,
    window: Mutex<Window>,
    pending: AtomicBool,
    running: AtomicBool,
    /// Invariant: set while the last entry failed to write; only `/compact` spends the summarizer.
    unsaved: AtomicBool,
    instructions: Mutex<Option<String>>,
    standing: Mutex<Option<Standing>>,
}

struct Raised<'a>(&'a AtomicBool);

impl<'a> Raised<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self(flag)
    }
}

impl Drop for Raised<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Read at every compaction rather than stored, so a directive derived from
/// live state cannot go stale between the schedule and the summarizer call.
pub type Standing = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

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

pub(crate) async fn complete_text(
    provider: &ProviderStream,
    model: &Model,
    context: &LlmContext,
    effort: Effort,
    signal: &InterruptSignal,
) -> Result<String, String> {
    let mut receiver = provider.stream(model, context, effort, signal);
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

fn merge(standing: Option<String>, once: Option<String>) -> Option<String> {
    match (standing, once) {
        (Some(standing), Some(once)) => Some(format!("{standing}\n\n{once}")),
        (standing, once) => standing.or(once),
    }
}

fn directive_message(
    prepared: &Preparation,
    instructions: Option<&str>,
    key: &str,
) -> AgentMessage {
    let mut text = key.to_owned();
    if let Some(previous) = &prepared.previous_summary {
        text.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    text.push_str(&prompts::build_summarization_prompt(
        instructions,
        prepared.previous_summary.as_deref(),
    ));
    AgentMessage::host_user(UserContent::Text(text), 0)
}

impl Compactor {
    pub fn new(initial_window_id: String) -> Self {
        Self {
            settings: Settings::default(),
            summarizer: None,
            scope: Scope::BodyAfterPrefix,
            window: Mutex::new(Window::new_initial(initial_window_id)),
            pending: AtomicBool::new(false),
            running: AtomicBool::new(false),
            unsaved: AtomicBool::new(false),
            instructions: Mutex::new(None),
            standing: Mutex::new(None),
        }
    }

    /// A directive that rides every compaction, ahead of whatever `/compact`
    /// asked for once. The caller's own words win, so this leads.
    pub fn set_standing(&self, standing: Standing) {
        if let Ok(mut slot) = self.standing.lock() {
            *slot = Some(standing);
        }
    }

    pub fn standing_directive(&self) -> Option<String> {
        self.standing
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .and_then(|read| read())
    }

    /// What the next compaction would tell the summarizer, without spending the
    /// one-shot half — the same merge that compaction itself applies.
    pub fn pending_directive(&self) -> Option<String> {
        let once = self.instructions.lock().ok().and_then(|slot| slot.clone());
        merge(self.standing_directive(), once)
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

    pub fn compacting(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Input-side tokens only: the reply is body, not prefix.
    pub fn on_usage(&self, usage: &Usage) {
        // Invariant: a `ServerObserved` prefill latches for the whole window, so an unreported
        // usage recorded as zero would pin it there and drop every later observation.
        if usage.unknown {
            return;
        }
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

    fn wanted(&self, messages: &[AgentMessage], model: &Model) -> bool {
        self.settings.enabled && self.due(messages, model)
    }

    fn due(&self, messages: &[AgentMessage], model: &Model) -> bool {
        if self.pending.load(Ordering::Relaxed) {
            return true;
        }
        if self.unsaved.load(Ordering::Relaxed) {
            return false;
        }
        let estimate = estimate_context(messages);
        let prefill = lock_window(&self.window).prefill_tokens();
        let scoped = yi_context::account::scoped_tokens(estimate.tokens, self.scope, prefill);
        should_compact(scoped, Tokens(model.context_window), &self.settings)
    }

    /// On the session's model this is the loop's request plus the directive, a cache read (§7);
    /// another summarizer sends no tools or thinking. Overflow retries once trimmed, then rolls.
    pub async fn maybe_compact(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        loop_request: &LoopRequest,
        provider: &ProviderStream,
        store: Option<&yi_session::SharedSession>,
        signal: &InterruptSignal,
    ) -> Result<Option<Vec<AgentMessage>>, yi_session::SessionError> {
        if !self.wanted(messages, model) {
            return Ok(None);
        }
        self.pending.store(false, Ordering::Relaxed);
        let _running = Raised::new(&self.running);
        let once = self
            .instructions
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let instructions = merge(self.standing_directive(), once);
        let entries: Option<Vec<Entry>> = match store {
            Some(store) => yi_session::lock_session(store)
                .find_entries_on_branch(
                    "main",
                    &yi_session::EntryQuery {
                        order: yi_session::EntryOrder::OldestFirst,
                        ..yi_session::EntryQuery::default()
                    },
                    &yi_session::BranchBounds::default(),
                )
                .ok(),
            None => Some(synthesize_entries(messages)),
        };
        let prepared = entries.and_then(|entries| prepare_compaction(&entries, &self.settings));
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        let inputs = store.and_then(|store| crate::fetch::user_inputs(store).ok());
        let summarizer = self.summarizer.as_ref().unwrap_or(model);
        let warm = summarizer == model;
        let request = |window_messages: &[AgentMessage], warm: bool| LlmContext {
            cache_ttl: yi_types::model::Ttl::Min5,
            system_prompt: loop_request.system_prompt.clone(),
            messages: {
                let mut converted = convert_to_llm(window_messages);
                let key = yi_context::user_key(window_messages, inputs.as_deref().unwrap_or(&[]));
                converted.push(directive_message(&prepared, instructions.as_deref(), &key));
                converted
            },
            transient: Vec::new(),
            schema: None,
            shared_through: None,
            reuse: if warm {
                Reuse::ReadOnly
            } else {
                Reuse::OneShot
            },
            tools: loop_request.tools.clone().filter(|_| warm),
            tool_choice: None,
        };
        let effort = |warm: bool| {
            if warm {
                loop_request.effort
            } else {
                summarizer.clamp_effort(Effort::Off)
            }
        };
        let first = request(messages, warm);
        let summary_text =
            match complete_text(provider, summarizer, &first, effort(warm), signal).await {
                // With the loop's tools attached a reply can be a call alone: no summary in it.
                Ok(text) if !text.trim().is_empty() => text,
                // The trimmed retry shares no prefix, so it goes cold: no tools, so it must be text.
                _ => {
                    let trimmed = request(&messages[messages.len() / 4..], false);
                    complete_text(provider, summarizer, &trimmed, effort(false), signal)
                        .await
                        .unwrap_or_default()
                }
            };
        let (composed, mut details) =
            compose_summary(&summary_text, &prepared.file_ops, &prepared.view);
        let retained_tail = drop_internal(&prepared.retained_tail);
        let new_window_id = match store {
            Some(store) => yi_session::lock_session(store).next_id(),
            None => format!("win-{}", yi_session::now_ms()),
        };
        let mut next = lock_window(&self.window).clone();
        details.window = Some(next.advance(new_window_id));
        let timestamp = yi_session::now_ms();
        if let Some(store) = store {
            let details_value = serde_json::to_value(&details).ok();
            let saved = yi_session::lock_session(store).append_compaction(
                "main",
                composed.clone(),
                retained_tail.clone(),
                prepared.tokens_before.0,
                details_value,
            );
            self.unsaved.store(saved.is_err(), Ordering::Relaxed);
            saved?;
        }
        *lock_window(&self.window) = next;
        let mut replacement = Vec::with_capacity(retained_tail.len().saturating_add(1));
        replacement.push(AgentMessage::CompactionSummary {
            summary: composed,
            tokens_before: prepared.tokens_before.0,
            timestamp,
        });
        replacement.extend(retained_tail);
        Ok(Some(replacement))
    }
}

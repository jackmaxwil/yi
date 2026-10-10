use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use yi_context::{
    Preparation, Settings, Tokens, Window, compose_summary, convert_to_llm, drop_internal,
    estimate_context, estimate_message, is_cut_point, prepare_compaction, prompts, reply_tokens,
    serialize_conversation, should_compact,
};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::run::StreamFn;
use yi_types::entry::Entry;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason, Usage};
use yi_types::model::{Effort, LlmContext, Model, Reuse, ToolDef};

use crate::provider::ProviderStream;

pub type CompactFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<AgentMessage>>> + Send>>;
pub type CompactHook = Box<dyn Fn(&[AgentMessage], &Model, Effort) -> CompactFuture + Send + Sync>;
pub type StoreOf = Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync>;
pub type WaitFn = dyn Fn(Option<yi_types::event::Wait>) + Send + Sync;

#[derive(Clone)]
pub struct CompactReports {
    pub waiting: Arc<WaitFn>,
    pub compacted: Arc<dyn Fn() + Send + Sync>,
    pub failed: Arc<dyn Fn(&CompactError) + Send + Sync>,
    pub elided: Arc<dyn Fn(&Elision) + Send + Sync>,
}

/// What a compaction puts in place of the history.
pub struct Replacement {
    pub messages: Vec<AgentMessage>,
    /// Set when no summary came and the oldest context left the view instead (D337).
    pub elision: Option<Elision>,
}

/// How many messages `Compactor::elide` took out of the model's view (notes aside).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elision {
    pub messages: usize,
}

/// One text for the user's notice, the summary's line and the marker where the messages sat.
impl std::fmt::Display for Elision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plural = if self.messages == 1 { "" } else { "s" };
        write!(
            formatter,
            "[compaction elided {} earlier message{plural} from the model's view: summary failed; \
             the transcript keeps them]",
            self.messages
        )
    }
}

/// What `AgentSession::compact_now` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactOutcome {
    NotApplied,
    Summarized,
    Elided(Elision),
}

impl CompactOutcome {
    pub fn applied(&self) -> bool {
        *self != Self::NotApplied
    }
}

/// Bytes/3, not chars/4: code and JSON run denser, and a count that runs short overflows.
fn wide(tokens: Tokens) -> Tokens {
    Tokens(tokens.0.saturating_mul(4).div_ceil(3))
}

fn text_tokens(text: &str) -> Tokens {
    Tokens(u64::try_from(text.len()).unwrap_or(u64::MAX).div_ceil(3))
}

/// A summary's own words, without the view and file lists the new summary renders again.
fn carried(summary: &str) -> &str {
    let body = summary
        .rsplit_once("</yi_compact_view>")
        .map_or(summary, |(_, rest)| rest);
    let end = ["<read-files>", "<modified-files>"]
        .iter()
        .filter_map(|tag| body.find(tag))
        .min()
        .unwrap_or(body.len());
    body[..end].trim()
}

/// The earliest cut point from which the rest of `messages` fits `budget`.
fn fitting_start(messages: &[AgentMessage], budget: Tokens) -> Option<usize> {
    let (mut kept, mut found) = (Tokens(0), None);
    for (index, message) in messages.iter().enumerate().rev() {
        kept = kept.saturating_add(wide(estimate_message(message)));
        if kept > budget {
            break;
        }
        if is_cut_point(message) {
            found = Some(index);
        }
    }
    found
}

/// The custom type of every compaction notice and marker: a note, never a turn of its own.
pub const COMPACTION_NOTICE: &str = "compaction_notice";

/// Why a due compaction left the history as it was; the text is the notice the model reads.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("compaction not saved: {0}; history left uncompacted, /compact retries")]
    Unsaved(#[from] yi_session::SessionError),
    #[error("compaction failed: {0}; history left uncompacted, the next prompt retries")]
    NoSummary(String),
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

/// Built only when a compaction is due: a run's tool table is not free to render. Takes the
/// effort of the loop's last request, which the loop hands the hook.
pub type LoopRequestOf = Arc<dyn Fn(Effort) -> LoopRequest + Send + Sync>;

pub fn loop_hook(
    compactor: Arc<Compactor>,
    provider: Arc<ProviderStream>,
    request: LoopRequestOf,
    store: StoreOf,
    reports: CompactReports,
) -> CompactHook {
    // One failed compaction per run: the next prompt's run tries again (#946 F1).
    let gave_up = Arc::new(AtomicBool::new(false));
    let hook = move |messages: &[AgentMessage], model: &Model, effort: Effort| -> CompactFuture {
        let _span = yi_types::trace::span("compact.hook");
        if gave_up.load(Ordering::Relaxed) || !compactor.wanted(messages, model) {
            return Box::pin(std::future::ready(None));
        }
        let gave_up = Arc::clone(&gave_up);
        let compactor = Arc::clone(&compactor);
        let provider = Arc::clone(&provider);
        let model = model.clone();
        let request = request(effort);
        let store = store();
        let CompactReports {
            waiting,
            compacted,
            failed,
            elided,
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
                Ok(Some(replaced)) => {
                    if let Some(elision) = &replaced.elision {
                        elided(elision);
                    }
                    compacted();
                    Some(replaced.messages)
                }
                Ok(None) => None,
                Err(error) => {
                    gave_up.store(true, Ordering::Relaxed);
                    failed(&error);
                    None
                }
            }
        })
    };
    Box::new(hook)
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
    window: Mutex<Window>,
    pending: AtomicBool,
    running: AtomicBool,
    /// Invariant: set while the last entry failed to write; only `/compact` spends the summarizer.
    unsaved: AtomicBool,
    instructions: Mutex<Option<String>>,
    standing: Mutex<Option<Standing>>,
    /// The message that opened the latest run, which a rescue keeps (D337).
    opening: Mutex<Option<AgentMessage>>,
    /// The newest reply with a usage that the latest compaction kept: its count is the history
    /// before that compaction, so `due` does not read it (D338).
    stale: Mutex<Option<AgentMessage>>,
    /// `compaction.at` in tokens, 0 when unset: due once the request passes it, if that comes
    /// before the window's reserve.
    ceiling: AtomicU64,
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

/// Invariant: the usage is the terminal message's own, error stops included: a reply that
/// ended in an error was still billed when the provider reported tokens, so it is never dropped.
pub(crate) async fn complete_text(
    provider: &ProviderStream,
    model: &Model,
    context: &LlmContext,
    effort: Effort,
    signal: &InterruptSignal,
) -> (Result<String, String>, Usage) {
    reply_of(provider.stream(model, context, effort, signal)).await
}

async fn reply_of(
    mut receiver: tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
) -> (Result<String, String>, Usage) {
    while let Some(event) = receiver.recv().await {
        let (message, failed) = match event {
            AssistantMessageEvent::Done { message, .. } => (message, false),
            AssistantMessageEvent::Error { error, .. } => (error, true),
            _ => continue,
        };
        let AgentMessage::Assistant {
            content,
            stop_reason,
            error_message,
            usage,
            ..
        } = message
        else {
            return (
                Err("summarizer returned a non-assistant message".to_owned()),
                Usage::unknown(),
            );
        };
        let text = if failed || stop_reason == StopReason::Error {
            Err(error_message.unwrap_or_else(|| "unknown error".to_owned()))
        } else {
            Ok(yi_types::message::join_text(&content, "\n"))
        };
        return (text, usage);
    }
    (
        Err("summarizer stream ended without a terminal event".to_owned()),
        Usage::unknown(),
    )
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
    AgentMessage::task(&text, 0)
}

impl Compactor {
    pub fn new(initial_window_id: String) -> Self {
        Self {
            settings: Settings::default(),
            summarizer: None,
            window: Mutex::new(Window::new_initial(initial_window_id)),
            pending: AtomicBool::new(false),
            running: AtomicBool::new(false),
            unsaved: AtomicBool::new(false),
            instructions: Mutex::new(None),
            standing: Mutex::new(None),
            opening: Mutex::new(None),
            stale: Mutex::new(None),
            ceiling: AtomicU64::new(0),
        }
    }

    /// The reason when `tokens` is under what a compaction keeps and was raised to it.
    pub fn set_ceiling(&self, tokens: Option<u64>) -> Option<String> {
        let tokens = tokens.unwrap_or(0);
        // Invariant: a compaction keeps `keep_recent` plus the prefix and summary, which a second
        // reserve covers; a ceiling under that would compact again on every request.
        let reserve = self.settings.reserve_tokens.0;
        let floor = self
            .settings
            .keep_recent_tokens
            .0
            .saturating_add(reserve.saturating_mul(2));
        let at = if tokens == 0 { 0 } else { tokens.max(floor) };
        self.ceiling.store(at, Ordering::Relaxed);
        (at > tokens && tokens > 0).then(|| {
            format!("compaction.at {tokens} is under the {floor} tokens a compaction keeps; using {floor}")
        })
    }

    /// Rebuilds the window chain and the stale reply from the latest compaction entry, which
    /// stores both its details and the kept tail; resume would otherwise compact once more.
    /// A branch with none (a rewind past the compaction) goes back to the initial window.
    pub fn resume(&self, entries: &[yi_types::entry::Entry]) {
        let latest = entries.iter().rev().find_map(|entry| match entry {
            yi_types::entry::Entry::Compaction {
                retained_tail,
                details,
                ..
            } => Some((retained_tail, details)),
            _ => None,
        });
        let mut window = lock_window(&self.window);
        match latest {
            None => *window = Window::new_initial(window.ids().first),
            Some((_, details)) => {
                let ids = details
                    .as_ref()
                    .and_then(|value| {
                        serde_json::from_value::<yi_types::compaction::CompactionDetails>(
                            value.clone(),
                        )
                        .ok()
                    })
                    .and_then(|details| details.window);
                if let Some(ids) = ids {
                    *window = Window::restore(ids.number, ids);
                }
            }
        }
        drop(window);
        if let Ok(mut slot) = self.stale.lock() {
            *slot = latest.and_then(|(tail, _)| {
                tail.iter()
                    .rfind(|message| reply_tokens(message).is_some())
                    .cloned()
            });
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

    /// Recorded at a run's start, whoever sent it: a typed prompt, a child's brief, a wake.
    pub fn open_run(&self, prompt: &AgentMessage) {
        if let Ok(mut slot) = self.opening.lock() {
            *slot = Some(prompt.clone());
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

    pub fn compacting(&self) -> bool {
        self.running.load(Ordering::Relaxed)
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
        // The whole request, prefix included, leaves the reserve for the summary request (#947):
        // the provider's count, then bytes/3 as the rescue charges it.
        let estimate = estimate_context(messages);
        let stale = self.stale.lock().ok().and_then(|slot| slot.clone());
        let counted = estimate
            .last_usage_index
            .is_some_and(|index| Some(&messages[index]) != stale.as_ref());
        let sent = if counted {
            estimate
                .usage_tokens
                .saturating_add(wide(estimate.trailing_tokens))
        } else {
            wide(
                messages
                    .iter()
                    .map(estimate_message)
                    .fold(Tokens(0), Tokens::saturating_add),
            )
        };
        let window = match self.ceiling.load(Ordering::Relaxed) {
            0 => model.context_window,
            at => {
                let capped = at.saturating_add(self.settings.reserve_tokens.0);
                match model.context_window {
                    0 => capped,
                    window => window.min(capped),
                }
            }
        };
        should_compact(sent, Tokens(window), &self.settings)
    }

    /// No summary, no room (D337): keep the first user message, the work's opening and the earlier
    /// summary; fill the rest with the newest messages from a cut point, charged at bytes/3.
    fn elide(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        loop_request: &LoopRequest,
        prepared: &Preparation,
    ) -> Option<(String, Vec<AgentMessage>, Elision)> {
        let window = Tokens(model.context_window);
        if estimate_context(messages).tokens < window {
            return None;
        }
        let (start, earlier) = match messages.first() {
            Some(AgentMessage::CompactionSummary { summary, .. }) => (1, carried(summary)),
            _ => (0, ""),
        };
        let opening = self.opening.lock().ok().and_then(|slot| slot.clone());
        let first = messages[start..]
            .iter()
            .position(|message| matches!(message, AgentMessage::User { .. }))
            .map(|offset| start.saturating_add(offset));
        let run = opening
            .and_then(|opening| messages.iter().rposition(|message| *message == opening))
            .filter(|run| Some(*run) != first);
        let tools = loop_request
            .tools
            .as_ref()
            .and_then(|tools| serde_json::to_string(tools).ok())
            .unwrap_or_default();
        // The marker rides twice: in the summary and where the dropped messages sat.
        let line = format!("[{}]", "#".repeat(96));
        let widest = format!("{earlier}\n\n{line}");
        let (summary_room, _) = compose_summary(&widest, &prepared.file_ops, &prepared.view);
        let cost = |index: usize| wide(estimate_message(&messages[index]));
        let room = [&loop_request.system_prompt, &tools, &summary_room, &line]
            .into_iter()
            .map(|text| text_tokens(text))
            .chain(first.map(cost))
            .fold(
                window.saturating_sub(self.settings.reserve_tokens),
                Tokens::saturating_sub,
            );
        let floor = first.map_or(start, |first| first.saturating_add(1));
        let fit = |room: Tokens| {
            fitting_start(&messages[floor..], room)
                .map_or(messages.len(), |offset| floor.saturating_add(offset))
        };
        // The run's opening rides in the kept tail when it fits, else it is kept ahead of it.
        let mut cut = fit(room);
        let run = run.filter(|run| *run < cut);
        if let Some(run) = run {
            cut = fit(room.saturating_sub(cost(run)));
        }
        let mut pinned: Vec<usize> = first.into_iter().chain(run).collect();
        pinned.sort_unstable();
        let elision = Elision {
            messages: (start..cut)
                .filter(|index| !pinned.contains(index))
                .filter(|index| !matches!(messages[*index], AgentMessage::Custom { .. }))
                .count(),
        };
        if elision.messages == 0 {
            return None;
        }
        let marker = elision.to_string();
        let mut tail: Vec<AgentMessage> = pinned
            .iter()
            .map(|index| messages[*index].clone())
            .collect();
        tail.push(AgentMessage::host_note(
            COMPACTION_NOTICE,
            marker.clone(),
            0,
        ));
        tail.extend_from_slice(&messages[cut..]);
        let body = if earlier.is_empty() {
            marker
        } else {
            format!("{earlier}\n\n{marker}")
        };
        let (summary, _) = compose_summary(&body, &prepared.file_ops, &prepared.view);
        Some((summary, drop_internal(&tail), elision))
    }

    /// The loop's request plus the directive, a cache read on its model (§7). A failed summary
    /// retries once trimmed and cold, then keeps the history (#871) unless `elide` must make room.
    pub async fn maybe_compact(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        loop_request: &LoopRequest,
        provider: &ProviderStream,
        store: Option<&yi_session::SharedSession>,
        signal: &InterruptSignal,
    ) -> Result<Option<Replacement>, CompactError> {
        if !self.wanted(messages, model) {
            return Ok(None);
        }
        let scheduled = self.pending.swap(false, Ordering::Relaxed);
        let _running = Raised::new(&self.running);
        let once = self
            .instructions
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let instructions = merge(self.standing_directive(), once.clone());
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
        let findings = entries
            .as_deref()
            .map(crate::annotate::findings)
            .unwrap_or_default();
        let prepared = entries.and_then(|entries| prepare_compaction(&entries, &self.settings));
        let Some(mut prepared) = prepared else {
            return Ok(None);
        };
        prepared.view.findings = findings;
        let inputs = store.and_then(|store| crate::fetch::user_inputs(store).ok());
        let summarizer = self.summarizer.as_ref().unwrap_or(model);
        let warm = summarizer == model;
        let request = |window_messages: &[AgentMessage], warm: bool| LlmContext {
            cache_ttl: yi_types::model::Ttl::Min5,
            system_prompt: loop_request.system_prompt.clone(),
            messages: {
                let mut converted = convert_to_llm(window_messages);
                // A cold attempt sends no tools, and Anthropic refuses tool blocks without them
                // (#948): it reads the history as text.
                if !warm {
                    let text = serialize_conversation(&converted);
                    converted = vec![AgentMessage::task(&text, 0)];
                }
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
        let summarize = |context: LlmContext, warm: bool| async move {
            let (reply, usage) =
                complete_text(provider, summarizer, &context, effort(warm), signal).await;
            if let Some(store) = store {
                // A lost cost row must not undo a compaction that can still apply.
                crate::spend::book_side_call(store, "side:compact", usage).ok();
            }
            match reply {
                // With the loop's tools attached a reply can be a call alone: no summary in it.
                Ok(text) if text.trim().is_empty() => {
                    Err("the summarizer returned no text".to_owned())
                }
                outcome => outcome,
            }
        };
        // Past the window the warm request cannot go out: two cold tries, a quarter then half
        // trimmed. Else warm, then a cold quarter-trimmed retry (no tools, so it must be text).
        let over = estimate_context(messages).tokens >= Tokens(model.context_window);
        let len = messages.len();
        let attempts = if over {
            [(len / 4, false), (len / 2, false)]
        } else {
            [(0, warm), (len / 4, false)]
        };
        let mut summary = Err(String::new());
        for (skip, warm) in attempts {
            // Never open on a tool result, which no provider takes without its call.
            let skip = messages[skip..]
                .iter()
                .position(is_cut_point)
                .map_or(len, |offset| skip.saturating_add(offset));
            summary = summarize(request(&messages[skip..], warm), warm).await;
            if summary.is_ok() {
                break;
            }
        }
        let (composed, mut details, retained_tail, elision) = match summary {
            Ok(text) => {
                let (composed, details) =
                    compose_summary(&text, &prepared.file_ops, &prepared.view);
                (
                    composed,
                    details,
                    drop_internal(&prepared.retained_tail),
                    None,
                )
            }
            Err(reason) => match self.elide(messages, model, loop_request, &prepared) {
                Some((summary, tail, elision)) => {
                    let (_, details) = compose_summary("", &prepared.file_ops, &prepared.view);
                    (summary, details, tail, Some(elision))
                }
                None => {
                    // The asked-for compaction and its instructions wait for the retry.
                    self.pending.fetch_or(scheduled, Ordering::Relaxed);
                    if let Ok(mut slot) = self.instructions.lock()
                        && slot.is_none()
                    {
                        *slot = once;
                    }
                    return Err(CompactError::NoSummary(reason));
                }
            },
        };
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
        if let Ok(mut slot) = self.stale.lock() {
            *slot = retained_tail
                .iter()
                .rfind(|message| reply_tokens(message).is_some())
                .cloned();
        }
        let mut replacement = Vec::with_capacity(retained_tail.len().saturating_add(1));
        replacement.push(AgentMessage::CompactionSummary {
            summary: composed,
            tokens_before: prepared.tokens_before.0,
            timestamp,
        });
        replacement.extend(retained_tail);
        Ok(Some(Replacement {
            messages: replacement,
            elision,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::message::Content;

    /// A stream cut before its terminal event billed something nobody can read: the row it books
    /// must say "unknown", and a zero would leave the call off the books.
    #[tokio::test]
    async fn a_stream_cut_before_its_terminal_event_reports_unknown_usage() {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let partial = yi_ai::faux::faux_assistant_message(
            vec![Content::Text {
                text: "## Goal\nhalf a summ".to_owned(),
                text_signature: None,
            }],
            StopReason::Pending,
        );
        let _ = sender.send(AssistantMessageEvent::Start { partial }).await;
        drop(sender);
        let (reply, usage) = reply_of(receiver).await;
        assert!(reply.is_err());
        assert!(usage.unknown, "{usage:?}");
    }
}

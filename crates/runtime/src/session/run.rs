use std::collections::VecDeque;
use std::sync::Arc;

use yi_loop::{LoopConfig, LoopContext, run_loop};
use yi_types::event::AgentEvent;
use yi_types::mail::Delivery;
use yi_types::message::{AgentMessage, Attribution, StopReason};

use super::{
    RunParts, SessionError, Shared, Status, dispatch_ext, extensions_of, hooks, persist_message,
    record_store_error, store_of,
};

const TURN_CAP_WORD: &str = "[turns] No more tool calls: answer now from what you have, and name what is missing if it does not decide the question.";
const LAST_WORD: &str = "[deadline] Time is up: no more tool calls. Write your final answer now from what you have, and say plainly what is unfinished.";

/// Invariant: asked under the session's status lock when a turn would present the message, so
/// it may lock a host's roster but nothing that waits on this session; false drops it unread.
pub type StillNews = Arc<dyn Fn() -> bool + Send + Sync>;
pub type JobReportFn = dyn Fn(Vec<(String, StillNews)>) -> bool + Send + Sync;

pub(super) struct Queued {
    pub(super) message: AgentMessage,
    wakes: bool,
    news: Option<StillNews>,
}

impl Queued {
    pub(super) fn new(message: AgentMessage, wakes: bool, news: Option<StillNews>) -> Self {
        Self {
            message,
            wakes,
            news,
        }
    }

    fn current(&self) -> bool {
        self.news.as_ref().is_none_or(|news| news())
    }
}

/// Invariant: an envelope is queued once; a sender's newer progress replaces its queued one.
pub(super) fn push(queue: &mut VecDeque<Queued>, entry: Queued) {
    if let Some(from) = crate::mail::progress_from(&entry.message) {
        queue.retain(|held| crate::mail::progress_from(&held.message) != Some(from));
    }
    let id = crate::mail::envelope_id(&entry.message);
    let queued = |held: &Queued| crate::mail::envelope_id(&held.message) == id;
    if id.is_none() || !queue.iter().any(queued) {
        queue.push_back(entry);
    }
}

pub(super) fn take_mail(queue: &mut VecDeque<Queued>) -> Vec<AgentMessage> {
    let (mail, rest) = std::mem::take(queue)
        .into_iter()
        .partition(|queued: &Queued| crate::mail::envelope_id(&queued.message).is_some());
    *queue = rest;
    mail.into_iter().map(|queued| queued.message).collect()
}

pub(super) fn drain(shared: &Shared) -> Vec<AgentMessage> {
    let taken = shared
        .steer
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default();
    taken
        .into_iter()
        .filter(Queued::current)
        .map(|queued| queued.message)
        .collect()
}

/// Matched once admitted: at intake a prompt may be refused as busy, and a pointer would wake it.
fn pointers_for(shared: &Shared, message: &AgentMessage) -> Vec<AgentMessage> {
    let rules = shared.rules.lock().ok().and_then(|slot| slot.clone());
    rules.map_or_else(Vec::new, |rules| rules.observe_user(message))
}

fn with_pointers(shared: &Shared, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let mut placed = Vec::with_capacity(messages.len());
    for message in messages {
        let pointers = pointers_for(shared, &message);
        placed.push(message);
        placed.extend(pointers);
    }
    placed
}

/// The prompt a waking entry or a follow-up is owed, taken under the status lock enqueue takes.
/// `ended_clean` is true only where a run settles undisturbed: only then is a user steer owed a turn.
fn owed(shared: &Shared, follow_ups: bool, ended_clean: bool) -> Option<AgentMessage> {
    if shared.winding_down() || shared.held.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    if let Ok(mut queue) = shared.steer.lock() {
        queue.retain(Queued::current);
        // Incident: a user steer sent after the loop's last read waited behind the next prompt.
        let waking = |queued: &Queued| {
            queued.wakes || (ended_clean && queued.message.attribution() == Attribution::User)
        };
        if queue.iter().any(waking) {
            return queue.pop_front().map(|queued| queued.message);
        }
    }
    let mut follow = shared.follow_up.lock().ok()?;
    follow.retain(Queued::current);
    (follow_ups && !follow.is_empty()).then(|| follow.remove(0).message)
}

/// The answer is what happened, decided under the lock a run's end takes to go idle.
pub(super) fn enqueue(parts: &RunParts, entry: Queued) -> Delivery {
    let Ok(mut status) = parts.shared.status.lock() else {
        return Delivery::Inboxed;
    };
    let wakes = entry.wakes && entry.current();
    if let Ok(mut queue) = parts.shared.steer.lock() {
        push(&mut queue, entry);
    }
    parts.shared.mail.notify_waiters();
    if *status == Status::Running {
        return Delivery::Queued;
    }
    let Some(prompt) = wakes.then(|| owed(&parts.shared, false, false)).flatten() else {
        return Delivery::Inboxed;
    };
    admit(&parts.shared, &mut status);
    drop(status);
    launch(parts.clone(), prompt, None);
    Delivery::Woken
}

pub(super) fn kick(parts: &RunParts) {
    let Ok(mut status) = parts.shared.status.lock() else {
        return;
    };
    if *status == Status::Running {
        return;
    }
    if let Some(prompt) = owed(&parts.shared, true, false) {
        admit(&parts.shared, &mut status);
        drop(status);
        launch(parts.clone(), prompt, None);
    }
}

pub(super) fn failed_compaction(parts: &RunParts, error: &crate::compaction::CompactError) {
    if let crate::compaction::CompactError::Unsaved(error) = error {
        record_store_error(&parts.shared, error);
    }
    queue_compaction_notice(parts, &format!("[{error}]"));
}

/// A newer compaction notice replaces one not yet read, so they never pile up in the queue.
pub(super) fn queue_compaction_notice(parts: &RunParts, notice: &str) {
    if let Ok(mut queue) = parts.shared.steer.lock() {
        queue.retain(|held| !compaction_notice(&held.message));
    }
    let note = AgentMessage::host_note(crate::compaction::COMPACTION_NOTICE, notice.to_owned(), 0);
    enqueue(parts, Queued::new(note, false, None));
}

fn compaction_notice(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::Custom { custom_type, .. } if custom_type == crate::compaction::COMPACTION_NOTICE)
}

pub(super) fn follow(parts: &RunParts, message: AgentMessage, news: Option<StillNews>) -> bool {
    let Ok(mut status) = parts.shared.status.lock() else {
        return false;
    };
    if let Ok(mut queue) = parts.shared.follow_up.lock() {
        queue.push(Queued::new(message, false, news));
    }
    if *status == Status::Running {
        return true;
    }
    if let Some(prompt) = owed(&parts.shared, true, false) {
        admit(&parts.shared, &mut status);
        drop(status);
        launch(parts.clone(), prompt, None);
    }
    false
}

pub(super) fn spawn_run(
    parts: RunParts,
    prompt: AgentMessage,
    requested: Option<u64>,
) -> Result<(), SessionError> {
    {
        let Ok(mut status) = parts.shared.status.lock() else {
            return Err(SessionError::Busy);
        };
        if *status == Status::Running {
            return Err(SessionError::Busy);
        }
        admit(&parts.shared, &mut status);
    }
    launch(parts, prompt, requested);
    Ok(())
}

/// Invariant: every run is counted under the status lock that admits it.
fn admit(shared: &Shared, status: &mut Status) {
    *status = Status::Running;
    shared
        .runs
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn launch(parts: RunParts, prompt: AgentMessage, requested: Option<u64>) {
    // Incident: nothing cleared the session-wide signal, so the first abort aborted every
    // later turn. Reading the epoch at admission still stops one hit before the spawn.
    let admitted_epoch = requested.unwrap_or_else(|| parts.shared.signal.epoch());
    tokio::spawn(async move {
        let ended_clean = run_once(&parts, prompt, admitted_epoch).await;
        settle(parts, ended_clean).await;
    });
}

/// Every way a run ends comes here: an early stop, an error, an abort or the loop's own end.
async fn settle(parts: RunParts, ended_clean: bool) {
    let shared = Arc::clone(&parts.shared);
    let next = match shared.status.lock() {
        Ok(mut status) => {
            // An abort keeps what was queued for after the answer until the user's next turn.
            let undisturbed = !shared.signal.is_fired();
            let next = owed(&shared, undisturbed, undisturbed && ended_clean);
            match next {
                Some(_) => admit(&shared, &mut status),
                None => *status = Status::Idle,
            }
            next
        }
        Err(_) => None,
    };
    if next.is_none() {
        shared.idle.notify_waiters();
    }
    // The end capture runs after the session is idle again: holding Running across it
    // rejects the follow-up the user types the moment the answer lands.
    let end_hook = shared.on_turn_end.lock().ok().and_then(|slot| slot.clone());
    if let Some(hook) = end_hook {
        let _hook_failure_never_fails_a_turn = tokio::task::spawn_blocking(move || hook()).await;
    }
    if let Some(prompt) = next {
        launch(parts, prompt, None);
    }
}

async fn run_once(parts: &RunParts, prompt: AgentMessage, admitted_epoch: u64) -> bool {
    let RunParts {
        shared,
        provider,
        system_prompt,
        tool_execution,
        compactor,
        on_compacted,
    } = parts.clone();
    let capture = start_capture(&shared);
    let assembling = yi_types::trace::span("turn.context");
    let mut context = LoopContext {
        system_prompt: {
            let _span = yi_types::trace::span("turn.system_prompt");
            system_prompt()
        },
        messages: shared
            .messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default(),
        tools: shared.tools.lock().as_deref().cloned().unwrap_or_default(),
    };
    drop(assembling);
    let (model, effort) = hooks::settings_of(&shared);
    let mut config = LoopConfig::new(model.clone());
    config.effort = effort;
    config.guards = crate::levers::get().loop_guards();
    if let Some(shape) = shared.shape.get() {
        (config.schema, config.shared_through) = (shape.schema.clone(), shape.shared_through);
    }
    config.tool_execution = tool_execution;
    config.reuse = shared.reuse.lock().map(|reuse| *reuse).unwrap_or_default();
    config.convert_to_llm = Box::new(yi_context::convert_to_llm);
    if let Some(compactor) = compactor.clone() {
        compactor.open_run(&prompt);
        let stores = Arc::clone(&shared);
        let notify = Arc::clone(&shared);
        let hook = on_compacted.clone();
        let failed = parts.clone();
        let elided = parts.clone();
        let announce = Arc::clone(&shared);
        config.maybe_compact = Some(crate::compaction::loop_hook(
            compactor,
            Arc::clone(&provider),
            {
                let (system, tools) = (context.system_prompt.clone(), context.tools.clone());
                Arc::new(move |effort| {
                    crate::compaction::LoopRequest::new(system.clone(), &tools, effort)
                })
            },
            Arc::new(move || store_of(&stores)),
            crate::compaction::CompactReports {
                waiting: Arc::new(move |wait| {
                    let _ = announce.events.send(AgentEvent::Wait { wait });
                }),
                compacted: Arc::new(move || {
                    dispatch_ext(&notify, &crate::ext::Event::Compacted);
                    if let Some(hook) = &hook {
                        hook();
                    }
                }),
                failed: Arc::new(move |error| failed_compaction(&failed, error)),
                elided: Arc::new(move |elision| {
                    queue_compaction_notice(&elided, &elision.to_string())
                }),
            },
        ));
    }
    wire_queues_and_coupling(&mut config, &shared, &prompt);
    // A follow-up delivered inside this run opens the work it does next: a rescue keeps it.
    if let (Some(compactor), Some(follow)) =
        (compactor.clone(), config.get_follow_up_messages.take())
    {
        config.get_follow_up_messages = Some(Box::new(move || {
            let taken = follow();
            if let Some(opening) = taken
                .iter()
                .find(|message| matches!(message, AgentMessage::User { .. }))
            {
                compactor.open_run(opening);
            }
            taken
        }));
    }
    wire_environment(&mut config, &shared);
    let gate = Arc::clone(&capture);
    config.side_work = Some(Box::new(move || Box::pin(settled(Arc::clone(&gate)))));
    wire_last_word(&mut config, &shared);
    let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut emit = emit_event(&shared, &failed);
    if cfg!(debug_assertions)
        && let Some(errored) = first_system_prompt_broken(&shared, &context.system_prompt, &model)
    {
        failed_turn(&shared, prompt, errored, &mut emit);
        return false;
    }
    shared.signal.reset_if_epoch(admitted_epoch);
    run_loop(
        &mut context,
        vec![prompt],
        &config,
        &shared.signal,
        &mut emit,
        provider.as_ref(),
    )
    .await;
    // The end capture must follow the start capture, or undo pairs the wrong trees.
    settled(capture).await;
    if let Ok(mut messages) = shared.messages.lock() {
        *messages = context.messages;
    }
    if let Some(host) = extensions_of(&shared) {
        let event = host.lock().ok().map(|host| host.turn_end_event());
        if let Some(event) = event {
            dispatch_ext(&shared, &event);
        }
    }
    !failed.load(std::sync::atomic::Ordering::SeqCst)
}

fn emit_event(
    shared: &Arc<Shared>,
    failed: &Arc<std::sync::atomic::AtomicBool>,
) -> impl FnMut(AgentEvent) {
    let shared = Arc::clone(shared);
    let failed = Arc::clone(failed);
    move |event: AgentEvent| {
        shared.time_turn(&event);
        if let AgentEvent::MessageEnd { message } = &event {
            if let AgentMessage::Assistant {
                stop_reason: StopReason::Error,
                ..
            } = message
            {
                failed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if let AgentMessage::Assistant { usage, .. } = message {
                if let Ok(mut last) = shared.last_usage.lock() {
                    *last = Some(usage.clone());
                }
                dispatch_ext(
                    &shared,
                    &crate::ext::Event::Usage {
                        input: usage.input,
                        cache_read: usage.cache_read,
                        cache_write: usage.cache_write,
                    },
                );
            }
            let _span = yi_types::trace::span("turn.persist");
            persist_message(&shared, message);
        }
        if let Some(telemetry) = shared.telemetry.lock().ok().and_then(|slot| slot.clone()) {
            telemetry.on_event(&event);
        }
        let _ = shared.events.send(event);
    }
}

/// D310: the first request's bytes are the conversation's. In a debug build a later turn whose
/// bytes differ ends before any request with this errored reply, fast and loud, and the
/// session still settles idle; a release build rests on `ext::Host::frozen` alone.
fn first_system_prompt_broken(
    shared: &Shared,
    system: &str,
    model: &yi_types::model::Model,
) -> Option<AgentMessage> {
    let first = shared.first_system_prompt.get_or_init(|| system.to_owned());
    if first == system {
        return None;
    }
    let at = first
        .bytes()
        .zip(system.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(first.len().min(system.len()));
    let text = format!(
        "the system prompt changed after the first request: {} bytes, was {}, first difference at byte {at}",
        system.len(),
        first.len()
    );
    Some(yi_loop::synthesized_error_message(model, &text))
}

/// Ends a turn before any request with `errored` as its reply, through the events a run emits,
/// so the transcript, the store and every front end see the same failed turn.
fn failed_turn(
    shared: &Shared,
    prompt: AgentMessage,
    errored: AgentMessage,
    emit: &mut impl FnMut(AgentEvent),
) {
    emit(AgentEvent::AgentStart);
    for message in [&prompt, &errored] {
        emit(AgentEvent::MessageStart {
            message: message.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
    }
    emit(AgentEvent::AgentEnd {
        messages: vec![prompt.clone(), errored.clone()],
    });
    if let Ok(mut messages) = shared.messages.lock() {
        messages.extend([prompt, errored]);
    }
}

type Capture = Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>;

fn start_capture(shared: &Shared) -> Capture {
    let hook = shared
        .on_turn_start
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    let handle = hook.map(|hook| {
        tokio::task::spawn_blocking(move || {
            let _span = yi_types::trace::span("turn.start_hook");
            hook();
        })
    });
    Arc::new(tokio::sync::Mutex::new(handle))
}

async fn settled(capture: Capture) {
    let handle = capture.lock().await.take();
    if let Some(handle) = handle {
        let _span = yi_types::trace::span("turn.start_hook_wait");
        let _hook_failure_never_fails_a_turn = handle.await;
    }
}

/// The stop test and the last word: the deadline's, the wind-down's and the turn cap's.
fn wire_last_word(config: &mut LoopConfig, shared: &Arc<Shared>) {
    // Not the interrupt: the turn in flight ends and settles, and no request follows.
    let stop = Arc::clone(shared);
    let capped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (turns, cap_hit) = (std::sync::atomic::AtomicU32::new(0), Arc::clone(&capped));
    let cap = shared.turn_cap.get().copied();
    config.should_stop_after_turn = Some(Box::new(move |_| {
        let taken = turns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1);
        let at_cap = cap.is_some_and(|cap| taken >= cap.saturating_sub(1));
        cap_hit.store(at_cap, std::sync::atomic::Ordering::SeqCst);
        stop.winding_down() || stop.last_word_due() || at_cap
    }));
    // Incident: the wind-down ended three confirmation runs on a tool call, with no answer.
    let clock = Arc::clone(shared);
    config.last_word = Some(Box::new(move |_| {
        let cancelled = clock.cancelled.load(std::sync::atomic::Ordering::SeqCst);
        let out_of_time = clock.deadline.get().is_some_and(|at| at.winding_down());
        let out_of_time = out_of_time || clock.last_word_due();
        if capped.load(std::sync::atomic::Ordering::SeqCst) && !out_of_time && !cancelled {
            return Some(super::host_text(
                yi_types::message::HostSource::Deadline,
                TURN_CAP_WORD,
            ));
        }
        (out_of_time && !cancelled)
            .then(|| super::host_text(yi_types::message::HostSource::Deadline, LAST_WORD))
    }));
    let due = Arc::clone(shared);
    config.last_word_due = Some(Box::new(move || due.last_word_due()));
}

fn wire_environment(config: &mut LoopConfig, shared: &Arc<Shared>) {
    let hook = shared.environment.lock().ok().and_then(|slot| slot.clone());
    let Some(hook) = hook else {
        return;
    };
    // Incident: one block per prompt froze `files:`, `deadline:` and `todos:` for an hour of
    // tool calls, so the model read its own files as deleted; the facts are read per request.
    config.request_tail = Some(Box::new(move || {
        let _span = yi_types::trace::span("env.block");
        hook()
            .map(crate::environment::message)
            .into_iter()
            .collect()
    }));
}

fn wire_queues_and_coupling(config: &mut LoopConfig, shared: &Arc<Shared>, prompt: &AgentMessage) {
    let steer = Arc::clone(shared);
    let follow = Arc::clone(shared);
    // The loop reads steering once before its first request: the prompt's pointers ride that read.
    let opening = std::sync::Mutex::new(pointers_for(shared, prompt));
    config.get_steering_messages = Some(Box::new(move || {
        let mut taken = opening
            .lock()
            .map(|mut pointers| std::mem::take(&mut *pointers))
            .unwrap_or_default();
        taken.extend(with_pointers(&steer, drain(&steer)));
        taken
    }));
    config.get_follow_up_messages = Some(Box::new(move || {
        let taken: Vec<AgentMessage> = follow
            .follow_up
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
            .into_iter()
            .filter(Queued::current)
            .map(|queued| queued.message)
            .collect();
        with_pointers(&follow, taken)
    }));
    let coupling = shared
        .coupling
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(coupling) = coupling {
        config.first_turn_tool_choice = (coupling.on_prompt)(prompt);
        let observe = Arc::clone(&coupling.on_turn);
        config.prepare_next_turn = Some(Box::new(move |snapshot| {
            observe(snapshot);
            None
        }));
        let intercept = Arc::clone(&coupling.intercept_stop);
        config.intercept_stop = Some(Box::new(move |snapshot| intercept(snapshot)));
    }
    config.waiting = shared.waits.lock().ok().and_then(|slot| slot.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::dispatch::tests::faux_model;
    use crate::provider::ProviderStream;
    use crate::session::{AgentSession, SessionConfig, user_input};
    use yi_ai::faux::{faux_assistant_message, faux_text};
    use yi_types::message::{StopReason, UserContent};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn session(replies: usize) -> AgentSession {
        let provider = Arc::new(ProviderStream::new(None));
        provider.queue_faux(
            (0..replies)
                .map(|_| faux_assistant_message(vec![faux_text("on it")], StopReason::Stop))
                .collect(),
        );
        AgentSession::new(
            SessionConfig {
                system_prompt: String::new(),
                model: faux_model(),
                thinking_level: None,
                tool_execution: yi_loop::ExecutionMode::Sequential,
            },
            provider,
        )
    }

    fn running_with_a_steer(session: &AgentSession) -> Result<RunParts, &'static str> {
        let parts = session.parts();
        *parts.shared.status.lock().map_err(|_| "status poisoned")? = Status::Running;
        let held = Queued::new(user_input("one more thing"), false, None);
        parts
            .shared
            .steer
            .lock()
            .map_err(|_| "queue poisoned")?
            .push_back(held);
        Ok(parts)
    }

    fn user_texts(session: &AgentSession) -> Vec<String> {
        session
            .messages()
            .into_iter()
            .filter_map(|message| match message {
                AgentMessage::User {
                    content: UserContent::Text(text),
                    ..
                } => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Dies with a user steer that landed after the loop's last read: the run settled idle, the
    /// UI forgot the row, and the text waited for the next prompt to arrive behind it.
    #[tokio::test]
    async fn a_steer_the_loop_never_read_runs_as_the_next_prompt() -> TestResult {
        let session = session(1);
        settle(running_with_a_steer(&session)?, true).await;
        session.wait_idle().await;
        assert_eq!(user_texts(&session), ["one more thing"]);
        Ok(())
    }

    /// Dies with an abort that restarts the run: the queue keeps what was steered for the user's
    /// next turn, so a settle after a fired signal leaves it there and goes idle.
    #[tokio::test]
    async fn a_steer_left_by_an_abort_waits_for_the_next_turn() -> TestResult {
        let session = session(1);
        let parts = running_with_a_steer(&session)?;
        parts.shared.signal.fire();
        let shared = Arc::clone(&parts.shared);
        settle(parts, true).await;
        assert_eq!(session.status(), Status::Idle);
        assert_eq!(shared.steer.lock().map_err(|_| "queue poisoned")?.len(), 1);
        Ok(())
    }

    /// Dies with an errored settle waking the steer: a run that ended in error owes the steer
    /// nothing, so a queued steer stays for the user's next turn instead of billing a prompt.
    #[tokio::test]
    async fn an_errored_settle_leaves_a_queued_steer_for_the_next_turn() -> TestResult {
        let session = session(1);
        let parts = running_with_a_steer(&session)?;
        let shared = Arc::clone(&parts.shared);
        settle(parts, false).await;
        assert_eq!(session.status(), Status::Idle);
        assert_eq!(shared.steer.lock().map_err(|_| "queue poisoned")?.len(), 1);
        assert_eq!(user_texts(&session), Vec::<String>::new());
        Ok(())
    }

    /// Dies with a follow-up to an idle session starting the steer it found inboxed as the
    /// prompt: only the end of a run owes a stranded steer a turn, so the follow-up is the prompt.
    #[tokio::test]
    async fn a_follow_up_does_not_start_a_steer_inboxed_while_idle() -> TestResult {
        let session = session(2);
        session.steer_message(user_input("one more thing"));
        session.follow_up("later");
        session.wait_idle().await;
        assert_eq!(user_texts(&session), ["later", "one more thing"]);
        Ok(())
    }
}

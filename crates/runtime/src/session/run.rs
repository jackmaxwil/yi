use std::collections::VecDeque;
use std::sync::Arc;

use yi_loop::{LoopConfig, LoopContext, run_loop};
use yi_types::event::AgentEvent;
use yi_types::mail::Delivery;
use yi_types::message::AgentMessage;

use super::{
    RunParts, SessionError, Shared, Status, dispatch_ext, extensions_of, hooks, persist_message,
    record_store_error, store_of,
};

const LAST_WORD: &str = "[deadline] Time is up: no more tool calls. Write your final answer now from what you have, and say plainly what is unfinished.";

/// Invariant: asked under the session's status lock when a turn would present the message, so
/// it may lock a host's roster but nothing that waits on this session; false drops it unread.
pub type StillNews = Arc<dyn Fn() -> bool + Send + Sync>;

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
fn owed(shared: &Shared, follow_ups: bool) -> Option<AgentMessage> {
    if shared.winding_down() || shared.held.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    if let Ok(mut queue) = shared.steer.lock() {
        queue.retain(Queued::current);
        if queue.iter().any(|queued| queued.wakes) {
            return queue.pop_front().map(|queued| queued.message);
        }
    }
    let mut follow = shared.follow_up.lock().ok()?;
    (follow_ups && !follow.is_empty()).then(|| follow.remove(0))
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
    let Some(prompt) = wakes.then(|| owed(&parts.shared, false)).flatten() else {
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
    if let Some(prompt) = owed(&parts.shared, true) {
        admit(&parts.shared, &mut status);
        drop(status);
        launch(parts.clone(), prompt, None);
    }
}

pub(super) fn unsaved_compaction(parts: &RunParts, error: &yi_session::SessionError) {
    record_store_error(&parts.shared, error);
    let notice =
        format!("[compaction not saved: {error}; history left uncompacted, /compact retries]");
    enqueue(
        parts,
        Queued::new(super::user_message(&notice), false, None),
    );
}

pub(super) fn follow(parts: &RunParts, message: AgentMessage) -> bool {
    let Ok(mut status) = parts.shared.status.lock() else {
        return false;
    };
    if let Ok(mut queue) = parts.shared.follow_up.lock() {
        queue.push(message);
    }
    if *status == Status::Running {
        return true;
    }
    if let Some(prompt) = owed(&parts.shared, true) {
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
        run_once(&parts, prompt, admitted_epoch).await;
        settle(parts).await;
    });
}

/// Every way a run ends comes here: an early stop, an error, an abort or the loop's own end.
async fn settle(parts: RunParts) {
    let shared = Arc::clone(&parts.shared);
    let next = match shared.status.lock() {
        Ok(mut status) => {
            // An abort keeps what was queued for after the answer until the user's next turn.
            let next = owed(&shared, !shared.signal.is_fired());
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

async fn run_once(parts: &RunParts, prompt: AgentMessage, admitted_epoch: u64) {
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
    config.tool_execution = tool_execution;
    config.convert_to_llm = Box::new(yi_context::convert_to_llm);
    if let Some(compactor) = compactor.clone() {
        let stores = Arc::clone(&shared);
        let notify = Arc::clone(&shared);
        let hook = on_compacted.clone();
        let unsaved = parts.clone();
        let announce = Arc::clone(&shared);
        config.maybe_compact = Some(crate::compaction::loop_hook(
            compactor,
            Arc::clone(&provider),
            model.clone(),
            Arc::clone(&system_prompt),
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
                unsaved: Arc::new(move |error| unsaved_compaction(&unsaved, error)),
            },
        ));
    }
    wire_queues_and_coupling(&mut config, &shared, &prompt);
    wire_environment(&mut config, &shared);
    let gate = Arc::clone(&capture);
    config.side_work = Some(Box::new(move || Box::pin(settled(Arc::clone(&gate)))));
    // Not the interrupt: the turn in flight ends and settles, and no request follows.
    let stop = Arc::clone(&shared);
    config.should_stop_after_turn = Some(Box::new(move |_| {
        stop.winding_down() || stop.last_word_due()
    }));
    // Incident: the wind-down ended three confirmation runs on a tool call, with no answer.
    let clock = Arc::clone(&shared);
    config.last_word = Some(Box::new(move |_| {
        let cancelled = clock.cancelled.load(std::sync::atomic::Ordering::SeqCst);
        let out_of_time = clock.deadline.get().is_some_and(|at| at.winding_down());
        let out_of_time = out_of_time || clock.last_word_due();
        (out_of_time && !cancelled).then(|| super::user_message(LAST_WORD))
    }));
    let due = Arc::clone(&shared);
    config.last_word_due = Some(Box::new(move || due.last_word_due()));
    let emit_shared = Arc::clone(&shared);
    let mut emit = move |event: AgentEvent| {
        emit_shared.time_turn(&event);
        if let AgentEvent::MessageEnd { message } = &event {
            if let AgentMessage::Assistant { usage, .. } = message {
                if let Ok(mut last) = emit_shared.last_usage.lock() {
                    *last = Some(usage.clone());
                }
                if let Some(compactor) = &compactor {
                    compactor.on_usage(usage);
                }
                dispatch_ext(
                    &emit_shared,
                    &crate::ext::Event::Usage {
                        input: usage.input,
                        cache_read: usage.cache_read,
                        cache_write: usage.cache_write,
                    },
                );
            }
            let _span = yi_types::trace::span("turn.persist");
            persist_message(&emit_shared, message);
        }
        if let Some(telemetry) = emit_shared
            .telemetry
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
        {
            telemetry.on_event(&event);
        }
        let _ = emit_shared.events.send(event);
    };
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
        let taken = follow
            .follow_up
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
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

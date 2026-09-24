use std::collections::VecDeque;
use std::sync::Arc;

use yi_loop::{LoopConfig, LoopContext, run_loop};
use yi_types::event::AgentEvent;
use yi_types::mail::Delivery;
use yi_types::message::AgentMessage;

use super::{
    RunParts, SessionError, Shared, Status, dispatch_ext, extensions_of, hooks, persist_message,
    store_of,
};

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

/// Invariant: an envelope is queued once, whether a respawn moved it or an attach re-read it.
pub(super) fn push(queue: &mut VecDeque<Queued>, entry: Queued) {
    let id = crate::mail::envelope_id(&entry.message);
    let queued = |held: &Queued| crate::mail::envelope_id(&held.message) == id;
    if id.is_none() || !queue.iter().any(queued) {
        queue.push_back(entry);
    }
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

/// The prompt a waking entry or a follow-up is owed, taken under the status lock enqueue takes.
fn owed(shared: &Shared, follow_ups: bool) -> Option<AgentMessage> {
    if shared.winding_down() {
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
    if *status == Status::Running {
        return Delivery::Queued;
    }
    let Some(prompt) = wakes.then(|| owed(&parts.shared, false)).flatten() else {
        return Delivery::Inboxed;
    };
    *status = Status::Running;
    drop(status);
    launch(parts.clone(), prompt, None);
    Delivery::Woken
}

pub(super) fn follow(parts: &RunParts, message: AgentMessage) {
    let Ok(mut status) = parts.shared.status.lock() else {
        return;
    };
    if let Ok(mut queue) = parts.shared.follow_up.lock() {
        queue.push(message);
    }
    if *status == Status::Running {
        return;
    }
    if let Some(prompt) = owed(&parts.shared, true) {
        *status = Status::Running;
        drop(status);
        launch(parts.clone(), prompt, None);
    }
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
        *status = Status::Running;
    }
    launch(parts, prompt, requested);
    Ok(())
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
            if next.is_none() {
                *status = Status::Idle;
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
    let hook = shared
        .on_turn_start
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    if let Some(hook) = hook {
        // Snapshotting shells out to git; the turn waits for it but the
        // runtime thread does not.
        let _hook_failure_never_fails_a_turn = tokio::task::spawn_blocking(move || hook()).await;
    }
    let mut context = LoopContext {
        system_prompt: system_prompt(),
        messages: shared
            .messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default(),
        tools: shared.tools.lock().as_deref().cloned().unwrap_or_default(),
    };
    let (model, effort) = hooks::settings_of(&shared);
    let mut config = LoopConfig::new(model.clone());
    config.effort = effort;
    config.tool_execution = tool_execution;
    config.convert_to_llm = Box::new(yi_context::convert_to_llm);
    if let Some(compactor) = compactor.clone() {
        let stores = Arc::clone(&shared);
        let notify = Arc::clone(&shared);
        let hook = on_compacted.clone();
        config.maybe_compact = Some(crate::compaction::loop_hook(
            compactor,
            Arc::clone(&provider),
            model.clone(),
            Arc::clone(&system_prompt),
            Arc::new(move || store_of(&stores)),
            Arc::new(move || {
                dispatch_ext(&notify, &crate::ext::Event::Compacted);
                if let Some(hook) = &hook {
                    hook();
                }
            }),
        ));
    }
    wire_queues_and_coupling(&mut config, &shared, &prompt);
    wire_environment(&mut config, &shared);
    // Not the interrupt: the turn in flight ends and settles, and no request follows.
    let stop = Arc::clone(&shared);
    config.should_stop_after_turn = Some(Box::new(move |_| stop.winding_down()));
    let emit_shared = Arc::clone(&shared);
    let mut emit = move |event: AgentEvent| {
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

fn wire_environment(config: &mut LoopConfig, shared: &Arc<Shared>) {
    let hook = shared.environment.lock().ok().and_then(|slot| slot.clone());
    let Some(hook) = hook else {
        return;
    };
    // Incident: one block per prompt froze `files:`, `deadline:` and `todos:` for an hour of
    // tool calls, so the model read its own files as deleted; the facts are read per request.
    config.transform_context = Some(Box::new(move |messages| {
        hook().map(|block| crate::environment::append(messages, &block))
    }));
}

fn wire_queues_and_coupling(config: &mut LoopConfig, shared: &Arc<Shared>, prompt: &AgentMessage) {
    let steer = Arc::clone(shared);
    let follow = Arc::clone(shared);
    config.get_steering_messages = Some(Box::new(move || drain(&steer)));
    config.get_follow_up_messages = Some(Box::new(move || {
        follow
            .follow_up
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
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

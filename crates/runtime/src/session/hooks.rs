//! The session's handle vocabulary: every closure the runtime hands a subsystem so it can
//! steer, notice or read this session without holding it. The run loop stays out.

use std::sync::Arc;

use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{Effort, Model};
use yi_types::wire::SessionStats;

use super::run::{self, Queued};
use super::{
    AgentSession, ExtHook, Shared, Status, StillNews, attribute_to_shared, dispatch_ext, host_text,
    store_of,
};

pub(super) type LedgerFold = dyn FnMut(&str, &[Entry], &SessionStats) + Send;

pub(super) fn settings_of(shared: &Shared) -> (Model, Effort) {
    let model = shared
        .model
        .lock()
        .map(|model| model.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    let effort = shared
        .effort
        .lock()
        .map(|effort| *effort)
        .unwrap_or_else(|poisoned| *poisoned.into_inner());
    (model, effort)
}

impl AgentSession {
    /// Invariant: a resumed session's state is a fold of its ledger, never a count from zero;
    /// `fold` reads each store attached after this call (id, branch, totals) before a request.
    pub fn on_attach(&self, fold: impl FnMut(&str, &[Entry], &SessionStats) + Send + 'static) {
        if let Ok(mut folds) = self.on_attach.lock() {
            folds.push(Box::new(fold));
        }
    }

    pub(super) fn restore_from_ledger(&self, entries: &[Entry]) {
        let mut effort = None;
        for entry in entries {
            match entry {
                Entry::ModelChange {
                    provider, model_id, ..
                } => {
                    if let Some(model) = crate::provider::resolve_model(provider, model_id)
                        && let Ok(mut slot) = self.shared.model.lock()
                    {
                        *slot = model;
                    }
                }
                Entry::ThinkingLevelChange { thinking_level, .. } => {
                    effort = thinking_level.parse().ok();
                }
                _ => {}
            }
        }
        let restored = self.model().clamp_effort(effort.unwrap_or(self.effort()));
        if let Ok(mut slot) = self.shared.effort.lock() {
            *slot = restored;
        }
        let (id, stats) = store_of(&self.shared).map_or_else(
            || (String::new(), SessionStats::zero()),
            |store| {
                let session = yi_session::lock_session(&store);
                (session.metadata().id.clone(), session.stats())
            },
        );
        if let Ok(mut folds) = self.on_attach.lock() {
            for fold in folds.iter_mut() {
                fold(&id, entries, &stats);
            }
        }
    }

    pub fn abort(&self) {
        self.shared.signal.fire();
    }

    /// The interrupt's epoch, read when a run is requested for [`Self::prompt_requested`].
    pub fn abort_epoch(&self) -> u64 {
        self.shared.signal.epoch()
    }

    pub async fn wait_idle(&self) {
        super::until(&self.shared.idle, || match self.status() {
            Status::Idle => std::ops::ControlFlow::Break(()),
            _ => std::ops::ControlFlow::Continue(None),
        })
        .await;
    }

    pub fn ext_hook(&self) -> ExtHook {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |event| dispatch_ext(&shared, &event))
    }

    /// Read per call: the engine attaches after the heartbeat service is wired.
    pub fn rules_handle(
        &self,
    ) -> Arc<dyn Fn() -> Option<Arc<crate::rules::RuleEngine>> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || shared.rules.lock().ok().and_then(|slot| slot.clone()))
    }

    pub fn store_handle(
        &self,
    ) -> std::sync::Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        std::sync::Arc::new(move || store_of(&shared))
    }

    /// The environment block as the host would render it now, for checks that count its facts.
    pub fn environment_handle(&self) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let hook = shared
                .environment
                .lock()
                .ok()
                .and_then(|slot| slot.clone())?;
            hook()
        })
    }

    /// Incident: snapshotting these left `rlm.run` and `model.info` on the
    /// startup model once the TUI could switch.
    pub fn kernel_state_handle(&self) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
        let kernel = Arc::clone(&self.kernel);
        Arc::new(move || {
            kernel
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|service| service.state()))
        })
    }

    pub fn settings_handle(&self) -> Arc<dyn Fn() -> (Model, Effort) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || settings_of(&shared))
    }

    /// A steer wakes an idle session through the one queue; a follow-up waits for a turn's answer.
    pub fn heartbeat_hook(
        &self,
    ) -> Arc<dyn Fn(AgentMessage, yi_types::schedule::DeliveryMode) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |message, mode| match mode {
            yi_types::schedule::DeliveryMode::Steer => {
                run::enqueue(&parts, Queued::new(message, true, None));
            }
            yi_types::schedule::DeliveryMode::FollowUp => {
                run::follow(&parts, message);
            }
        })
    }

    pub fn deliver_hook(&self) -> Arc<dyn Fn(AgentMessage, bool) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |message, wakes| {
            run::enqueue(&parts, Queued::new(message, wakes, None));
        })
    }

    /// Delivers each text `observe` returns as a shown notice of `custom_type`, never a wake:
    /// a notice must not buy another turn.
    pub fn show_notices(
        &self,
        custom_type: &'static str,
        mut observe: impl FnMut(&yi_types::event::AgentEvent) -> Option<String> + Send + 'static,
    ) {
        let mut events = self.subscribe();
        let deliver = self.deliver_hook();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Some(text) = observe(&event) {
                            let notice =
                                AgentMessage::host_note(custom_type, text, yi_session::now_ms());
                            deliver(notice, false);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub fn mail_hook(&self) -> Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let taken = shared
                .steer
                .lock()
                .map(|mut queue| run::take_mail(&mut queue));
            crate::mail::received(store_of(&shared), taken.unwrap_or_default())
        })
    }

    /// Notified whenever a message joins the steer queue, for a wait on [`Self::mail_hook`].
    pub fn mail_arrived(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.shared.mail)
    }

    pub fn wake_idle_hook(&self) -> Arc<dyn Fn(AgentMessage, Option<StillNews>) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |message, news| {
            run::enqueue(&parts, Queued::new(message, true, news));
        })
    }

    /// Running ⇒ steer queue (next tool boundary); idle ⇒ follow-up queue. The
    /// advisor never wakes an idle primary.
    pub fn advisory_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |message| {
            let Ok(status) = shared.status.lock() else {
                return;
            };
            if *status == Status::Running {
                if let Ok(mut queue) = shared.steer.lock() {
                    run::push(&mut queue, Queued::new(message, false, None));
                }
            } else if let Ok(mut queue) = shared.follow_up.lock() {
                queue.push(Queued::new(message, false, None));
            }
        })
    }

    pub fn history_handle(&self) -> Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .messages
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default()
        })
    }

    /// Summed under the lock, not over a history clone; `None` when poisoned, never a fake `0.0`.
    /// The flag is true when a reply came back without usage, so the sum is a lower bound.
    pub fn cost_handle(&self) -> Arc<dyn Fn() -> Option<(f64, bool)> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let messages = shared.messages.lock().ok()?;
            let usages = messages.iter().filter_map(|message| match message {
                AgentMessage::Assistant { usage, .. } => Some(usage),
                _ => None,
            });
            Some(usages.fold((0.0, false), |(cost, lower_bound), usage| {
                let spent = usage.cost.total.as_f64().unwrap_or(0.0);
                (cost + spent, lower_bound || usage.unknown)
            }))
        })
    }

    pub fn usage_handle(&self) -> Arc<dyn Fn() -> Option<Usage> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .last_usage
                .lock()
                .ok()
                .and_then(|usage| usage.clone())
        })
    }

    pub fn activity_handle(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false)
        })
    }

    pub fn halt_hook(&self) -> Arc<dyn Fn(bool) -> bool + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |on| {
            let held = &parts.shared.held;
            let was = held.swap(on, std::sync::atomic::Ordering::SeqCst);
            if !on {
                if was {
                    run::kick(&parts);
                }
                return was;
            }
            let running = parts
                .shared
                .status
                .lock()
                .is_ok_and(|status| *status == Status::Running);
            parts.shared.signal.fire();
            running
        })
    }

    /// Invariant: an idle queue holds inboxed notices and held follow-ups waiting for another
    /// turn, so only a queue behind a running turn is pending work that defers a heartbeat.
    pub fn heartbeat_deliverer(&self) -> Arc<crate::schedule::DeliverFn> {
        let shared = Arc::clone(&self.shared);
        let compactor = self.compactor.clone();
        let hook = self.heartbeat_hook();
        Arc::new(move |job, firing| {
            let is_streaming = shared
                .status
                .lock()
                .is_ok_and(|status| *status == Status::Running);
            let queued = shared.steer.lock().is_ok_and(|queue| !queue.is_empty())
                || shared.follow_up.lock().is_ok_and(|queue| !queue.is_empty());
            let activity = crate::schedule::SessionActivity {
                is_streaming,
                is_compacting: compactor.as_ref().is_some_and(|c| c.compacting()),
                has_pending_session_work: is_streaming && queued,
            };
            if crate::schedule::should_defer(job, &activity) {
                return Ok(crate::schedule::RunOutcome::Deferred);
            }
            let todos = shared
                .todos
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(Arc::clone))
                .ok_or("a tick creates a todo, and this session has no todo list")?;
            let fired = crate::schedule::clock::fire(&todos, job, firing)?;
            let outcome = crate::schedule::clock::outcome(&fired);
            if let Some(wake) =
                crate::schedule::clock::wake_message(job, &fired, yi_session::now_ms())
            {
                let mode = job
                    .delivery_mode
                    .unwrap_or(crate::schedule::DEFAULT_HEARTBEAT_DELIVERY_MODE);
                hook(wake, mode);
            }
            Ok(outcome)
        })
    }

    /// A job's report (§4.3), taken after a running turn's answer or the next one's, never waking;
    /// `news` false when the queue is read drops it unread.
    pub fn follow_up_hook(&self) -> Arc<super::FollowUpFn> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str, news| {
            if let Ok(mut queue) = shared.follow_up.lock() {
                let message = host_text(yi_types::message::HostSource::Job, text);
                queue.push(Queued::new(message, false, news));
            }
        })
    }

    pub fn wait_hook(&self) -> Arc<crate::compaction::WaitFn> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |wait| {
            let _ = shared
                .events
                .send(yi_types::event::AgentEvent::Wait { wait });
        })
    }

    pub fn notice_hook(
        &self,
        source: yi_types::message::HostSource,
    ) -> Arc<dyn Fn(&str) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |text: &str| {
            run::enqueue(&parts, Queued::new(host_text(source, text), false, None));
        })
    }

    /// The model snapshot is taken now: a later [`AgentSession::set_model`] leaves percent
    /// computed against the old window until re-wired.
    pub fn compact_status_handle(
        &self,
    ) -> Option<Arc<dyn Fn() -> crate::compaction::CompactStatus + Send + Sync>> {
        let compactor = self.compactor.clone()?;
        let shared = Arc::clone(&self.shared);
        let model = self.model();
        Some(Arc::new(move || match shared.messages.lock() {
            Ok(messages) => compactor.status(&messages, &model),
            Err(_) => compactor.status(&[], &model),
        }))
    }

    /// Usable after the session moves — the hook holds only shared state.
    pub fn attribution_handle(&self) -> Arc<dyn Fn(&Usage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |child: &Usage| attribute_to_shared(&shared, child))
    }
}

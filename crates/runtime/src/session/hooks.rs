//! The session's handle vocabulary: every closure the runtime hands a subsystem so it can
//! steer, notice or read this session without holding it. The run loop stays out.

use std::sync::Arc;

use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{Effort, Model};

use super::run::{self, Queued};
use super::{
    AgentSession, ExtHook, Shared, Status, StillNews, attribute_to_shared, dispatch_ext, store_of,
    user_message,
};

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
    pub fn abort(&self) {
        self.shared.signal.fire();
    }

    /// The interrupt's epoch, read when a run is requested for [`Self::prompt_requested`].
    pub fn abort_epoch(&self) -> u64 {
        self.shared.signal.epoch()
    }

    pub async fn wait_idle(&self) {
        loop {
            if self.status() == Status::Idle {
                return;
            }
            self.shared.idle.notified().await;
        }
    }

    pub fn ext_hook(&self) -> ExtHook {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |event| dispatch_ext(&shared, &event))
    }

    pub fn store_handle(
        &self,
    ) -> std::sync::Arc<dyn Fn() -> Option<yi_session::SharedSession> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        std::sync::Arc::new(move || store_of(&shared))
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
                queue.push(message);
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
    pub fn cost_handle(&self) -> Arc<dyn Fn() -> Option<f64> + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
            let messages = shared.messages.lock().ok()?;
            Some(
                messages
                    .iter()
                    .filter_map(|message| {
                        if let AgentMessage::Assistant { usage, .. } = message {
                            usage.cost.total.as_f64()
                        } else {
                            None
                        }
                    })
                    .sum(),
            )
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
            let Some(wake) =
                crate::schedule::clock::wake_message(job, &fired, yi_session::now_ms())
            else {
                return Ok(crate::schedule::RunOutcome::Skipped);
            };
            let mode = job
                .delivery_mode
                .unwrap_or(crate::schedule::DEFAULT_HEARTBEAT_DELIVERY_MODE);
            hook(wake, mode);
            Ok(crate::schedule::RunOutcome::Ran)
        })
    }

    /// A job's report (§4.3), taken after a running turn's answer or the next one's, never waking.
    pub fn follow_up_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str| {
            if let Ok(mut queue) = shared.follow_up.lock() {
                queue.push(user_message(text));
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

    pub fn notice_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let parts = self.parts();
        Arc::new(move |text: &str| {
            run::enqueue(&parts, Queued::new(user_message(text), false, None));
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

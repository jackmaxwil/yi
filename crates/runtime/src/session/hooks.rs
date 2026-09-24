//! The session's handle vocabulary: every closure the runtime hands a subsystem so it can
//! steer, notice or read this session without holding it. The run loop stays out.

use std::sync::Arc;

use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{Effort, Model};

use super::{
    AgentSession, ExtHook, Shared, Status, attribute_to_shared, dispatch_ext, store_of,
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

fn take_queued(queue: &std::sync::Mutex<Vec<AgentMessage>>, message: &AgentMessage) -> bool {
    let Ok(mut pending) = queue.lock() else {
        return false;
    };
    match pending.iter().position(|queued| queued == message) {
        Some(index) => {
            pending.remove(index);
            true
        }
        None => false,
    }
}

impl AgentSession {
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

    /// Running session ⇒ queued (Steer drains at the next message boundary,
    /// FollowUp at turn end); idle session ⇒ the message starts a run.
    pub fn heartbeat_hook(
        &self,
    ) -> Arc<dyn Fn(AgentMessage, yi_types::schedule::DeliveryMode) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        let run = self.run_handle();
        Arc::new(move |message, mode| {
            let running = shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false);
            if running {
                let queue = match mode {
                    yi_types::schedule::DeliveryMode::Steer => &shared.steer,
                    yi_types::schedule::DeliveryMode::FollowUp => &shared.follow_up,
                };
                if let Ok(mut pending) = queue.lock() {
                    pending.push(message);
                }
            } else {
                let _ = run(message);
            }
        })
    }

    /// A running turn takes the message from its follow-up queue; one that ended past its
    /// last drain leaves it there, and this task takes it back and starts the next turn.
    pub fn wake_idle_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        let run = self.run_handle();
        Arc::new(move |message| {
            let shared = Arc::clone(&shared);
            let run = Arc::clone(&run);
            tokio::spawn(async move {
                let mut queued = false;
                loop {
                    // Armed before the status read: `notify_waiters` stores no permit.
                    let idle = shared.idle.notified();
                    tokio::pin!(idle);
                    idle.as_mut().enable();
                    let running = shared
                        .status
                        .lock()
                        .map(|status| *status == Status::Running)
                        .unwrap_or(false);
                    if !running {
                        if queued && !take_queued(&shared.follow_up, &message) {
                            return;
                        }
                        queued = false;
                        if run(message.clone()).is_ok() {
                            return;
                        }
                    }
                    if !queued {
                        if let Ok(mut pending) = shared.follow_up.lock() {
                            pending.push(message.clone());
                        }
                        queued = true;
                    }
                    idle.await;
                }
            });
        })
    }

    /// Running ⇒ steer queue (next tool boundary); idle ⇒ follow-up queue. The
    /// advisor never wakes an idle primary.
    pub fn advisory_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |message| {
            let running = shared
                .status
                .lock()
                .map(|status| *status == Status::Running)
                .unwrap_or(false);
            let queue = if running {
                &shared.steer
            } else {
                &shared.follow_up
            };
            if let Ok(mut pending) = queue.lock() {
                pending.push(message);
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

    /// A user-role steering message consumed at the next message boundary, or
    /// the next turn when idle — R3 delivery for work finishing outside a turn.
    pub fn follow_up_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str| {
            if let Ok(mut queue) = shared.follow_up.lock() {
                queue.push(user_message(text));
            }
        })
    }

    pub fn notice_hook(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |text: &str| {
            if let Ok(mut queue) = shared.steer.lock() {
                queue.push(user_message(text));
            }
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
        Some(Arc::new(move || {
            let messages = shared
                .messages
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default();
            compactor.status(&messages, &model)
        }))
    }

    /// Usable after the session moves — the hook holds only shared state.
    pub fn attribution_handle(&self) -> Arc<dyn Fn(&Usage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |child: &Usage| attribute_to_shared(&shared, child))
    }
}

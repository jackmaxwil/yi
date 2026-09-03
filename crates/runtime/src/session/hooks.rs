//! The session's handle vocabulary: every closure the runtime hands a subsystem so it can
//! steer, notice or read this session without holding it. The run loop stays out.

use std::sync::Arc;

use yi_types::message::{AgentMessage, Usage};
use yi_types::model::{Effort, Model};

use super::{
    AgentSession, ExtHook, Status, attribute_to_shared, dispatch_ext, store_of, user_message,
};

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
    pub fn settings_handle(&self) -> Arc<dyn Fn() -> (Model, Effort) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        Arc::new(move || {
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
        })
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

    /// Waits out any running turn rather than gating on status: AgentEnd is emitted while the
    /// status is still Running, so a status-gated hook queued into a follow-up that never ran.
    pub fn wake_idle_hook(&self) -> Arc<dyn Fn(AgentMessage) + Send + Sync> {
        let shared = Arc::clone(&self.shared);
        let run = self.run_handle();
        Arc::new(move |message| {
            let shared = Arc::clone(&shared);
            let run = Arc::clone(&run);
            tokio::spawn(async move {
                loop {
                    let idle = shared
                        .status
                        .lock()
                        .map(|status| *status == Status::Idle)
                        .unwrap_or(true);
                    if idle {
                        break;
                    }
                    shared.idle.notified().await;
                }
                let _busy_means_queued = run(message);
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

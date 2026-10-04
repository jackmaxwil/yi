use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use yi_types::event::AgentEvent;
use yi_types::message::AgentMessage;

use crate::AgentSession;

pub const SPEND_ALERT_TYPE: &str = "spend_alert";

/// Invariant: arithmetic alone decides an alert, never a model: the session's own turns plus
/// each child's count, both in `total_tokens`, announced once per multiple of `every` crossed.
pub struct SpendAlarm {
    every: NonZeroU64,
    own: u64,
    children: HashMap<String, u64>,
    announced: u64,
}

impl SpendAlarm {
    pub fn new(every: NonZeroU64) -> Self {
        Self {
            every,
            own: 0,
            children: HashMap::new(),
            announced: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.children
            .values()
            .fold(self.own, |sum, spent| sum.saturating_add(*spent))
    }

    /// The alert when this event carries the total past a multiple not yet announced.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<String> {
        match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { usage, .. },
            } => {
                let spent = u64::try_from(usage.total_tokens).unwrap_or(0);
                self.own = self.own.saturating_add(spent);
            }
            AgentEvent::ChildUpdate { update } => {
                let id = update.id.as_str().to_owned();
                let before = self.children.insert(id, update.token_count).unwrap_or(0);
                // A respawn restarts the child's count; what the last incarnation spent stays spent.
                if update.token_count < before {
                    self.own = self.own.saturating_add(before);
                }
            }
            _ => return None,
        }
        let total = self.total();
        let every = self.every.get();
        let reached = total / self.every;
        if reached <= self.announced {
            return None;
        }
        self.announced = reached;
        Some(format!(
            "[spend] this session and its children have used {total} tokens, past the alert at {} (spend.alertTokens {every}); the next alert is at {}.",
            reached.saturating_mul(every),
            reached.saturating_add(1).saturating_mul(every),
        ))
    }

    /// Starts from a session ledger's total; the crossings below it were announced when made.
    pub fn seed(&mut self, total: u64) {
        (self.own, self.announced) = (total, total / self.every);
        self.children.clear();
    }
}

/// Queues each alert as a shown notice, never a wake: an alert must not buy another turn.
pub fn attach(session: &AgentSession, every: NonZeroU64) {
    let alarm = Arc::new(Mutex::new(SpendAlarm::new(every)));
    let (resumed, mut seeded) = (Arc::clone(&alarm), None::<String>);
    // Invariant: a re-attach of the same session (rewind) keeps the live counts; its ledger holds
    // finished children the host publishes again. Another session's store seeds from its own.
    session.on_attach(move |id, _, stats| {
        if seeded.as_deref() != Some(id)
            && let Ok(mut alarm) = resumed.lock()
        {
            alarm.seed(u64::try_from(stats.total_tokens).unwrap_or(0));
            seeded = Some(id.to_owned());
        }
    });
    session.show_notices(SPEND_ALERT_TYPE, move |event| {
        alarm.lock().ok().and_then(|mut alarm| alarm.observe(event))
    });
}

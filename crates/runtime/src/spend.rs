use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, PoisonError};

use yi_types::event::AgentEvent;
use yi_types::message::AgentMessage;

use crate::AgentSession;

pub const SPEND_ALERT_TYPE: &str = "spend_alert";

/// Invariant: arithmetic alone decides an alert, never a model: the session's own turns plus
/// each child's growth, both in `total_tokens`, announced once per multiple of `every` crossed.
pub struct SpendAlarm {
    every: NonZeroU64,
    total: u64,
    /// Each child's owning session and last count: a re-published count adds nothing.
    children: HashMap<String, (String, u64)>,
    session: String,
    announced: u64,
}

impl SpendAlarm {
    pub fn new(every: NonZeroU64) -> Self {
        Self {
            every,
            total: 0,
            children: HashMap::new(),
            session: String::new(),
            announced: 0,
        }
    }

    /// The alert when this event carries the total past a multiple not yet announced.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<String> {
        let grown = match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { usage, .. },
            } => u64::try_from(usage.total_tokens).unwrap_or(0),
            AgentEvent::ChildUpdate { update } => {
                let (owner, last) = self
                    .children
                    .entry(update.id.as_str().to_owned())
                    .or_insert_with(|| (self.session.clone(), 0));
                if *owner != self.session {
                    return None;
                }
                // A respawn restarts the child's count; the last incarnation's spend stays spent.
                let grown = update
                    .token_count
                    .checked_sub(*last)
                    .unwrap_or(update.token_count);
                *last = update.token_count;
                grown
            }
            _ => return None,
        };
        self.total = self.total.saturating_add(grown);
        let total = self.total;
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

    /// Invariant: late, never early, but for one window: a child ending after a switch or failing
    /// reaches no ledger a re-attach reads; one recorded mid-re-attach counts its last step twice.
    pub fn seed(&mut self, session: &str, total: u64) {
        let floor = if self.session == session {
            self.announced
        } else {
            0
        };
        (self.total, self.announced) = (total, floor.max(total / self.every));
        self.session = session.to_owned();
    }
}

/// Queues each alert as a shown notice, never a wake: an alert must not buy another turn.
pub fn attach(session: &AgentSession, every: NonZeroU64) {
    let alarm = Arc::new(Mutex::new(SpendAlarm::new(every)));
    let resumed = Arc::clone(&alarm);
    session.on_attach(move |id, _, stats| {
        let mut alarm = resumed.lock().unwrap_or_else(PoisonError::into_inner);
        alarm.seed(id, u64::try_from(stats.total_tokens).unwrap_or(0));
    });
    session.show_notices(SPEND_ALERT_TYPE, move |event| {
        let mut alarm = alarm.lock().unwrap_or_else(PoisonError::into_inner);
        alarm.observe(event)
    });
}

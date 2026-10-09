use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, PoisonError};

use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage};
use yi_types::record::LaneRecord;

use crate::AgentSession;

/// The one [`yi_types::record::LaneRecord::Usage`] row a call's spend lands on, always on the
/// main lane so the session's cost total cannot undercount it.
pub(crate) fn append_main_usage(
    store: &yi_session::SharedSession,
    cause: String,
    usage: Usage,
) -> Result<(), yi_session::SessionError> {
    let mut session = yi_session::lock_session(store);
    let id = session.next_id();
    session.append_record(LaneRecord::Usage {
        id,
        lane: "main".to_owned(),
        usage,
        cause,
        run_id: None,
        entry_id: None,
        attempt: None,
        stop_reason: None,
        tool_call_id: None,
        details: None,
        seq: 0,
        timestamp: 0,
    })?;
    Ok(())
}

/// Invariant: a side call that reports spend leaves one [`yi_types::record::LaneRecord::Usage`]
/// row; a known zero leaves none.
pub(crate) fn book_side_call(
    store: &yi_session::SharedSession,
    cause: &str,
    usage: Usage,
) -> Result<(), yi_session::SessionError> {
    if usage == Usage::zero() {
        return Ok(());
    }
    append_main_usage(store, cause.to_owned(), usage)
}

pub const SPEND_ALERT_TYPE: &str = "spend_alert";

/// What a [`SpendAlarm`] counts: `total_tokens` (`spend.alertTokens`) or billed dollars
/// (`spend.alertUsd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    Tokens,
    Usd,
}

/// Invariant: arithmetic alone decides an alert, never a model: the session's own turns plus
/// each child's growth, in one [`Measure`], announced once per multiple of `every` crossed.
pub struct SpendAlarm {
    measure: Measure,
    every: f64,
    total: f64,
    /// Each child's owning session and last count: a re-published count adds nothing.
    children: HashMap<String, (String, f64)>,
    session: String,
    announced: u64,
}

impl SpendAlarm {
    pub fn new(every: NonZeroU64) -> Self {
        Self::of(Measure::Tokens, every.get() as f64)
    }

    /// `None` unless `every` is a positive, finite number of dollars.
    pub fn usd(every: f64) -> Option<Self> {
        (every.is_finite() && every > 0.0).then(|| Self::of(Measure::Usd, every))
    }

    fn of(measure: Measure, every: f64) -> Self {
        Self {
            measure,
            every,
            total: 0.0,
            children: HashMap::new(),
            session: String::new(),
            announced: 0,
        }
    }

    fn amount(&self, value: f64) -> String {
        match self.measure {
            Measure::Tokens => format!("{value:.0} tokens"),
            Measure::Usd => format!("${value:.2}"),
        }
    }

    /// The alert when this event carries the total past a multiple not yet announced.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<String> {
        let measure = self.measure;
        let grown = match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { usage, .. },
            } => match measure {
                Measure::Tokens => u64::try_from(usage.total_tokens).unwrap_or(0) as f64,
                Measure::Usd => usage.cost.total.as_f64().unwrap_or(0.0),
            },
            AgentEvent::ChildUpdate { update } => {
                let count = match measure {
                    Measure::Tokens => update.token_count as f64,
                    Measure::Usd => update
                        .cost
                        .as_ref()
                        .and_then(|cost| cost.as_f64())
                        .unwrap_or(0.0),
                };
                let (owner, last) = self
                    .children
                    .entry(update.id.as_str().to_owned())
                    .or_insert_with(|| (self.session.clone(), 0.0));
                if *owner != self.session {
                    return None;
                }
                // A respawn restarts the child's count; the last incarnation's spend stays spent.
                let grown = if count >= *last { count - *last } else { count };
                *last = count;
                grown
            }
            _ => return None,
        };
        self.total += grown.max(0.0);
        let reached = (self.total / self.every) as u64;
        if reached <= self.announced {
            return None;
        }
        self.announced = reached;
        let call = if self.session.is_empty() {
            "yi stats".to_owned()
        } else {
            format!("yi stats {}", self.session)
        };
        let (key, every) = match measure {
            Measure::Tokens => ("alertTokens", format!("{:.0}", self.every)),
            Measure::Usd => ("alertUsd", format!("{}", self.every)),
        };
        Some(format!(
            "[spend] this session and its children have used {}, past the alert at {} (spend.{key} {every}); the next alert is at {}. `{call}` breaks it down by model.",
            self.amount(self.total),
            self.amount(reached as f64 * self.every),
            self.amount(reached.saturating_add(1) as f64 * self.every),
        ))
    }

    /// Invariant: late, never early, but for one window: a child ending after a switch or failing
    /// reaches no ledger a re-attach reads; one recorded mid-re-attach counts its last step twice.
    pub fn seed(&mut self, session: &str, stats: &yi_types::wire::SessionStats) {
        let total = match self.measure {
            Measure::Tokens => u64::try_from(stats.total_tokens).unwrap_or(0) as f64,
            Measure::Usd => stats.cost_total.max(0.0),
        };
        let floor = if self.session == session {
            self.announced
        } else {
            0
        };
        (self.total, self.announced) = (total, floor.max((total / self.every) as u64));
        self.session = session.to_owned();
    }
}

/// Attaches an alarm for each configured key; the reason when `alertUsd` is not a positive amount.
pub fn attach_configured(
    session: &AgentSession,
    spend: Option<&yi_types::config::SpendConfig>,
) -> Option<String> {
    if let Some(every) = spend
        .and_then(|spend| spend.alert_tokens)
        .and_then(NonZeroU64::new)
    {
        attach(session, SpendAlarm::new(every));
    }
    let every = spend?.alert_usd.as_ref()?;
    match every.as_f64().and_then(SpendAlarm::usd) {
        Some(alarm) => {
            attach(session, alarm);
            None
        }
        None => Some(format!(
            "spend.alertUsd {every} is not a positive amount; no dollar alert"
        )),
    }
}

/// Queues each alert as a shown notice, never a wake: an alert must not buy another turn.
pub fn attach(session: &AgentSession, alarm: SpendAlarm) {
    let alarm = Arc::new(Mutex::new(alarm));
    let resumed = Arc::clone(&alarm);
    session.on_attach(move |id, _, stats| {
        let mut alarm = resumed.lock().unwrap_or_else(PoisonError::into_inner);
        alarm.seed(id, stats);
    });
    session.show_notices(SPEND_ALERT_TYPE, move |event| {
        let mut alarm = alarm.lock().unwrap_or_else(PoisonError::into_inner);
        alarm.observe(event)
    });
}

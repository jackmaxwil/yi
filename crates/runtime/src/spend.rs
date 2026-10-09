use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Usage};
use yi_types::record::LaneRecord;

use crate::AgentSession;
use crate::rollup::{DAY_MS, now_ms};

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
/// (`spend.alertUsd`, or `spend.dayAlertUsd` with a [`Day`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measure {
    Tokens,
    Usd,
}

/// A dollar alarm over the local day: the sessions directory its total is seeded from.
struct Day {
    dir: PathBuf,
    ends_ms: u64,
    seeded: bool,
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
    /// A reply in the total came back without usage, so its dollars are a lower bound.
    unpriced: bool,
    day: Option<Day>,
}

impl SpendAlarm {
    pub fn new(every: NonZeroU64) -> Self {
        Self::of(Measure::Tokens, every.get() as f64)
    }

    /// `None` unless `every` is a positive, finite number of dollars.
    pub fn usd(every: f64) -> Option<Self> {
        (every.is_finite() && every > 0.0).then(|| Self::of(Measure::Usd, every))
    }

    /// Like [`SpendAlarm::usd`], over the local day across every session under `dir`.
    pub fn day_usd(every: f64, dir: &Path) -> Option<Self> {
        let mut alarm = Self::usd(every)?;
        alarm.day = Some(Day {
            dir: dir.to_path_buf(),
            ends_ms: 0,
            seeded: false,
        });
        Some(alarm)
    }

    fn of(measure: Measure, every: f64) -> Self {
        Self {
            measure,
            every,
            total: 0.0,
            children: HashMap::new(),
            session: String::new(),
            announced: 0,
            unpriced: false,
            day: None,
        }
    }

    /// Invariant: a process alive across local midnight starts the new day from zero, never
    /// carrying yesterday's total into it, which would alert early.
    fn roll(&mut self, now_ms: u64) {
        if let Some(day) = self.day.as_mut()
            && now_ms >= day.ends_ms
        {
            if day.ends_ms != 0 {
                self.total = 0.0;
                self.announced = 0;
                self.unpriced = false;
            }
            day.ends_ms = crate::rollup::local_midnight_ms(now_ms).saturating_add(DAY_MS);
        }
    }

    /// Invariant: ten $0.10 replies sum to 0.999…9 in binary, so an exact multiple needs the
    /// epsilon to count as crossed.
    fn multiples(&self) -> u64 {
        (self.total / self.every + 1e-9) as u64
    }

    fn amount(&self, value: f64, lower_bound: bool) -> String {
        match self.measure {
            Measure::Tokens => format!("{value:.0} tokens"),
            Measure::Usd => yi_types::message::fmt_cost(value, lower_bound),
        }
    }

    /// The alert when this event carries the total past a multiple not yet announced.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<String> {
        self.observe_at(event, now_ms())
    }

    /// [`SpendAlarm::observe`] with the clock handed in.
    pub fn observe_at(&mut self, event: &AgentEvent, now_ms: u64) -> Option<String> {
        self.roll(now_ms);
        let measure = self.measure;
        let grown = match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { usage, .. },
            } => {
                self.unpriced |= usage.unknown;
                match measure {
                    Measure::Tokens => u64::try_from(usage.total_tokens).unwrap_or(0) as f64,
                    Measure::Usd => usage.cost.total.as_f64().unwrap_or(0.0),
                }
            }
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
        let reached = self.multiples();
        if reached <= self.announced {
            return None;
        }
        self.announced = reached;
        let (key, every) = match (measure, &self.day) {
            (Measure::Tokens, _) => ("alertTokens", format!("{:.0}", self.every)),
            (Measure::Usd, None) => ("alertUsd", format!("{}", self.every)),
            (Measure::Usd, Some(_)) => ("dayAlertUsd", format!("{}", self.every)),
        };
        let (whose, call) = if self.day.is_some() {
            (
                "today's billed spend across every session is",
                "yi stats --since 1d --by model".to_owned(),
            )
        } else if self.session.is_empty() {
            (
                "this session and its children have used",
                "yi stats".to_owned(),
            )
        } else {
            (
                "this session and its children have used",
                format!("yi stats {}", self.session),
            )
        };
        Some(format!(
            "[spend] {whose} {}, past the alert at {} (spend.{key} {every}); the next alert is at {}. `{call}` breaks it down by model.",
            self.amount(self.total, self.unpriced),
            self.amount(reached as f64 * self.every, false),
            self.amount(reached.saturating_add(1) as f64 * self.every, false),
        ))
    }

    /// Invariant: late, never early, but for one window: a child ending after a switch or failing
    /// reaches no ledger a re-attach reads; one recorded mid-re-attach counts its last step twice.
    pub fn seed(&mut self, session: &str, stats: &yi_types::wire::SessionStats) {
        if let Some(day) = self.day.as_mut() {
            // The day's files already hold this session up to now; replies after the seed arrive
            // live, so a later attach keeps the running total instead of starting over from files.
            if !day.seeded {
                let now = now_ms();
                (self.total, self.unpriced) = crate::rollup::spent_today(&day.dir, now);
                day.ends_ms = crate::rollup::local_midnight_ms(now).saturating_add(DAY_MS);
                day.seeded = true;
            }
            self.announced = self.announced.max(self.multiples());
            self.session = session.to_owned();
            return;
        }
        let total = match self.measure {
            Measure::Tokens => u64::try_from(stats.total_tokens).unwrap_or(0) as f64,
            Measure::Usd => stats.cost_total.max(0.0),
        };
        let floor = if self.session == session {
            self.announced
        } else {
            0
        };
        self.total = total;
        self.unpriced = stats.unknown_usage;
        self.announced = floor.max(self.multiples());
        self.session = session.to_owned();
    }
}

/// Attaches an alarm for each configured key; the reasons when a dollar key is not a positive
/// amount. `sessions` is the directory the day alarm sums across.
pub fn attach_configured(
    session: &AgentSession,
    spend: Option<&yi_types::config::SpendConfig>,
    sessions: &Path,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(every) = spend
        .and_then(|spend| spend.alert_tokens)
        .and_then(NonZeroU64::new)
    {
        attach(session, SpendAlarm::new(every));
    }
    let Some(spend) = spend else {
        return warnings;
    };
    for (key, every) in [
        ("alertUsd", &spend.alert_usd),
        ("dayAlertUsd", &spend.day_alert_usd),
    ] {
        let Some(every) = every else { continue };
        let amount = every.as_f64();
        let alarm = if key == "alertUsd" {
            amount.and_then(SpendAlarm::usd)
        } else {
            amount.and_then(|every| SpendAlarm::day_usd(every, sessions))
        };
        match alarm {
            Some(alarm) => attach(session, alarm),
            None => warnings.push(format!(
                "spend.{key} {every} is not a positive amount; no dollar alert"
            )),
        }
    }
    warnings
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

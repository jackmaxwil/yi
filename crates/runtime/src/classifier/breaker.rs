use std::sync::Mutex;
use std::time::{Duration, Instant};

use yi_types::message::{AgentMessage, UserContent};

use super::NOTICE_TYPE;

pub(super) const TRIP_AFTER: u32 = 3;
const PAUSE: Duration = Duration::from_secs(60);

pub(super) struct Breaker {
    failures: u32,
    trips_at: u32,
    paused_until: Option<Instant>,
    /// The failure that paused it, for the one caller that journals why nothing was screened.
    cause: Option<String>,
}

impl Breaker {
    pub(super) const fn new(trips_at: u32) -> Self {
        Self {
            failures: 0,
            trips_at,
            paused_until: None,
            cause: None,
        }
    }
}

/// `None` while the endpoint is open; `Some(cause)` while it is paused, the cause recorded only
/// for a caller that journals it. A poisoned lock answers paused, so the endpoint is not asked.
pub(super) fn paused(breaker: &Mutex<Breaker>) -> Option<Option<String>> {
    let Ok(mut breaker) = breaker.lock() else {
        return Some(None);
    };
    match breaker.paused_until {
        Some(until) if Instant::now() < until => Some(breaker.cause.clone()),
        Some(_) => {
            breaker.paused_until = None;
            breaker.cause = None;
            None
        }
        None => None,
    }
}

pub(super) fn fail(breaker: &Mutex<Breaker>, cause: Option<&str>) -> bool {
    let Ok(mut breaker) = breaker.lock() else {
        return false;
    };
    breaker.failures = breaker.failures.saturating_add(1);
    if breaker.failures < breaker.trips_at {
        return false;
    }
    breaker.failures = 0;
    breaker.paused_until = Instant::now().checked_add(PAUSE);
    breaker.cause = cause.map(str::to_owned);
    true
}

pub(super) fn succeed(breaker: &Mutex<Breaker>) {
    if let Ok(mut breaker) = breaker.lock() {
        breaker.failures = 0;
    }
}

pub(super) fn breaker_notice(url: &str, error: &str, consequence: &str) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: NOTICE_TYPE.to_owned(),
        content: UserContent::Text(format!(
            "The classifier at {url} failed {TRIP_AFTER} times in a row (last: {error}); {consequence} for the next {} s.",
            PAUSE.as_secs()
        )),
        display: true,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

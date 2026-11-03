use std::time::Duration;

/// 408/409/429/5xx retryable, x-should-retry wins, retry-after honored under a hard cap,
/// backoff 0.5s * 2^attempt capped at 8s. No jitter: rand is banned and replays must be stable.
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub max_delay: Duration,
    pub max_total_wall: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_delay: Duration::from_secs(60),
            max_total_wall: Duration::from_secs(300),
        }
    }
}

/// A 429 coding an exhausted quota or balance is final; billing wording without a code counts on
/// Moonshot's host only, since Gemini's per-minute throttle says it too (kimi-errors.ts:1-22).
pub fn quota_exhausted(status: u16, body: &str, url: &str) -> bool {
    const CODES: [&str; 2] = ["exceeded_current_quota_error", "insufficient_quota"];
    const WORDING: [&str; 8] = [
        "exceeded your current quota",
        "exceeded your current token quota",
        "check your account balance",
        "insufficient balance",
        "recharge your account",
        "please recharge",
        "account in arrears",
        "account is in arrears",
    ];
    let parsed = serde_json::from_str::<serde_json::Value>(body).unwrap_or_default();
    let coded = std::iter::successors(Some(&parsed), |level| level.get("error"))
        .take(3)
        .flat_map(|level| [level.get("code"), level.get("type")])
        .any(|code| {
            code.and_then(serde_json::Value::as_str)
                .is_some_and(|code| CODES.contains(&code))
        });
    let lower = body.to_lowercase();
    let worded =
        crate::compat::moonshot_url(url) && WORDING.iter().any(|words| lower.contains(words));
    status == 429 && (coded || worded)
}

pub fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 409 || status == 429 || status >= 500
}

/// The wait the provider asked for: `retry-after-ms` first, then `retry-after` seconds.
pub fn server_wait(retry_after_ms: Option<f64>, retry_after_secs: Option<f64>) -> Option<Duration> {
    let milliseconds =
        retry_after_ms.or_else(|| retry_after_secs.map(|seconds| seconds * 1000.0))?;
    let whole = milliseconds.floor();
    if !whole.is_finite() || whole < 0.0 || whole > u64::MAX as f64 {
        return None;
    }
    Some(Duration::from_micros(
        (whole * 1000.0).min(u64::MAX as f64) as u64
    ))
}

/// 0.5s * 2^attempt, capped at 8s: the wait when the provider named none.
pub fn retry_delay(attempt: u32) -> Duration {
    let exponential = 500.0 * f64::from(2u32.saturating_pow(attempt));
    Duration::from_millis(exponential.min(8000.0) as u64)
}

/// Incident: a `retry-after: 61` was slept out and the same bytes sent again. Past the cap the wait
/// is the provider's window closing, so it comes back as the error and the turn ends naming it.
pub fn wait_for(
    attempt: u32,
    server: Option<Duration>,
    policy: &RetryPolicy,
) -> Result<Duration, Duration> {
    match server {
        Some(wait) if wait > policy.max_delay => Err(wait),
        Some(wait) => Ok(wait),
        None => Ok(retry_delay(attempt)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_caps_at_eight_seconds() {
        assert_eq!(retry_delay(0), Duration::from_millis(500));
        assert_eq!(retry_delay(10), Duration::from_secs(8));
    }

    #[test]
    fn a_server_wait_at_the_cap_is_honored_and_one_past_it_is_refused() {
        let policy = RetryPolicy::default();
        assert_eq!(
            wait_for(0, server_wait(None, Some(60.0)), &policy),
            Ok(Duration::from_secs(60))
        );
        assert_eq!(
            wait_for(0, server_wait(None, Some(61.0)), &policy),
            Err(Duration::from_secs(61))
        );
        assert_eq!(
            wait_for(0, server_wait(Some(120_000.0), None), &policy),
            Err(Duration::from_secs(120))
        );
        assert_eq!(
            wait_for(1, server_wait(None, None), &policy),
            Ok(Duration::from_secs(1))
        );
    }
}

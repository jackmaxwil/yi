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

pub fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 409 || status == 429 || status >= 500
}

pub fn retry_delay(
    attempt: u32,
    retry_after_ms: Option<f64>,
    retry_after_secs: Option<f64>,
    policy: &RetryPolicy,
) -> Option<Duration> {
    let capped = |milliseconds: f64| {
        if milliseconds.is_finite() && milliseconds >= 0.0 {
            let duration = Duration::from_millis(milliseconds as u64);
            (duration <= policy.max_delay).then_some(duration)
        } else {
            None
        }
    };
    if let Some(server) = retry_after_ms.and_then(capped) {
        return Some(server);
    }
    if let Some(server) = retry_after_secs
        .map(|seconds| seconds * 1000.0)
        .and_then(capped)
    {
        return Some(server);
    }
    let exponential = 500.0 * f64::from(2u32.saturating_pow(attempt));
    Some(Duration::from_millis(exponential.min(8000.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_caps_at_eight_seconds() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_delay(0, None, None, &policy),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            retry_delay(10, None, None, &policy),
            Some(Duration::from_secs(8))
        );
    }

    #[test]
    fn server_delay_beyond_cap_is_rejected() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_delay(0, Some(120_000.0), None, &policy),
            Some(Duration::from_millis(500))
        );
    }
}

//! Pacing for a poll with no event to await: a condition that is nearly ready is seen within
//! a millisecond or two, and a slow one is still checked no more often than `cap`.

use std::time::Duration;

/// Each call gives the pause before the next check: 1 ms first, doubling up to `cap`.
pub fn backoff(cap: Duration) -> impl FnMut() -> Duration + Send {
    let mut next = Duration::from_millis(1).min(cap);
    move || {
        let now = next;
        next = now.saturating_mul(2).min(cap);
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_short_and_doubles_to_the_cap() {
        let mut pace = backoff(Duration::from_millis(25));
        let steps: Vec<u128> = (0..7).map(|_| pace().as_millis()).collect();
        assert_eq!(steps, [1, 2, 4, 8, 16, 25, 25]);
    }
}

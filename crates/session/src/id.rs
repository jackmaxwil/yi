use std::hash::{BuildHasher, RandomState};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SessionError;

pub fn now_ms() -> u64 {
    #[expect(
        clippy::disallowed_methods,
        reason = "storage-assigned timestamps are this fn's job; every caller routes through it"
    )]
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub fn age_label(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if days > 0 {
        format!("{days}d ago")
    } else if hours > 0 {
        format!("{hours}h ago")
    } else if minutes > 0 {
        format!("{minutes}m ago")
    } else {
        format!("{seconds}s ago")
    }
}

pub struct IdGenerator {
    seed: RandomState,
    counter: u64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl IdGenerator {
    pub fn new() -> Self {
        Self {
            seed: RandomState::new(),
            counter: 0,
        }
    }

    pub fn next_id(&mut self) -> String {
        self.counter = self.counter.wrapping_add(1);
        let timestamp = now_ms() & 0xffff_ffff_ffff;
        let a = self.seed.hash_one((self.counter, timestamp, 0u8));
        let b = self.seed.hash_one((self.counter, timestamp, 1u8));
        let rand_a = a & 0x0fff;
        let rand_b_high = (b >> 34) & 0x3fff_ffff;
        let rand_b_low = b & 0xffff_ffff;
        format!(
            "{:08x}-{:04x}-7{:03x}-{:04x}-{:04x}{:08x}",
            timestamp >> 16,
            timestamp & 0xffff,
            rand_a,
            0x8000 | (rand_b_high >> 16) as u16 & 0x3fff,
            rand_b_high & 0xffff,
            rand_b_low,
        )
    }
}

pub fn validate_session_id(id: &str) -> Result<(), SessionError> {
    let bytes = id.as_bytes();
    let alnum = |b: u8| b.is_ascii_alphanumeric();
    let inner = |b: u8| alnum(b) || b == b'.' || b == b'_' || b == b'-';
    let valid = match bytes {
        [] => false,
        [only] => alnum(*only),
        [first, middle @ .., last] => {
            alnum(*first) && alnum(*last) && middle.iter().all(|b| inner(*b))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(SessionError::InvalidPayload(
            "Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character".to_owned(),
        ))
    }
}

#[cfg(test)]
mod age_tests {
    use super::age_label;

    #[test]
    fn age_label_uses_the_largest_unit() {
        assert_eq!(age_label(0), "0s ago");
        assert_eq!(age_label(59_999), "59s ago");
        assert_eq!(age_label(60_000), "1m ago");
        assert_eq!(age_label(3_600_000), "1h ago");
        assert_eq!(age_label(86_400_000), "1d ago");
    }
}

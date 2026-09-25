use std::time::{Duration, Instant};

/// 60 fps ceiling (design §17.3): the inline viewport repaints a handful of rows,
/// so 60 is imperceptible from 120 here.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Adaptive floor cap: a draw costing c schedules the next no earlier than
/// start + 2c, capped so a pathological draw cannot freeze the UI.
pub const MAX_FRAME_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Default)]
pub struct FrameScheduler {
    dirty: bool,
    last_emitted_at: Option<Instant>,
    last_start: Option<Instant>,
    last_cost: Duration,
}

impl FrameScheduler {
    pub fn request(&mut self) {
        self.dirty = true;
    }

    /// Dirty, past the 60 fps ceiling, and past the adaptive floor
    /// `last_start + min(2 × last_cost, 200 ms)`.
    pub fn should_draw(&self, now: Instant) -> bool {
        if !self.dirty {
            return false;
        }
        if let Some(last) = self.last_emitted_at
            && now < last + MIN_FRAME_INTERVAL
        {
            return false;
        }
        if let Some(start) = self.last_start {
            let floor = start + (self.last_cost * 2).min(MAX_FRAME_INTERVAL);
            if now < floor {
                return false;
            }
        }
        true
    }

    pub fn mark_drawn(&mut self, start: Instant, end: Instant) {
        self.dirty = false;
        self.last_emitted_at = Some(end);
        self.last_start = Some(start);
        self.last_cost = end.saturating_duration_since(start);
    }

    pub fn poll_timeout(&self, now: Instant) -> Duration {
        if !self.dirty {
            return Duration::from_millis(100);
        }
        let mut deadline = now;
        if let Some(last) = self.last_emitted_at {
            deadline = deadline.max(last + MIN_FRAME_INTERVAL);
        }
        if let Some(start) = self.last_start {
            deadline = deadline.max(start + (self.last_cost * 2).min(MAX_FRAME_INTERVAL));
        }
        deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(1))
    }
}

/// The glyph steps on `elapsed / SPINNER_PERIOD_MS`, so a fixed wake interval
/// beats against that period and the spinner advances unevenly.
pub fn next_spinner_wake(elapsed_ms: u128) -> Duration {
    let period = crate::app::SPINNER_PERIOD_MS;
    let remaining = period.saturating_sub(elapsed_ms % period);
    Duration::from_millis(u64::try_from(remaining).unwrap_or(1).max(1))
}

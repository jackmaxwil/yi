//! Text arrives in a provider's bursts and is shown at a rate proportional to the backlog over
//! the measured burst gap, so the tail decelerates toward the arrival edge instead of jumping.

use std::time::{Duration, Instant};

/// Below this the reveal never slows, so a tail lands instead of trailing off (about 480 wpm).
pub const FLOOR_CPS: f64 = 40.0;
const DRAIN_FLOOR_CPS: f64 = 240.0;
const GAP_MIN_MS: f64 = 150.0;
const GAP_MAX_MS: f64 = 1_200.0;
const GAP_INITIAL_MS: f64 = 400.0;
const GAP_WEIGHT: f64 = 0.3;
/// The most one tick may spend: a late tick after a stall would otherwise reveal a screenful.
const MAX_STEP_MS: f64 = 50.0;
pub const FRAME: Duration = crate::frame::MIN_FRAME_INTERVAL;

/// The cursor between arrived text and painted text for one stream.
#[derive(Debug, Clone)]
pub struct Reveal {
    shown: usize,
    seen: usize,
    last_arrival: Option<Instant>,
    gap_ms: f64,
    budget_ms: f64,
    last_tick: Option<Instant>,
    draining: bool,
}

impl Default for Reveal {
    fn default() -> Self {
        Self {
            shown: 0,
            seen: 0,
            last_arrival: None,
            gap_ms: GAP_INITIAL_MS,
            budget_ms: 0.0,
            last_tick: None,
            draining: false,
        }
    }
}

/// How long a character holds the cursor relative to a letter; the pause after a sentence
/// is the one a reader takes, and it shrinks with the rate when the reveal is behind.
fn weight(ch: char, next: Option<char>) -> f64 {
    let boundary = next.is_none_or(char::is_whitespace);
    match ch {
        '.' | '!' | '?' if boundary => 6.0,
        ',' | ';' | ':' if boundary => 2.5,
        '\n' if next == Some('\n') => 5.0,
        '\n' => 3.0,
        _ => 1.0,
    }
}

/// A character the cursor never stops before: it composes with the one behind it, or it is a
/// markdown delimiter whose run styles as a unit.
fn glued(ch: char) -> bool {
    matches!(ch, '\u{0300}'..='\u{036F}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}')
        || matches!(ch, '*' | '_' | '`' | '~')
}

impl Reveal {
    pub fn shown(&self) -> usize {
        self.shown
    }

    pub fn behind(&self, len: usize) -> bool {
        self.shown < len
    }

    /// Everything up to `len` is shown at once: the thought's answer has started under it.
    pub fn snap(&mut self, len: usize) {
        self.shown = self.shown.max(len);
        self.budget_ms = 0.0;
    }

    /// The message ended: what is left finishes at the short horizon, still paced.
    pub fn drain(&mut self) {
        self.draining = true;
    }

    /// A new message; the burst gap learned from the last one carries over.
    pub fn reset(&mut self) {
        let gap_ms = self.gap_ms;
        *self = Self {
            gap_ms,
            ..Self::default()
        };
    }

    /// Records a snapshot of `len` bytes; growth since the last one is one burst.
    pub fn on_arrival(&mut self, len: usize, now: Instant) {
        if len > self.seen {
            if let Some(previous) = self.last_arrival {
                let gap = now.saturating_duration_since(previous).as_secs_f64() * 1_000.0;
                let gap = gap.clamp(GAP_MIN_MS, GAP_MAX_MS);
                self.gap_ms += GAP_WEIGHT * (gap - self.gap_ms);
            }
            self.last_arrival = Some(now);
            self.seen = len;
        }
        self.shown = self.shown.min(len);
    }

    /// Characters per second for `backlog` characters: the backlog over one gap, so the rate
    /// decays with what is left and the lag settles at about one burst.
    fn rate_cps(&self, backlog: f64, pace: u16) -> f64 {
        let (horizon_ms, floor) = if self.draining {
            (GAP_MIN_MS, DRAIN_FLOOR_CPS)
        } else {
            (self.gap_ms, FLOOR_CPS)
        };
        (backlog * 1_000.0 / horizon_ms).max(floor) * f64::from(pace) / 100.0
    }

    /// Moves the cursor by the time since the last tick; true when it moved. `pace` is a
    /// percentage of the default speed and 0 shows everything at once.
    pub fn advance(&mut self, text: &str, now: Instant, pace: u16) -> bool {
        let len = text.len();
        let elapsed = self
            .last_tick
            .map(|last| now.saturating_duration_since(last).as_secs_f64() * 1_000.0)
            .unwrap_or(0.0);
        self.last_tick = Some(now);
        if self.shown >= len {
            self.budget_ms = 0.0;
            return false;
        }
        if pace == 0 {
            self.shown = len;
            return true;
        }
        self.budget_ms += elapsed.min(MAX_STEP_MS);
        let tail = text.get(self.shown..).unwrap_or_default();
        let per_char_ms = 1_000.0 / self.rate_cps(tail.chars().count() as f64, pace);
        let start = self.shown;
        let mut chars = tail.chars().peekable();
        while let Some(ch) = chars.next() {
            let cost = per_char_ms * weight(ch, chars.peek().copied());
            if self.budget_ms < cost {
                break;
            }
            self.budget_ms -= cost;
            self.shown += ch.len_utf8();
        }
        if self.shown > start {
            while let Some(ch) = text.get(self.shown..).and_then(|rest| rest.chars().next())
                && glued(ch)
            {
                self.shown += ch.len_utf8();
            }
        }
        if self.shown >= len {
            self.budget_ms = 0.0;
        }
        self.shown > start
    }
}

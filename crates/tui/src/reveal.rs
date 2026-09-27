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
    waiting: bool,
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
            waiting: false,
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

/// A character the cursor never stops before: it composes with the one behind it.
fn glued(ch: char) -> bool {
    matches!(ch, '\u{0300}'..='\u{036F}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}')
}

/// Where the cursor may next stop inside a line: a delimiter run is one unit and an opener
/// takes its first letter; at the arrival edge a run waits.
fn mid_unit(text: &str, at: usize, draining: bool) -> Option<usize> {
    let rest = text.get(at..)?;
    let ch = rest.chars().next()?;
    if !matches!(ch, '*' | '_' | '`' | '~') {
        return Some(at + ch.len_utf8());
    }
    let end = at + rest.len() - rest.trim_start_matches(ch).len();
    let Some(next) = text.get(end..).and_then(|after| after.chars().next()) else {
        return draining.then_some(text.len());
    };
    let opens = text
        .get(..at)
        .and_then(|before| before.chars().next_back())
        .is_none_or(char::is_whitespace)
        && !next.is_whitespace();
    Some(if opens { end + next.len_utf8() } else { end })
}

fn is_table_row(line: &str) -> bool {
    line.trim_start_matches([' ', '\t']).starts_with('|')
}

/// Incident: `1. one\n2` drew `2` on the row above until `. t` arrived. From a line start the
/// cursor holds (`None`) until a marker's first governed character is there.
fn line_unit(text: &str, at: usize, draining: bool) -> Option<usize> {
    let rest = text.get(at..).unwrap_or_default();
    let complete = |lines: usize| {
        rest.match_indices('\n')
            .nth(lines - 1)
            .map(|(end, _)| at + end + 1)
            .or_else(|| draining.then_some(text.len()))
    };
    // A table's first row reads as a paragraph until its rule row lands under it.
    if is_table_row(rest) {
        let head = at == 0
            || !is_table_row(
                text.get(..at - 1)
                    .and_then(|before| before.rsplit('\n').next())
                    .unwrap_or_default(),
            );
        return complete(if head { 2 } else { 1 });
    }
    let marker = rest
        .find(|ch| !crate::markdown::lead(ch))
        .unwrap_or(rest.len());
    match rest.get(marker..).and_then(|body| body.chars().next()) {
        Some(first) => Some(at + marker + first.len_utf8()),
        None => draining.then_some(text.len()),
    }
}

impl Reveal {
    pub fn shown(&self) -> usize {
        self.shown
    }

    pub fn behind(&self, len: usize) -> bool {
        self.shown < len
    }

    pub fn waiting(&self) -> bool {
        self.waiting
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
        self.waiting = false;
        let tail = text.get(self.shown..).unwrap_or_default();
        let per_char_ms = 1_000.0 / self.rate_cps(tail.chars().count() as f64, pace);
        let start = self.shown;
        while self.shown < len {
            let at = self.shown;
            let end = if at == 0 || text.as_bytes().get(at - 1) == Some(&b'\n') {
                match line_unit(text, at, self.draining) {
                    Some(end) => end,
                    None => {
                        self.waiting = true;
                        break;
                    }
                }
            } else {
                match mid_unit(text, at, self.draining) {
                    Some(end) => end,
                    None => {
                        self.waiting = true;
                        break;
                    }
                }
            };
            let unit = text.get(at..end).unwrap_or_default();
            let mut chars = unit.chars().peekable();
            let mut cost = 0.0;
            while let Some(ch) = chars.next() {
                let next = chars
                    .peek()
                    .copied()
                    .or_else(|| text.get(end..)?.chars().next());
                cost += per_char_ms * weight(ch, next);
            }
            if self.budget_ms < cost {
                break;
            }
            self.budget_ms -= cost;
            self.shown = end;
        }
        if self.shown > start && text.as_bytes().get(self.shown - 1) != Some(&b'\n') {
            while let Some(ch) = text.get(self.shown..).and_then(|rest| rest.chars().next())
                && glued(ch)
            {
                self.shown += ch.len_utf8();
            }
        }
        // A wait banks no time: the text after it would otherwise land in one frame.
        if self.shown >= len || self.waiting {
            self.budget_ms = self
                .budget_ms
                .min(if self.waiting { MAX_STEP_MS } else { 0.0 });
        }
        self.shown > start
    }
}

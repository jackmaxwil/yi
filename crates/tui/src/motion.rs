use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::colors::{ColorTier, Theme};

/// Invariant: every animated glyph derives from one process-relative elapsed,
/// so the screen pulses in step. Two cadences, not two clocks: braille at 80 ms
/// for a call in flight, this for agent work (prime-agent `working-icon.ts`).
pub const PULSE_PERIOD_MS: u128 = 250;
pub const PULSE_FRAMES: [char; 4] = ['◇', '◈', '◆', '◈'];

/// The clock's own resolution: the frame scheduler wakes on this boundary while
/// a spinner is live, so every animation here is quantised to it and a dwell
/// shorter than one tick renders as one tick.
pub const TICK_MS: u128 = 80;

/// The app counts ticks; every function here takes milliseconds, so the one
/// clock stays one clock rather than becoming a second counter per surface.
pub fn elapsed_of(phase: usize) -> u128 {
    (phase as u128).saturating_mul(TICK_MS)
}

/// OMP's starburst. Every glyph is one cell wide — a width change would reflow
/// the row on every frame and jitter whatever trails it.
pub const THINKING_FRAMES: [char; 8] = ['✻', '✼', '❉', '❊', '✺', '✹', '✸', '✶'];
const BREATH_FAST_MS: f64 = 70.0;
const BREATH_SLOW_MS: f64 = 230.0;

/// codex `shimmer.rs:26-34`: a band swept across the text, phase taken from the
/// process clock so two shimmering rows are never out of step.
const SWEEP_MS: f64 = 2_000.0;
const BAND: f64 = 5.0;
const PAD: f64 = 10.0;

const STRIKE_FRAMES: u64 = 12;

pub fn pulse_frame(elapsed_ms: u128) -> char {
    let step = usize::try_from(elapsed_ms / PULSE_PERIOD_MS).unwrap_or(0);
    PULSE_FRAMES
        .get(step % PULSE_FRAMES.len())
        .copied()
        .unwrap_or('◆')
}

/// A raised-cosine breath: the cycle accelerates and slows rather than ticking,
/// so the dwell of frame `n` is not the dwell of frame `n + 1`.
fn breath_dwell(index: usize) -> f64 {
    let turn = (index as f64) / (THINKING_FRAMES.len() as f64);
    let eased = 0.5 * (1.0 - (turn * std::f64::consts::TAU).cos());
    BREATH_FAST_MS + (BREATH_SLOW_MS - BREATH_FAST_MS) * eased
}

/// The glyph whose dwell window `elapsed_ms` falls into. Walking the cycle is
/// what makes the dwell uneven; a division would make it uniform again.
pub fn thinking_glyph(elapsed_ms: u128) -> char {
    let cycle: f64 = (0..THINKING_FRAMES.len()).map(breath_dwell).sum();
    let mut into = (elapsed_ms as f64) % cycle.max(1.0);
    for (index, glyph) in THINKING_FRAMES.iter().enumerate() {
        let dwell = breath_dwell(index);
        if into < dwell {
            return *glyph;
        }
        into -= dwell;
    }
    THINKING_FRAMES.first().copied().unwrap_or('✻')
}

/// The band's weight at `column`, 0 outside it. codex `shimmer.rs:44-50`.
fn weight(column: f64, head: f64) -> f64 {
    let distance = (column - head).abs();
    if distance > BAND {
        return 0.0;
    }
    0.5 * (1.0 + (std::f64::consts::PI * distance / BAND).cos())
}

fn blend(from: Color, to: Color, alpha: f64) -> Color {
    let mix = |a: u8, b: u8| {
        let value = f64::from(a) + (f64::from(b) - f64::from(a)) * alpha;
        value.clamp(0.0, 255.0) as u8
    };
    match (from, to) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ => to,
    }
}

/// Text with a band sweeping across it, interpolating toward the theme's own
/// text colour so it reads on any ground; below truecolor the same weight
/// quantises into three attributes (codex `shimmer.rs:70-78`).
pub fn shimmer(text: &str, elapsed_ms: u128, theme: &Theme) -> Vec<Span<'static>> {
    let count = text.chars().count();
    if count == 0 {
        return Vec::new();
    }
    let period = (count as f64) + 2.0 * PAD;
    let head = ((elapsed_ms as f64) / SWEEP_MS * period) % period - PAD;
    let base = theme.dim_style();
    let mut out: Vec<Span<'static>> = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        let t = weight(index as f64, head);
        let style = match theme.tier {
            ColorTier::TrueColor => {
                let lit = blend(theme.dim, theme.text, t * 0.9);
                if t > 0.0 {
                    Style::default().fg(lit).add_modifier(Modifier::BOLD)
                } else {
                    base
                }
            }
            _ if t < 0.2 => base,
            _ if t < 0.6 => Style::default().fg(theme.muted),
            _ => Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        };
        // Runs of one style coalesce, so a row emits a handful of spans rather
        // than one per character (OMP `theme/shimmer.ts`).
        match out.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push(ch),
            _ => out.push(Span::styled(ch.to_string(), style)),
        }
    }
    out
}

/// A completed row struck through left to right over 12 frames, then settled.
/// `None` once the sweep is done, so the caller renders its normal done style.
pub fn strike_sweep(label: &str, since_ms: u64) -> Option<(String, String)> {
    let frame = since_ms / (PULSE_PERIOD_MS as u64 / 4).max(1);
    if frame >= STRIKE_FRAMES {
        return None;
    }
    let count = label.chars().count() as u64;
    let cut = usize::try_from(count.saturating_mul(frame) / STRIKE_FRAMES).unwrap_or(0);
    let struck: String = label.chars().take(cut).collect();
    let rest: String = label.chars().skip(cut).collect();
    Some((struck, rest))
}

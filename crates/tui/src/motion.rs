use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::colors::{ColorTier, Theme};

/// Invariant: one elapsed for every animated glyph, so the screen pulses in
/// step. Braille at `TICK_MS` is a call in flight; this is agent work.
pub const PULSE_PERIOD_MS: u128 = 250;
pub const PULSE_FRAMES: [char; 4] = ['◇', '◈', '◆', '◈'];

/// The frame scheduler's resolution: a shorter dwell renders as one tick.
pub const TICK_MS: u128 = 80;

pub fn elapsed_of(phase: usize) -> u128 {
    u128::try_from(phase).unwrap_or(0).saturating_mul(TICK_MS)
}

/// Starburst. Every glyph is one cell wide — a width change would reflow the
/// row on every frame.
pub const THINKING_FRAMES: [char; 8] = ['✻', '✼', '❉', '❊', '✺', '✹', '✸', '✶'];

/// How long each frame holds: the cycle accelerates and slows rather than
/// ticking, so the dwell of frame `n` is not the dwell of frame `n + 1`.
const DWELL_MS: [u128; 8] = [70, 110, 190, 230, 230, 190, 110, 70];
pub const BREATH_CYCLE_MS: u128 = 1_200;

/// A band swept across the text, phase taken from the process clock so two
/// shimmering rows are never out of step.
const SWEEP_MS: u128 = 2_000;
const BAND: f64 = 5.0;
const PAD: f64 = 10.0;
const FULL: u16 = 255;

const STRIKE_FRAMES: u64 = 12;
const STRIKE_STEP_MS: u64 = 62;

pub fn pulse_frame(elapsed_ms: u128) -> char {
    let step = usize::try_from(elapsed_ms / PULSE_PERIOD_MS).unwrap_or(0);
    PULSE_FRAMES
        .get(step % PULSE_FRAMES.len())
        .copied()
        .unwrap_or('◆')
}

/// The glyph whose dwell window `elapsed_ms` falls into.
pub fn thinking_glyph(elapsed_ms: u128) -> char {
    let mut into = elapsed_ms % BREATH_CYCLE_MS;
    for (index, glyph) in THINKING_FRAMES.iter().enumerate() {
        let dwell = DWELL_MS.get(index).copied().unwrap_or(TICK_MS);
        if into < dwell {
            return *glyph;
        }
        into = into.saturating_sub(dwell);
    }
    THINKING_FRAMES.first().copied().unwrap_or('✻')
}

/// The band's weight at `column` as an alpha in `0..=FULL`, 0 outside it — the
/// one place a cosine becomes an integer.
fn weight(column: f64, head: f64) -> u16 {
    let distance = (column - head).abs();
    if distance > BAND {
        return 0;
    }
    let raw = 0.5 * (1.0 + (std::f64::consts::PI * distance / BAND).cos());
    (raw * f64::from(FULL)).round().clamp(0.0, f64::from(FULL)) as u16
}

fn blend(from: Color, to: Color, alpha: u16) -> Color {
    let alpha = alpha.min(FULL);
    let mix = |a: u8, b: u8| {
        let far = u16::from(a).saturating_mul(FULL.saturating_sub(alpha));
        let near = u16::from(b).saturating_mul(alpha);
        u8::try_from(far.saturating_add(near) / FULL).unwrap_or(u8::MAX)
    };
    match (from, to) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ => to,
    }
}

fn as_f64(value: u128) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

/// Text with a band sweeping across it, interpolating toward the theme's own text colour so
/// it reads on any ground; below truecolor the same weight quantises into three attributes.
pub fn shimmer(text: &str, elapsed_ms: u128, theme: &Theme) -> Vec<Span<'static>> {
    let count = text.chars().count();
    if count == 0 {
        return Vec::new();
    }
    let period = as_f64(u128::try_from(count).unwrap_or(0)) + 2.0 * PAD;
    let head = as_f64(elapsed_ms % SWEEP_MS) / as_f64(SWEEP_MS) * period - PAD;
    let base = theme.dim_style();
    let mut out: Vec<Span<'static>> = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        let alpha = weight(as_f64(u128::try_from(index).unwrap_or(0)), head);
        let style = match theme.tier {
            ColorTier::TrueColor if alpha == 0 => base,
            ColorTier::TrueColor => Style::default()
                .fg(blend(theme.dim, theme.text, alpha.saturating_mul(9) / 10))
                .add_modifier(Modifier::BOLD),
            _ if alpha < FULL / 5 => base,
            _ if alpha < FULL * 3 / 5 => Style::default().fg(theme.muted),
            _ => Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        };
        // Runs of one style coalesce, so a row emits a handful of spans rather
        // than one per character.
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
    let frame = since_ms / STRIKE_STEP_MS;
    if frame >= STRIKE_FRAMES {
        return None;
    }
    let count = u64::try_from(label.chars().count()).unwrap_or(0);
    let cut = usize::try_from(count.saturating_mul(frame) / STRIKE_FRAMES).unwrap_or(0);
    let struck: String = label.chars().take(cut).collect();
    let rest: String = label.chars().skip(cut).collect();
    Some((struck, rest))
}

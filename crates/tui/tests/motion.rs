use std::collections::HashSet;
use std::error::Error;

use ratatui::style::Modifier;
use unicode_width::UnicodeWidthChar;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::motion::{
    BREATH_CYCLE_MS, PULSE_FRAMES, THINKING_FRAMES, elapsed_of, pulse_frame, shimmer, strike_sweep,
    thinking_glyph,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Invariant: one clock. Two surfaces asked at the same instant must agree, or
/// the screen pulses out of step and reads as several unrelated animations.
#[test]
fn every_surface_asked_at_one_instant_shows_one_frame() -> TestResult {
    for phase in 0..40 {
        let at = elapsed_of(phase);
        assert_eq!(pulse_frame(at), pulse_frame(at));
        assert_eq!(thinking_glyph(at), thinking_glyph(at));
    }
    // The diamond is the slower cadence: it must not advance every tick.
    let frames: Vec<char> = (0..4).map(|p| pulse_frame(elapsed_of(p))).collect();
    assert!(
        frames.windows(2).any(|pair| pair[0] == pair[1]),
        "the 250 ms cadence holds across an 80 ms tick: {frames:?}"
    );
    Ok(())
}

/// A width change on an animated glyph reflows the row under it on every frame.
#[test]
fn every_animated_glyph_is_one_cell_wide() -> TestResult {
    for glyph in THINKING_FRAMES.iter().chain(PULSE_FRAMES.iter()) {
        assert_eq!(
            UnicodeWidthChar::width(*glyph),
            Some(1),
            "{glyph} would reflow the row"
        );
    }
    Ok(())
}

/// The breath is the point: a uniform dwell is a tick, and reads as one.
#[test]
fn the_thinking_cycle_breathes_rather_than_ticking() -> TestResult {
    let mut seen: Vec<char> = Vec::new();
    for ms in (0..1_200).step_by(10) {
        let glyph = thinking_glyph(ms);
        if seen.last() != Some(&glyph) {
            seen.push(glyph);
        }
    }
    let distinct: HashSet<char> = seen.iter().copied().collect();
    assert!(distinct.len() >= 4, "the cycle advances: {seen:?}");

    // Dwell lengths differ across the cycle; equal ones would be a plain tick.
    let mut dwells: Vec<u32> = Vec::new();
    let mut current = thinking_glyph(0);
    let mut run = 0_u32;
    for ms in 0..1_000 {
        let glyph = thinking_glyph(ms);
        if glyph == current {
            run += 1;
        } else {
            dwells.push(run);
            current = glyph;
            run = 1;
        }
    }
    let shortest = dwells.iter().min().copied().unwrap_or(0);
    let longest = dwells.iter().max().copied().unwrap_or(0);
    assert!(longest > shortest, "dwell varies: {dwells:?}");
    Ok(())
}

/// The band has to move, and it has to be a band — a fully lit or fully dim row
/// is not a sweep.
#[test]
fn the_shimmer_band_moves_across_the_text() -> TestResult {
    let theme = Theme::new(ColorTier::TrueColor, true);
    let text = "Reading crates/tui/src/cell.rs";
    let lit = |at: u128| -> usize {
        shimmer(text, at, &theme)
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
            .map(|span| span.content.chars().count())
            .sum()
    };
    let early = lit(0);
    let mid = lit(900);
    assert!(early != mid, "the band moved: {early} vs {mid}");
    assert!(
        mid < text.chars().count(),
        "a band, not a fully lit row: {mid}"
    );

    // The text itself is never changed by the sweep.
    let rebuilt: String = shimmer(text, 500, &theme)
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(rebuilt, text);
    Ok(())
}

/// A truecolor gradient cannot exist at 16 colours; the same weight has to
/// degrade into attributes the terminal actually has.
#[test]
fn the_shimmer_degrades_to_attributes_at_sixteen_colours() -> TestResult {
    let theme = Theme::new(ColorTier::Ansi16, true);
    let spans = shimmer("Working…", 400, &theme);
    assert!(!spans.is_empty());
    let rebuilt: String = spans.iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(rebuilt, "Working…");
    Ok(())
}

/// The strike sweeps in and then settles; a sweep that never ends would repaint
/// a finished row forever.
#[test]
fn the_strike_sweeps_then_settles() -> TestResult {
    let label = "☑ scout: read the tree";
    let (struck, rest) = strike_sweep(label, 0).ok_or("no sweep at the start")?;
    assert!(
        struck.is_empty(),
        "the sweep starts at the left: {struck:?}"
    );
    assert_eq!(format!("{struck}{rest}"), label);

    let (mid_struck, mid_rest) = strike_sweep(label, 120).ok_or("no sweep mid-way")?;
    assert!(!mid_struck.is_empty() && !mid_rest.is_empty());
    assert_eq!(format!("{mid_struck}{mid_rest}"), label);

    assert!(
        strike_sweep(label, 60_000).is_none(),
        "a finished sweep hands the row back to its normal style"
    );
    Ok(())
}

/// The dwell table and the cycle length are two constants that have to agree:
/// a cycle longer than the table's sum parks on the last glyph, and a shorter
/// one clips frames off the end. Both read as a stutter, not a breath.
#[test]
fn the_breath_cycle_closes_on_its_own_table() -> TestResult {
    assert_eq!(
        thinking_glyph(0),
        thinking_glyph(BREATH_CYCLE_MS),
        "the cycle wraps to its first frame"
    );
    let seen: HashSet<char> = (0..BREATH_CYCLE_MS).map(thinking_glyph).collect();
    assert_eq!(
        seen.len(),
        THINKING_FRAMES.len(),
        "every frame is reached inside one cycle: {seen:?}"
    );
    Ok(())
}

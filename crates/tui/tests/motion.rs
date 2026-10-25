use std::error::Error;

use ratatui::style::Modifier;
use unicode_width::UnicodeWidthChar;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::motion::{PULSE_FRAMES, elapsed_of, pulse_frame, shimmer};

type TestResult = Result<(), Box<dyn Error>>;

/// The diamond is the slower cadence: it must not advance every tick.
#[test]
fn the_pulse_holds_a_frame_across_an_80_ms_tick() -> TestResult {
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
    for glyph in PULSE_FRAMES.iter() {
        assert_eq!(
            UnicodeWidthChar::width(*glyph),
            Some(1),
            "{glyph} would reflow the row"
        );
    }
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

use std::error::Error;
use std::time::{Duration, Instant};

use yi_tui::reveal::{FRAME, Reveal};

type TestResult = Result<(), Box<dyn Error>>;

const PACE: u16 = 100;

/// One burst arrives at `t0`, then the clock steps a frame at a time; returns the cursor after
/// each step so a test can read the shape of the curve.
fn curve(text: &str, frames: usize) -> Vec<usize> {
    let t0 = Instant::now();
    let mut reveal = Reveal::default();
    reveal.on_arrival(text.len(), t0);
    reveal.advance(text, t0, PACE);
    (1..=frames)
        .map(|frame| {
            let now = t0 + FRAME * u32::try_from(frame).unwrap_or(u32::MAX);
            reveal.advance(text, now, PACE);
            reveal.shown()
        })
        .collect()
}

#[test]
fn a_burst_decelerates_and_still_lands() -> TestResult {
    let text = "word ".repeat(80);
    let shown = curve(&text, 600);
    assert!(shown.windows(2).all(|pair| pair[0] <= pair[1]), "monotone");
    assert!(
        shown.iter().all(|&at| at <= text.len()),
        "never past the text"
    );
    let quarter = shown.get(24).copied().ok_or("no 25th frame")?;
    let half = shown.get(49).copied().ok_or("no 50th frame")?;
    assert!(
        quarter > half - quarter,
        "the first 400 ms reveal more than the next 400 ms: {quarter} vs {}",
        half - quarter
    );
    assert!(quarter > 0, "the first frames already show something");
    assert_eq!(
        shown.last().copied(),
        Some(text.len()),
        "the floor lands the tail"
    );
    Ok(())
}

#[test]
fn a_sentence_end_holds_the_cursor_longer_than_a_letter() -> TestResult {
    let plain = "abcdef".repeat(20);
    let dotted = "ab. cd".repeat(20);
    let plain_at = curve(&plain, 30).last().copied().ok_or("no frame")?;
    let dotted_at = curve(&dotted, 30).last().copied().ok_or("no frame")?;
    assert!(
        plain_at > dotted_at,
        "same budget, fewer characters through the full stops: {plain_at} vs {dotted_at}"
    );
    Ok(())
}

#[test]
fn drain_finishes_within_a_second_and_pace_zero_snaps() -> TestResult {
    let text = "word ".repeat(80);
    let t0 = Instant::now();
    let mut reveal = Reveal::default();
    reveal.on_arrival(text.len(), t0);
    reveal.advance(&text, t0, PACE);
    reveal.drain();
    let mut frame = 0_u32;
    while reveal.behind(text.len()) {
        frame += 1;
        assert!(frame < 63, "still behind after {frame} frames");
        reveal.advance(&text, t0 + FRAME * frame, PACE);
    }

    let mut instant = Reveal::default();
    instant.on_arrival(text.len(), t0);
    assert!(instant.advance(&text, t0, 0));
    assert!(!instant.behind(text.len()));
    Ok(())
}

#[test]
fn the_cursor_never_splits_a_grapheme_or_a_delimiter_run() -> TestResult {
    let text = "e\u{301}x**bold**y".repeat(40);
    let inside_accent: Vec<usize> = text.match_indices('\u{301}').map(|(at, _)| at).collect();
    let inside_stars: Vec<usize> = text.match_indices("**").map(|(at, _)| at + 1).collect();
    for at in curve(&text, 400) {
        assert!(
            !inside_accent.contains(&at),
            "stopped before a combining mark at {at}"
        );
        assert!(
            !inside_stars.contains(&at),
            "stopped between two asterisks at {at}"
        );
    }
    Ok(())
}

#[test]
fn a_late_tick_reveals_one_step_not_a_screenful() -> TestResult {
    let text = "word ".repeat(400);
    let t0 = Instant::now();
    let mut reveal = Reveal::default();
    reveal.on_arrival(text.len(), t0);
    reveal.advance(&text, t0, PACE);
    reveal.advance(&text, t0 + Duration::from_secs(5), PACE);
    assert!(
        reveal.shown() < text.len() / 4,
        "a five second stall spent one clamped step: {}",
        reveal.shown()
    );
    Ok(())
}

use crate::common;

fn app_with_pace(pace: u16) -> yi_tui::app::App {
    yi_tui::app::App::new(
        yi_tui::TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "reveal".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace,
        },
        yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true),
        yi_tui::keymap::default_keymap(),
        80,
    )
}

fn update(text: &str) -> yi_types::event::AgentEvent {
    let message = yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(text)],
        yi_types::message::StopReason::Stop,
    );
    yi_types::event::AgentEvent::MessageUpdate {
        // A delta is a delta (D145): a test hands the reducer a whole message as `Done`.
        assistant_message_event: yi_types::event::AssistantMessageEvent::Done {
            reason: yi_types::message::StopReason::Stop,
            message,
        },
    }
}

/// Invariant: a paragraph the reader has not seen yet does not commit to scrollback, however
/// stable its markdown is; at pace 0 the cursor sits on the arrival edge and it commits at once.
#[test]
fn a_stable_paragraph_commits_only_once_it_is_shown() -> TestResult {
    let two = format!("{}\n\nSecond paragraph.", "First paragraph. ".repeat(10));
    let mut instant = app_with_pace(0);
    instant.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    instant.reduce_agent(update(&two));
    assert!(
        !instant.take_commits().is_empty(),
        "pace 0 commits on arrival"
    );

    let mut paced = app_with_pace(100);
    paced.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    paced.reduce_agent(update(&two));
    assert!(
        paced.take_commits().is_empty(),
        "nothing is shown yet, so nothing commits"
    );
    paced.step_reveal(Instant::now() + Duration::from_millis(16));
    assert!(
        paced.take_commits().is_empty(),
        "one frame in, the paragraph is still live"
    );
    let mut now = Instant::now();
    for _ in 0..600 {
        now += FRAME;
        paced.step_reveal(now);
    }
    assert!(
        !paced.take_commits().is_empty(),
        "once shown, the stable paragraph commits"
    );
    Ok(())
}

/// Invariant: the end of a message commits every character, however far the cursor was
/// behind when the end arrived; the paced path and the instant path leave the same scrollback.
#[test]
fn the_end_of_a_message_commits_the_whole_text() -> TestResult {
    let text = "faux: hello from yi";
    let end = yi_types::event::AgentEvent::MessageEnd {
        message: yi_runtime::faux::faux_assistant_message(
            vec![yi_runtime::faux::faux_text(text)],
            yi_types::message::StopReason::Stop,
        ),
    };
    let mut paced = app_with_pace(100);
    paced.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    paced.reduce_agent(update(text));
    let mut now = Instant::now();
    for _ in 0..20 {
        now += FRAME;
        paced.step_reveal(now);
    }
    paced.reduce_agent(end);
    let committed: String = paced
        .take_commits()
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        committed.contains("hello from yi"),
        "committed:\n{committed}"
    );
    Ok(())
}

/// Where the cursor rests once `text` has had a second to reveal.
fn settled(text: &str, draining: bool) -> usize {
    let t0 = Instant::now();
    let mut reveal = Reveal::default();
    reveal.on_arrival(text.len(), t0);
    if draining {
        reveal.drain();
    }
    let mut now = t0;
    while now <= t0 + Duration::from_secs(1) {
        reveal.advance(text, now, PACE);
        now += FRAME;
    }
    reveal.shown()
}

/// Incident: `2` drew at the end of the row above until `. t` arrived, then jumped to its own
/// row; a line's marker waits for the first character it governs.
#[test]
fn a_line_marker_shows_with_its_first_word() {
    for (text, rests) in [
        ("1. one\n2", "1. one\n"),
        ("1. one\n2. ", "1. one\n"),
        ("para\n-", "para\n"),
        ("para\n\n##", "para\n\n"),
        ("code\n```", "code\n"),
        ("| a | b |\n", ""),
        ("| a | b |\n|---|---|\n| 1", "| a | b |\n|---|---|\n"),
    ] {
        assert_eq!(settled(text, false), rests.len(), "{text:?}");
    }
    let next = "1. one\n2. two";
    assert_eq!(settled(next, false), next.len());
    assert_eq!(
        settled("para\n-", true),
        "para\n-".len(),
        "the end of the message releases a hold"
    );
}

/// An opening `**` shows with the letter it styles, and at the arrival edge a run waits.
#[test]
fn a_delimiter_run_waits_for_what_it_styles() {
    assert_eq!(settled("text **", false), "text ".len());
    assert_eq!(settled("text **b", false), "text **b".len());
    assert_eq!(settled("text **", true), "text **".len());
}

/// A wait at the arrival edge banks no time: the text after it reveals at the rate, not in
/// one frame.
#[test]
fn a_hold_banks_no_budget() {
    let t0 = Instant::now();
    let mut reveal = Reveal::default();
    let held = "text **";
    reveal.on_arrival(held.len(), t0);
    for frame in 0..=120 {
        reveal.advance(held, t0 + FRAME * frame, PACE);
    }
    let full = format!("text **bold** {}", "word ".repeat(40));
    let later = t0 + FRAME * 121;
    reveal.on_arrival(full.len(), later);
    reveal.advance(&full, later, PACE);
    assert!(
        reveal.shown() < 40,
        "revealed {} of {} in one frame",
        reveal.shown(),
        full.len()
    );
}

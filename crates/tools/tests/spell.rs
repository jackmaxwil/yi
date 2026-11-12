use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use yi_tools::spell::Answer;
use yi_tools::spell::{Ask, DEADLINE, Speller};

#[cfg(target_os = "macos")]
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Asks until the answer lands, giving the peer its own deadline and no more.
#[cfg(target_os = "macos")]
fn answer(speller: &mut Speller, ask: Ask, text: &str) -> Option<Answer> {
    let until = Instant::now() + DEADLINE * 3;
    while Instant::now() < until {
        speller.poll(Instant::now());
        if let Some(found) = speller.ask(ask, text, Instant::now()) {
            return Some(found.clone());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// Dies with the old helper's desync: a line past its 8 KiB read buffer came back as two
/// answers, and every later question read the one before it.
#[cfg(target_os = "macos")]
#[test]
fn a_line_past_eight_kib_and_the_next_one_each_get_their_own_answer() -> TestResult {
    let mut speller = Speller::apple().ok_or("no checker on a Mac")?;
    let long = format!("{}café blorpix", "word ".repeat(1_800));
    let cut = long.chars().count();
    let spans = answer(&mut speller, Ask::Check, &long).ok_or("no answer for the long line")?;
    assert_eq!(
        spans,
        Answer::Spans(std::iter::once(cut - 7..cut).collect())
    );
    let short =
        answer(&mut speller, Ask::Check, "a blorp").ok_or("no answer for the short line")?;
    assert_eq!(short, Answer::Spans(std::iter::once(2..7).collect()));
    let two = answer(&mut speller, Ask::Check, "I zorbix a qwzx then").ok_or("no answer")?;
    assert_eq!(
        two,
        Answer::Spans(vec![2..8, 11..15]),
        "every word, not the first"
    );
    let words = answer(&mut speller, Ask::Correct, "recieve").ok_or("no correction")?; // codespell:ignore recieve
    assert_eq!(words, Answer::Words(vec!["receive".to_owned()]));
    assert!(speller.alive());
    Ok(())
}

/// Dies with a stalled checker freezing the keyboard: a peer that never answers costs no wait,
/// and past the deadline spelling is absent rather than pending forever.
#[test]
fn a_peer_that_never_answers_costs_no_wait_and_dies_at_the_deadline() {
    let mut sleep = yi_tools::command("sleep");
    sleep.arg("30");
    let mut speller = Speller::spawn(sleep);
    let start = Instant::now();
    assert!(speller.ask(Ask::Check, "a blorp", start).is_none());
    assert!(
        start.elapsed() < Duration::from_millis(50),
        "{:?}",
        start.elapsed()
    );
    assert!(speller.waiting());
    speller.poll(start + DEADLINE + Duration::from_secs(1));
    assert!(!speller.alive());
    assert!(!speller.waiting());
    assert!(speller.ask(Ask::Check, "a blorp", start).is_none());
}

/// Dies with a missing checker read as one still starting: the peer that never spawned leaves
/// spelling absent, not waiting.
#[test]
fn a_peer_that_cannot_start_leaves_spelling_absent() {
    let mut speller = Speller::spawn(yi_tools::command("/nonexistent/yi-spell"));
    let _ = speller.ask(Ask::Check, "blorp", Instant::now());
    let until = Instant::now() + DEADLINE;
    while speller.alive() && Instant::now() < until {
        speller.poll(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!speller.alive());
}

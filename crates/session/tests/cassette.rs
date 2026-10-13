use std::error::Error;
use std::time::Duration;
use yi_session::cassette;

/// A scrubbed session the real binary wrote against a real provider; its entry timestamps are
/// the recording the lags below were read from, by hand, with `python3`, not with `cassette`.
const SESSION: &str = include_str!("../../tui/tests/fixtures/sessions/01a0648f.jsonl");

fn millis(lags: &[(yi_types::message::AgentMessage, Duration)]) -> Vec<u128> {
    lags.iter().map(|(_, lag)| lag.as_millis()).collect()
}

/// A paced replay that took the entry's own time since the message it answered would stream a
/// 984-character answer over 3 s or 30 s, and every motion scene would look wrong.
#[test]
fn a_recorded_session_replays_each_reply_over_the_time_it_took() -> Result<(), Box<dyn Error>> {
    let replies = cassette(SESSION)?;
    assert_eq!(
        millis(&replies),
        [3206, 7764, 10966, 4293, 3652, 4508, 4218, 3831, 8115]
    );
    assert!(
        replies.iter().all(|(message, _)| matches!(
            message,
            yi_types::message::AgentMessage::Assistant { .. }
        ))
    );
    Ok(())
}

/// Every cassette written before timing existed is a bare message per line, and it must replay
/// instantly.
#[test]
fn a_bare_message_cassette_replays_instantly() -> Result<(), Box<dyn Error>> {
    let bare = include_str!("../../../scripts/proof/tui_see/reply.jsonl");
    let replies = cassette(bare)?;
    assert_eq!(millis(&replies), [0]);
    Ok(())
}

#[test]
fn a_line_that_is_neither_names_its_number() {
    let error = cassette("{\"nope\":1}\n").err();
    assert!(error.is_some_and(|reason| reason.starts_with("line 1:")));
}

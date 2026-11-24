//! Resume after a reconnect: replay frames, sequence checks, and the asks the server will not send again.

use serde_json::Value;

use crate::ids::Millis;
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::reduce::state::Link;
use crate::wire::event::{Down, WireEvent};

/// How this app's copy of one thread's event stream stands against the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sync {
    /// Its branch was never replayed on this connection: its events pass by until a view
    /// opens it.
    #[default]
    Apart,
    /// A resume is in flight: events wait for its replay.
    Resuming,
    /// A resume was refused; the next poll asks again.
    Broken,
    /// In step: the number of the last event folded, none right after a replay.
    Live(Option<u64>),
}

/// What became of an event's number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Taken {
    /// It is the next one: fold the event.
    Next,
    /// Events before it never arrived: the branch must be replayed.
    Hole,
    /// The stream is not in step: the event passes by.
    Apart,
}

impl Sync {
    /// Whether the transcript follows the server's stream, event by event.
    pub fn live(self) -> bool {
        matches!(self, Self::Live(_))
    }

    /// Counts event `seq` in when it follows the last one folded.
    pub(crate) fn take(&mut self, seq: u64) -> Taken {
        match *self {
            Self::Live(last) if last.is_none_or(|last| last.checked_add(1) == Some(seq)) => {
                *self = Self::Live(Some(seq));
                Taken::Next
            }
            Self::Live(_) => Taken::Hole,
            Self::Apart | Self::Resuming | Self::Broken => Taken::Apart,
        }
    }
}

/// A recording that could not be played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("line {line} of the recording is not a recorded frame")]
pub struct PlayError {
    pub line: usize,
}

/// Steps a recording into the model with no socket: each line is `{"dir": "send" | "recv",
/// "frame": ..}`, as `fixtures/wire` holds them. It is how a scene or a test builds a model.
pub fn play(model: &mut Model, recording: &str) -> Result<Vec<Effect>, PlayError> {
    let mut effects = Vec::new();
    let lines = recording.lines().enumerate();
    for (index, line) in lines.filter(|(_, line)| !line.trim().is_empty()) {
        let line_effects = play_line(model, line).map_err(|_| PlayError {
            line: index.saturating_add(1),
        })?;
        effects.extend(line_effects);
    }
    Ok(effects)
}

/// Steps one line of a recording. A line the recorded client sent stands in for this app's
/// own request, so the server's answer to it is read the same way.
pub fn play_line(model: &mut Model, line: &str) -> Result<Vec<Effect>, PlayError> {
    let bad = PlayError { line: 1 };
    let mut row: Value = serde_json::from_str(line).map_err(|_| bad)?;
    let frame = row.get_mut("frame").map(Value::take).ok_or(bad)?;
    let now = model.wire.clock(model.now);
    let event = match row.get("dir").and_then(Value::as_str) {
        Some("recv") => match frame
            .pointer("/result/info/version")
            .and_then(Value::as_str)
        {
            // The answer to `initialize`: the client reads it and reports the link.
            Some(version) if frame.pointer("/result/protocolVersion").is_some() => WireEvent::Up {
                at: now,
                server: version.to_owned(),
                // A recording times nothing; a row may say how long its answer `took`.
                latency: row.get("took").and_then(Value::as_u64).map(Millis),
            },
            _ => WireEvent::Line(frame.to_string()),
        },
        Some("send") if frame.get("method").and_then(Value::as_str) == Some("initialize") => {
            if !matches!(model.wire.link, Link::Up { .. }) {
                return Ok(Vec::new());
            }
            WireEvent::Down {
                at: now,
                why: Down::Closed("the recorded client hung up".to_owned()),
            }
        }
        Some("send") => {
            model.wire.sent(&frame, now);
            return Ok(Vec::new());
        }
        _ => return Err(bad),
    };
    Ok(step(model, Msg::Wire(event)))
}

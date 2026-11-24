//! Plays a `fixtures/wire` recording's server side to a client under test and checks its sends.
//! A request is matched by method and its id mapped to the client's; a reply by id and result.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Map, Value};

/// How long the player waits for each frame the recording says the client sends.
pub const WAIT: Duration = Duration::from_secs(5);
/// How long it listens after a connection's last line for a frame the recording lacks.
pub const TAIL: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum Mismatch {
    #[error("recording line {line}: {reason}")]
    Recording { line: usize, reason: String },
    #[error("recording line {line}: want {want}, the client sent {got}")]
    Sent {
        line: usize,
        want: String,
        got: String,
    },
    #[error("recording line {line}: want {want}, the client sent nothing within {WAIT:?}")]
    Silent { line: usize, want: String },
    #[error("after recording line {line}: the client sent {got}, which the recording lacks")]
    Extra { line: usize, got: String },
    #[error(
        "after recording line {line}: the client sent {bytes} bytes that are not a JSON object"
    )]
    Garbled { line: usize, bytes: usize },
    #[error("connection {0}: the client did not connect")]
    Absent(usize),
    #[error("socket: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Send,
    Recv,
}

#[derive(Debug, Clone)]
pub struct Line {
    /// One-based line number in the recording.
    pub number: usize,
    pub dir: Dir,
    pub frame: Map<String, Value>,
}

/// A recording split into connections: each after the first opens with `initialize`.
#[derive(Debug, Clone)]
pub struct Replay {
    pub connections: Vec<Vec<Line>>,
}

/// What a connection carried, when the client matched it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Played {
    pub sent: usize,
    pub received: usize,
}

impl Replay {
    pub fn parse(text: &str) -> Result<Self, Mismatch> {
        let mut connections: Vec<Vec<Line>> = Vec::new();
        for (index, raw) in text
            .lines()
            .enumerate()
            .filter(|(_, raw)| !raw.trim().is_empty())
        {
            let number = index.saturating_add(1);
            let bad = |reason: &str| Mismatch::Recording {
                line: number,
                reason: reason.into(),
            };
            let value: Value =
                serde_json::from_str(raw).map_err(|error| bad(&error.to_string()))?;
            let dir = match value.get("dir").and_then(Value::as_str) {
                Some("send") => Dir::Send,
                Some("recv") => Dir::Recv,
                _ => return Err(bad("dir is not send or recv")),
            };
            let frame = value
                .get("frame")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| bad("frame is not an object"))?;
            let opens = dir == Dir::Send && method(&frame) == Some("initialize");
            match connections.last_mut() {
                Some(lines) if !opens => lines.push(Line { number, dir, frame }),
                _ => connections.push(vec![Line { number, dir, frame }]),
            }
        }
        Ok(Self { connections })
    }

    /// A recording under `fixtures/wire`, by file stem.
    pub fn fixture(name: &str) -> Result<Self, Mismatch> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/wire");
        Self::load(&root.join(format!("{name}.jsonl")))
    }

    pub fn load(path: &Path) -> Result<Self, Mismatch> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    /// Accepts one client connection per recorded connection, in order, and plays each.
    pub fn serve(self, listener: UnixListener) -> JoinHandle<Result<Vec<Played>, Mismatch>> {
        std::thread::spawn(move || {
            let mut played = Vec::new();
            for (index, lines) in self.connections.iter().enumerate() {
                let (stream, _) = listener.accept().map_err(|_| Mismatch::Absent(index))?;
                played.push(play(lines, stream)?);
            }
            Ok(played)
        })
    }
}

/// Plays one connection's lines on `stream`, then closes it.
pub fn play(lines: &[Line], stream: UnixStream) -> Result<Played, Mismatch> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut ids: Vec<(Value, Value)> = Vec::new();
    let mut played = Played::default();
    for line in lines {
        match line.dir {
            Dir::Recv => {
                let mut frame = line.frame.clone();
                let mapped = frame
                    .get("id")
                    .and_then(|id| ids.iter().find(|(from, _)| from == id));
                if method(&frame).is_none()
                    && let Some((_, to)) = mapped
                {
                    frame.insert("id".into(), to.clone());
                }
                writeln!(writer, "{}", Value::Object(frame))?;
                played.received = played.received.saturating_add(1);
            }
            Dir::Send => {
                reader.get_ref().set_read_timeout(Some(WAIT))?;
                let want = describe(&line.frame);
                let got =
                    next_frame(&mut reader, line.number)?.ok_or_else(|| Mismatch::Silent {
                        line: line.number,
                        want: want.clone(),
                    })?;
                check(line, &got, &mut ids)?;
                played.sent = played.sent.saturating_add(1);
            }
        }
    }
    reader.get_ref().set_read_timeout(Some(TAIL))?;
    let line = lines.last().map_or(0, |last| last.number);
    if let Some(extra) = next_frame(&mut reader, line)? {
        return Err(Mismatch::Extra {
            line,
            got: describe(&extra),
        });
    }
    Ok(played)
}

fn check(
    line: &Line,
    got: &Map<String, Value>,
    ids: &mut Vec<(Value, Value)>,
) -> Result<(), Mismatch> {
    let want = &line.frame;
    let same = match method(want) {
        Some(name) => method(got) == Some(name),
        None => ["id", "result", "error"]
            .iter()
            .all(|field| want.get(*field) == got.get(*field)),
    };
    if !same {
        return Err(Mismatch::Sent {
            line: line.number,
            want: describe(want),
            got: describe(got),
        });
    }
    if let (Some(_), Some(from), Some(to)) = (method(want), want.get("id"), got.get("id")) {
        ids.push((from.clone(), to.clone()));
    }
    Ok(())
}

/// The next whole frame, `None` at the end of the stream or when the wait runs out.
fn next_frame(
    reader: &mut BufReader<UnixStream>,
    line: usize,
) -> Result<Option<Map<String, Value>>, Mismatch> {
    let mut raw = String::new();
    match reader.read_line(&mut raw) {
        Ok(0) => Ok(None),
        Ok(_) => match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(frame)) => Ok(Some(frame)),
            _ => Err(Mismatch::Garbled {
                line,
                bytes: raw.len(),
            }),
        },
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn method(frame: &Map<String, Value>) -> Option<&str> {
    frame.get("method").and_then(Value::as_str)
}

/// A short name for a frame: its method, or the id and outcome of a reply.
fn describe(frame: &Map<String, Value>) -> String {
    match method(frame) {
        Some(name) => format!("request {name}"),
        None => {
            let id = frame.get("id").map(Value::to_string).unwrap_or_default();
            let body = frame
                .get("result")
                .or_else(|| frame.get("error"))
                .map(Value::to_string);
            let body: String = body.unwrap_or_default().chars().take(120).collect();
            format!("reply {id} {body}")
        }
    }
}

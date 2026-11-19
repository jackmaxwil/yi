//! JSON-RPC frames as the server writes them; a frame `yi-types` has no type for stays a raw map.

#[cfg(test)]
mod tests;

use std::io::{self, BufRead, Read};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use yi_types::acp::{AcpConfigOption, AcpPermissionParams, AcpStopReason};
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::goal::Goal;
use yi_types::subagent::ChildUpdate;
use yi_types::todo::{Claim, TodoList};
use yi_types::wire::SessionStats;

use crate::ids::{AgentId, ThreadId};
use crate::reduce::threads::Run;
use crate::reduce::todos::Plan;
use crate::wire::request::RequestId;

/// The most bytes one frame may hold: what Yi's own console reads. A prompt is one entry
/// in one frame, and two images of the largest size Yi takes are more than half of this.
pub const MAX_FRAME: u64 = 32 << 20;
/// The most characters of a server's error message this app keeps.
const MAX_MESSAGE: usize = 512;

/// A value from the server: read as its type when this build knows the shape, else kept as
/// written, so a newer server cannot break a fold or lose what it said.
#[derive(Debug, Clone, PartialEq)]
pub enum Kept<T> {
    Known(T),
    Raw(Value),
}

impl<T: DeserializeOwned> Kept<T> {
    pub fn read(value: Value) -> Self {
        match T::deserialize(&value) {
            Ok(known) => Self::Known(known),
            Err(_) => Self::Raw(value),
        }
    }
}

impl<T> Kept<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Self::Known(known) => Some(known),
            Self::Raw(_) => None,
        }
    }
}

/// Why a request got no result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the server refused: {message} ({code})")]
    Server { code: i64, message: String },
    #[error("not sent: the server is not connected")]
    Unsent,
    #[error("the connection closed before the answer came")]
    Lost,
    #[error("the server did not answer in time")]
    TimedOut,
}

/// One frame from the server, decoded as far as this build reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A `session/update` notification about one thread.
    Update { thread: ThreadId, update: Update },
    /// The server asks the user to allow something; `id` is echoed in the answer.
    Ask {
        id: Value,
        params: Box<Kept<AcpPermissionParams>>,
    },
    /// The answer to a request this app sent.
    Reply {
        id: RequestId,
        result: Result<Value, Refusal>,
    },
    /// The server calls a method this app does not have.
    Request { id: Value, method: String },
    /// A frame that is none of these, as written.
    Other(Value),
}

/// What a `session/update` says.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// One agent event, numbered per thread; `child` names the child agent it came from.
    Event {
        seq: u64,
        child: Option<AgentId>,
        event: Box<Kept<AgentEvent>>,
    },
    /// The server lost events on the way: the stream has a hole.
    Gap,
    Replay(Box<Replay>),
    /// Session entries just stored, oldest first: the message that ended and what led to it.
    Committed {
        entries: Vec<Kept<Entry>>,
        notice: Option<String>,
    },
    State(Run),
    /// Tokens in the context and the model's window.
    Context {
        used: u64,
        size: u64,
    },
    /// The model, effort and mode options, and the model's window.
    Config {
        options: Vec<AcpConfigOption>,
        window: Option<u64>,
    },
    Name(String),
    Workdir {
        cwd: String,
        lane: Option<String>,
    },
    Todo(Option<Box<TodoList>>),
    Plan(Option<Plan>),
    Goal(Option<Box<Goal>>),
    Claims(Vec<Claim>),
    Child(Box<ChildUpdate>),
    Notice(String),
    /// The thread's worker was swapped: its stream starts over.
    Replaced,
    /// The standard projection of what the agent events already carry; never folded.
    Echo,
    /// A kind this build has no fold for, or could not read: its fields as written.
    Other {
        kind: String,
        fields: Value,
    },
}

/// One `_yi/replay` frame: a run of the thread's stored entries.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Replay {
    /// How many stored entries come before this frame's; zero starts the branch over.
    pub from: u64,
    pub entries: Vec<Kept<Entry>>,
    /// The child agent whose branch this is; none is the thread's own.
    pub child: Option<AgentId>,
    pub name: Option<String>,
    pub todos: Option<Box<TodoList>>,
    pub goal: Option<Box<Goal>>,
    pub window: Option<u64>,
    /// The thread's totals, on its last frame: set, never added to.
    pub stats: Option<SessionStats>,
    /// Parts of the frame this build could not read, by field name.
    pub unread: Vec<(&'static str, Value)>,
}

/// Bytes from the socket that are not a frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{len} bytes that are not a JSON object: {why}")]
pub struct BadFrame {
    pub len: usize,
    pub why: String,
}

impl Frame {
    /// Reads one line off the socket. The line is another program's bytes: anything in it
    /// that is not understood is kept raw, and nothing in it can make this panic.
    pub fn decode(line: &[u8]) -> Result<Self, BadFrame> {
        let bad = |why: String| BadFrame {
            len: line.len(),
            why,
        };
        let value: Value = serde_json::from_slice(line).map_err(|error| bad(error.to_string()))?;
        let Value::Object(mut frame) = value else {
            return Err(bad("it is not an object".to_owned()));
        };
        let method = frame.get("method").and_then(Value::as_str);
        Ok(match (method, frame.get("id").is_some()) {
            (Some("session/update"), false) => match frame.remove("params") {
                Some(Value::Object(params)) => update(params),
                _ => Self::Other(Value::Object(frame)),
            },
            (Some(crate::wire::request::PERMISSION), true) => Self::Ask {
                id: frame.remove("id").unwrap_or_default(),
                params: Box::new(Kept::read(frame.remove("params").unwrap_or_default())),
            },
            (Some(method), true) => Self::Request {
                method: method.to_owned(),
                id: frame.remove("id").unwrap_or_default(),
            },
            (None, true) => Self::Reply {
                id: RequestId::of(frame.get("id").unwrap_or(&Value::Null)),
                result: match frame.remove("error") {
                    Some(error) => Err(refusal(&error)),
                    None => Ok(frame.remove("result").unwrap_or_default()),
                },
            },
            (Some(_) | None, false) => Self::Other(Value::Object(frame)),
        })
    }
}

fn refusal(error: &Value) -> Refusal {
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    Refusal::Server {
        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: message.chars().take(MAX_MESSAGE).collect(),
    }
}

fn read<T: DeserializeOwned>(value: Option<&Value>) -> Option<T> {
    T::deserialize(value?).ok()
}

fn text(fields: &Map<String, Value>, key: &str) -> Option<String> {
    fields.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn update(mut params: Map<String, Value>) -> Frame {
    let thread = params.remove("sessionId");
    let fields = params.remove("update");
    let (Some(Value::String(thread)), Some(Value::Object(mut fields))) = (thread, fields) else {
        return Frame::Other(Value::Object(params));
    };
    let Some(Value::String(kind)) = fields.remove("sessionUpdate") else {
        return Frame::Other(Value::Object(fields));
    };
    let update = known(&kind, &mut fields).unwrap_or_else(|| Update::Other {
        kind,
        fields: Value::Object(fields),
    });
    Frame::Update {
        thread: ThreadId::new(thread),
        update,
    }
}

/// The update as a kind this build folds; none leaves `fields` whole, to be kept raw.
fn known(kind: &str, fields: &mut Map<String, Value>) -> Option<Update> {
    let child = || text(fields, "childId").map(AgentId::new);
    Some(match kind {
        "_yi/event" => Update::Event {
            seq: fields.get("seq")?.as_u64()?,
            child: child(),
            event: Box::new(Kept::read(fields.remove("event")?)),
        },
        "_yi/event_gap" => Update::Gap,
        "_yi/worker_replaced" => Update::Replaced,
        "_yi/replay" => Update::Replay(Box::new(replay(fields)?)),
        "_yi/committed" => {
            let mut entries = match fields.remove("ancestors") {
                Some(Value::Array(ancestors)) => ancestors,
                _ => Vec::new(),
            };
            entries.push(fields.remove("entry")?);
            Update::Committed {
                entries: entries.into_iter().map(Kept::read).collect(),
                notice: text(fields, "ancestorsNotice"),
            }
        }
        "state_update" => Update::State(match fields.get("state")?.as_str()? {
            "running" => Run::Running,
            "requires_action" => Run::NeedsYou,
            "idle" => Run::Idle {
                stop: read::<AcpStopReason>(fields.get("stopReason")),
            },
            other => Run::Other(other.to_owned()),
        }),
        "usage_update" => Update::Context {
            used: fields.get("used")?.as_u64()?,
            size: fields.get("size")?.as_u64()?,
        },
        "_yi/config" => Update::Config {
            options: read(fields.get("configOptions"))?,
            window: fields.get("contextWindow").and_then(Value::as_u64),
        },
        "_yi/name" => Update::Name(text(fields, "name")?),
        "_yi/workdir" => Update::Workdir {
            cwd: text(fields, "cwd")?,
            lane: text(fields, "lane"),
        },
        "_yi/todo" => Update::Todo(optional(fields.get("list"))?),
        "_yi/goal" => Update::Goal(optional(fields.get("goal"))?),
        "_yi/plan_progress" => Update::Plan(match fields.get("plan") {
            None | Some(Value::Null) => None,
            Some(plan) => Some(Plan {
                done: plan.get("done")?.as_u64()?,
                total: plan.get("total")?.as_u64()?,
                running: plan
                    .get("running")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
        }),
        "_yi/claims" => Update::Claims(read(fields.get("claims"))?),
        "_yi/subagent_update" => Update::Child(Box::new(ChildUpdate::deserialize(&*fields).ok()?)),
        "_yi/notice" => Update::Notice(text(fields, "text")?),
        "agent_message_chunk"
        | "agent_thought_chunk"
        | "agent_message"
        | "user_message"
        | "tool_call_update"
        | "tool_call_content_chunk"
        | "terminal_update"
        | "terminal_output_chunk" => Update::Echo,
        _ => return None,
    })
}

/// A value the server may send as null: absent and null are none, unreadable fails.
fn optional<T: DeserializeOwned>(value: Option<&Value>) -> Option<Option<Box<T>>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(value) => T::deserialize(value)
            .ok()
            .map(|known| Some(Box::new(known))),
    }
}

fn replay(fields: &mut Map<String, Value>) -> Option<Replay> {
    let Some(Value::Array(entries)) = fields.remove("entries") else {
        return None;
    };
    let mut replay = Replay {
        from: fields.get("from").and_then(Value::as_u64).unwrap_or(0),
        entries: entries.into_iter().map(Kept::read).collect(),
        child: text(fields, "childId").map(AgentId::new),
        name: text(fields, "name"),
        window: fields.get("contextWindow").and_then(Value::as_u64),
        ..Replay::default()
    };
    // Each part is read on its own: one a newer server changed must not cost the rest.
    match optional(fields.get("todos")) {
        Some(todos) => replay.todos = todos,
        None => replay
            .unread
            .extend(fields.remove("todos").map(|raw| ("todos", raw))),
    }
    match optional(fields.get("goal")) {
        Some(goal) => replay.goal = goal,
        None => replay
            .unread
            .extend(fields.remove("goal").map(|raw| ("goal", raw))),
    }
    match optional::<SessionStats>(fields.get("stats")) {
        Some(stats) => replay.stats = stats.map(|stats| *stats),
        None => replay
            .unread
            .extend(fields.remove("stats").map(|raw| ("stats", raw))),
    }
    Some(replay)
}

/// Why a frame could not be read off the socket.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the socket failed")]
    Io(#[from] io::Error),
    /// `resynced` false means the line never ended: the stream is mid-frame and must close.
    #[error("a frame over {max} bytes (resynced: {resynced})")]
    TooLong { max: u64, resynced: bool },
    #[error("the socket closed in the middle of a frame, {len} bytes in")]
    Truncated { len: usize },
}

/// One newline-ended frame of at most `max` bytes; none is a clean end of the stream. A
/// longer line is skipped to its newline, within a bound, so its tail is never read as a frame.
pub fn read_frame(reader: &mut impl BufRead, max: u64) -> Result<Option<Vec<u8>>, ReadError> {
    let mut frame = Vec::new();
    reader.by_ref().take(max).read_until(b'\n', &mut frame)?;
    match frame.last() {
        None => Ok(None),
        Some(b'\n') => {
            frame.pop();
            Ok(Some(frame))
        }
        Some(_) if u64::try_from(frame.len()).is_ok_and(|len| len < max) => {
            Err(ReadError::Truncated { len: frame.len() })
        }
        Some(_) => Err(ReadError::TooLong {
            max,
            resynced: skip_line(reader, max.saturating_mul(2))?,
        }),
    }
}

/// Reads and drops up to `budget` bytes, through the next newline; whether one was found.
fn skip_line(reader: &mut impl BufRead, mut budget: u64) -> io::Result<bool> {
    while budget > 0 {
        let buffered = reader.fill_buf()?;
        let within = usize::try_from(budget).unwrap_or(usize::MAX);
        let window = buffered.get(..within).unwrap_or(buffered);
        if window.is_empty() {
            return Ok(false);
        }
        let newline = window.iter().position(|byte| *byte == b'\n');
        let used = newline.map_or(window.len(), |at| at.saturating_add(1));
        reader.consume(used);
        if newline.is_some() {
            return Ok(true);
        }
        budget = budget.saturating_sub(u64::try_from(used).unwrap_or(u64::MAX));
    }
    Ok(false)
}

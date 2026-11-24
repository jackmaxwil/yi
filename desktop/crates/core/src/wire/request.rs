//! What the UI asks of the server, for the writer thread to send.

use serde_json::{Value, json};

use crate::ids::{AgentId, EntryId, ThreadId};

/// The id a request carries and its answer echoes. This app's own are numbers, in the order
/// sent; a recording may hold any JSON, kept as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    Number(u64),
    Text(String),
}

impl RequestId {
    /// The id of the `n`th request this app sends.
    pub fn number(n: u64) -> Self {
        Self::Number(n)
    }

    /// The id a frame carries, whatever JSON it is.
    pub fn of(id: &Value) -> Self {
        id.as_u64()
            .map_or_else(|| Self::Text(id.to_string()), Self::Number)
    }
}

/// One frame for the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The id its answer will carry; none when no answer comes.
    pub id: Option<RequestId>,
    /// One JSON-RPC object, serialised, without its newline.
    pub line: String,
}

impl Request {
    /// The `n`th call this app makes; the server answers it by that number.
    pub fn call(n: u64, method: &str, params: Value) -> Self {
        Self {
            id: Some(RequestId::number(n)),
            line: json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params})
                .to_string(),
        }
    }

    /// The answer to a request the server sent, under the id the server gave it.
    pub fn answer(id: &Value, result: Value) -> Self {
        Self {
            id: None,
            line: json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        }
    }

    /// Tells the server this app has no such method, so it does not wait for an answer.
    pub fn refuse(id: &Value, method: &str) -> Self {
        let message = format!("yi-desktop has no method {method}");
        let error = json!({"code": -32601, "message": message});
        Self {
            id: None,
            line: json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string(),
        }
    }
}

pub const LIST: &str = "session/list";
pub const NEW: &str = "session/new";
pub const RESUME: &str = "session/resume";
pub const PROMPT: &str = "session/prompt";
pub const CANCEL: &str = "session/cancel";
/// The server's ask; this app sends it only to itself, reading back what a restart kept.
pub const PERMISSION: &str = "session/request_permission";
/// The server's ask to put a thing in front of the person: the model's `show` tool.
pub const SHOW: &str = yi_types::acp::SHOW_METHOD;
pub const SET_OPTION: &str = "session/set_config_option";
pub const STEER: &str = "_yi/steer";
pub const SLASH: &str = "_yi/slash";
pub const PIN: &str = "_yi/pin";
pub const SEEN: &str = "_yi/seen";
pub const FORK: &str = "_yi/fork";
pub const REWIND: &str = "_yi/rewind";
pub const SETTINGS: &str = "_yi/settings";
pub const SET_DEFAULT: &str = "_yi/set_default";
pub const CHILD_REPLAY: &str = "_yi/child_replay";
pub const CHILD_ANSWER: &str = "_yi/child_answer";
pub const CHILD_ABORT: &str = "_yi/child_abort";

pub fn new_thread(cwd: &str) -> Value {
    json!({"cwd": cwd, "mcpServers": [], "_meta": {"yi": {"shows": true}}})
}

/// The whole stored branch again, as `_yi/replay` frames only: the standard replay would
/// send every message a second time.
pub fn resume(thread: &ThreadId, cwd: &str) -> Value {
    let mut params = json!({"sessionId": thread});
    if !cwd.is_empty() {
        params["cwd"] = Value::from(cwd);
    }
    params["replayFrom"] = Value::from(0);
    params["_meta"] = json!({"yi": {"replayUpdates": false, "shows": true}});
    params
}

pub fn prompt(thread: &ThreadId, blocks: &[Value]) -> Value {
    json!({"sessionId": thread, "prompt": blocks})
}

pub fn of_thread(thread: &ThreadId) -> Value {
    json!({"sessionId": thread})
}

/// One child agent of a thread: what `_yi/child_replay` and `_yi/child_abort` take.
pub fn of_child(thread: &ThreadId, child: &AgentId) -> Value {
    json!({"sessionId": thread, "childId": child.as_str()})
}

/// The user's words for the question `question` a child asked.
pub fn answer_child(thread: &ThreadId, child: &AgentId, question: &str, text: &str) -> Value {
    let mut params = of_child(thread, child);
    params["questionId"] = Value::from(question);
    params["text"] = Value::from(text);
    params
}

pub fn steer(thread: &ThreadId, text: &str) -> Value {
    json!({"sessionId": thread, "text": text})
}

pub fn slash(thread: &ThreadId, line: &str) -> Value {
    json!({"sessionId": thread, "line": line})
}

pub fn set_option(thread: &ThreadId, option: &str, value: &str) -> Value {
    json!({"sessionId": thread, "configId": option, "type": "id", "value": value})
}

pub fn pin(thread: &ThreadId, pinned: bool) -> Value {
    json!({"sessionId": thread, "pinned": pinned})
}

/// A new thread branched from `entry`: keeping it when `at`, else leaving it out.
pub fn fork(thread: &ThreadId, entry: &EntryId, at: bool) -> Value {
    let mut params = json!({"sessionId": thread, "entryId": entry});
    if at {
        params["position"] = Value::from("at");
    }
    params
}

pub fn rewind(thread: &ThreadId, entry: &EntryId, files: bool) -> Value {
    let mut params = json!({"sessionId": thread, "entryId": entry});
    if files {
        params["files"] = Value::from(true);
    }
    params
}

pub fn set_default(setting: &str, value: Value) -> Value {
    json!({"id": setting, "value": value})
}

/// The body of an answer to a permission ask: one option picked, or none.
pub fn outcome(option: Option<&str>) -> Value {
    match option {
        Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

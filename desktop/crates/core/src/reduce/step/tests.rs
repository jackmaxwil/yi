//! The reducer against recordings of a real `yi serve`, with no socket in between.

#[cfg(test)]
mod agents;
#[cfg(test)]
mod approvals;
#[cfg(test)]
mod commands;
#[cfg(test)]
mod lists;
#[cfg(test)]
mod parts;
#[cfg(test)]
mod resume;
#[cfg(test)]
mod turn;
#[cfg(test)]
mod unknown;

use std::error::Error;
use std::path::Path;

use serde_json::{Value, json};
use yi_types::message::{AgentMessage, Content, UserContent};

use crate::address::uri::Address;
use crate::command::table::{Route, Run, Target};
use crate::ids::{CommandId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::reduce::threads::Thread;
use crate::reduce::turns::{Body, Transcript};
use crate::wire::event::WireEvent;
use crate::wire::replay::play;

pub(crate) type Check = Result<(), Box<dyn Error>>;

/// One recording from `fixtures/wire`, made by `spikes/wire/run.sh` against a real server.
pub(crate) fn recording(name: &str) -> Result<String, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/wire")
        .join(name);
    Ok(std::fs::read_to_string(path)?)
}

/// The recording's rows: whether the server sent it, and the frame.
pub(crate) fn rows(name: &str) -> Result<Vec<(bool, Value)>, Box<dyn Error>> {
    recording(name)?
        .lines()
        .map(|line| {
            let row: Value = serde_json::from_str(line)?;
            Ok((row["dir"] == "recv", row["frame"].clone()))
        })
        .collect()
}

/// The recording's first `rows` lines as a recording of their own.
pub(crate) fn head(name: &str, rows: usize) -> Result<String, Box<dyn Error>> {
    Ok(recording(name)?
        .lines()
        .take(rows)
        .collect::<Vec<_>>()
        .join("\n"))
}

pub(crate) fn played(recording: &str) -> Result<(Model, Vec<Effect>), Box<dyn Error>> {
    let mut model = Model::default();
    let effects = play(&mut model, recording)?;
    Ok((model, effects))
}

/// The entries the server of the recording `name` stored, oldest first: its commits'.
pub(crate) fn branch(name: &str) -> Result<Vec<Value>, Box<dyn Error>> {
    let updates = rows(name)?
        .into_iter()
        .map(|(_, frame)| frame["params"]["update"].clone());
    Ok(updates
        .filter(|update| update["sessionUpdate"] == "_yi/committed")
        .flat_map(|update| {
            let mut entries = update["ancestors"].as_array().cloned().unwrap_or_default();
            entries.push(update["entry"].clone());
            entries
        })
        .collect())
}

/// A `session/update` frame as the server writes one.
pub(crate) fn update(thread: &str, update: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": thread, "update": update}})
}

/// Steps one frame from the server into the model, stamped `at`.
pub(crate) fn feed(model: &mut Model, at: u64, frame: &Value) -> Vec<Effect> {
    step(model, Msg::Tick(Millis(at)));
    step(model, Msg::Wire(WireEvent::Line(frame.to_string())))
}

pub(crate) fn address(kind: Kind, id: &str) -> Result<Target, Box<dyn Error>> {
    Ok(Target::Thing(Address::new(kind, id)?))
}

/// Runs a wire command the way a view does.
pub(crate) fn run(
    model: &mut Model,
    command: CommandId,
    target: Target,
    text: Option<&str>,
) -> Vec<Effect> {
    let run = Run {
        text: text.map(str::to_owned),
        ..Run::new(command).on(target)
    };
    step(model, Msg::Command(Route::Wire, run))
}

/// The frames a step asked the client to send, as JSON.
pub(crate) fn sent(effects: &[Effect]) -> Result<Vec<Value>, Box<dyn Error>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Wire(request) => Some(serde_json::from_str(&request.line)),
            Effect::Source(..) | Effect::Window(_) | Effect::Notify(_) => None,
        })
        .map(|frame| frame.map_err(Into::into))
        .collect()
}

pub(crate) fn thread<'a>(model: &'a Model, id: &str) -> Result<&'a Thread, Box<dyn Error>> {
    model
        .wire
        .threads
        .get(&ThreadId::new(id))
        .ok_or_else(|| format!("the model has no thread {id}").into())
}

/// One line per message: who spoke, then the text or the tool calls.
pub(crate) fn said(transcript: &Transcript) -> Vec<String> {
    transcript
        .messages
        .iter()
        .map(|message| match &message.body {
            Body::Message(body) => match body.as_ref() {
                AgentMessage::User { content, .. } => match content {
                    UserContent::Text(text) => format!("user: {text}"),
                    UserContent::Blocks(blocks) => format!("user: {}", blocks.len()),
                },
                AgentMessage::Assistant { content, .. } => {
                    let parts: Vec<String> = content.iter().map(block).collect();
                    format!("assistant: {}", parts.join(" "))
                }
                other => format!("other: {}", other.plain_text()),
            },
            Body::Raw(raw) => format!("raw: {raw}"),
        })
        .collect()
}

fn block(content: &Content) -> String {
    match content {
        Content::Text { text, .. } => text.clone(),
        Content::Thinking { thinking, .. } => format!("({thinking})"),
        Content::ToolCall { id, name, .. } => format!("[{name} {id}]"),
        Content::Image { .. } | Content::Other(_) => "?".to_owned(),
    }
}

/// The entry ids of the stored messages, in order.
pub(crate) fn entries(transcript: &Transcript) -> Vec<&str> {
    transcript
        .messages
        .iter()
        .filter_map(|message| message.entry.as_ref())
        .map(|entry| entry.as_str())
        .collect()
}

//! A tool call the model wrote as text: GLM's call channel misfires into its prose as
//! `<tool_call>read<arg_key>path</arg_key><arg_value>/x</arg_value></tool_call>`.

use serde_json::{Map, Value};
use yi_types::message::{AgentMessage, Content, StopReason};

pub const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
const KEY_OPEN: &str = "<arg_key>";
const KEY_CLOSE: &str = "</arg_key>";
const VALUE_OPEN: &str = "<arg_value>";
const VALUE_CLOSE: &str = "</arg_value>";

/// Every well-formed block in the message's text becomes a `ToolCall` block after it; the
/// text keeps what stood outside them, and a `Stop` becomes `ToolUse`. False when nothing moved.
pub fn recover_in(output: &mut AgentMessage) -> bool {
    let AgentMessage::Assistant {
        content,
        stop_reason,
        ..
    } = output
    else {
        return false;
    };
    let mut calls = Vec::new();
    for block in content.iter_mut() {
        let Content::Text { text, .. } = block else {
            continue;
        };
        if let Some((kept, found)) = recover(text) {
            *text = kept;
            calls.extend(found);
        }
    }
    if calls.is_empty() {
        return false;
    }
    for (n, (name, arguments)) in calls.into_iter().enumerate() {
        content.push(Content::ToolCall {
            id: format!("leak-{n}"),
            name,
            arguments,
            thought_signature: None,
            namespace: None,
        });
    }
    if *stop_reason == StopReason::Stop {
        *stop_reason = StopReason::ToolUse;
    }
    true
}

/// A call as the text spelled it: the tool's name and its arguments.
pub type SpelledCall = (String, Map<String, Value>);

/// The text with its well-formed blocks cut out, and the calls they spelled; a block that
/// does not parse stays in the text. None when the text spelled no call.
pub fn recover(text: &str) -> Option<(String, Vec<SpelledCall>)> {
    let mut rest = text;
    let mut kept = String::new();
    let mut calls = Vec::new();
    while let Some(start) = rest.find(OPEN) {
        let Some(end) = rest.get(start..).and_then(|tail| tail.find(CLOSE)) else {
            break;
        };
        let after = start.saturating_add(end).saturating_add(CLOSE.len());
        let inner = rest.get(start.saturating_add(OPEN.len())..start.saturating_add(end));
        match inner.and_then(parse_block) {
            Some(call) => {
                kept.push_str(rest.get(..start).unwrap_or_default());
                calls.push(call);
            }
            None => kept.push_str(rest.get(..after).unwrap_or_default()),
        }
        rest = rest.get(after..).unwrap_or_default();
    }
    if calls.is_empty() {
        return None;
    }
    kept.push_str(rest);
    Some((kept.trim().to_owned(), calls))
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<(&'a str, &'a str)> {
    let start = text.find(open)?.saturating_add(open.len());
    let tail = text.get(start..)?;
    let end = tail.find(close)?;
    Some((
        tail.get(..end)?,
        tail.get(end.saturating_add(close.len())..)?,
    ))
}

fn parse_block(inner: &str) -> Option<SpelledCall> {
    let (name, mut args_text) = match inner.find(KEY_OPEN) {
        Some(at) => (inner.get(..at)?.trim(), inner.get(at..)?),
        None => (inner.trim(), ""),
    };
    let named = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !named {
        return None;
    }
    let mut arguments = Map::new();
    while args_text.contains(KEY_OPEN) {
        let (key, after_key) = between(args_text, KEY_OPEN, KEY_CLOSE)?;
        let (value, after_value) = between(after_key, VALUE_OPEN, VALUE_CLOSE)?;
        arguments.insert(key.trim().to_owned(), coerce(value));
        args_text = after_value;
    }
    Some((name.to_owned(), arguments))
}

/// A value that reads as JSON structure or a number is one; anything else is the string.
fn coerce(value: &str) -> Value {
    match serde_json::from_str::<Value>(value.trim()) {
        Ok(parsed) if !parsed.is_string() && !parsed.is_null() => parsed,
        _ => Value::String(value.to_owned()),
    }
}

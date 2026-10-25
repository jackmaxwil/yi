//! A tool call the model wrote as text: GLM's call channel misfires into its prose as
//! `<tool_call>read<arg_key>path</arg_key><arg_value>/x</arg_value></tool_call>`.

use std::borrow::Cow;

use serde_json::{Map, Value};
use yi_types::message::{AgentMessage, Content, StopReason};

pub const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
const KEY_OPEN: &str = "<arg_key>";
const KEY_CLOSE: &str = "</arg_key>";
const VALUE_OPEN: &str = "<arg_value>";
const VALUE_CLOSE: &str = "</arg_value>";

/// Every well-formed block in the text becomes a `ToolCall` after it, every call's arguments
/// pass [`unleak`], and a `Stop` becomes `ToolUse`. False when no block moved.
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
        match block {
            Content::Text { text, .. } => {
                if let Some((kept, found)) = recover(text) {
                    *text = kept;
                    calls.extend(found);
                }
            }
            Content::ToolCall { arguments, .. } => {
                if let Cow::Owned(fixed) = unleak(arguments) {
                    *arguments = fixed;
                }
            }
            _ => {}
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

/// Incident: 17 v4 `done` calls leaked the markup into a key, `{"done<arg_key>evidence": …}`, or a
/// value, `{"done": "evidence</arg_key><arg_value>…"}`; the field reads back, the stray word a flag.
pub fn unleak(args: &Map<String, Value>) -> Cow<'_, Map<String, Value>> {
    let word = |text: &str| {
        !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let mut fixed = Cow::Borrowed(args);
    for (key, value) in args {
        let leaked = match key.rsplit_once(KEY_OPEN) {
            Some((stray, field)) => Some((stray, field, value.clone())),
            None => (value.as_str())
                .and_then(|text| text.split_once("</arg_key><arg_value>"))
                .filter(|(field, _)| word(field))
                .map(|(field, rest)| (key.as_str(), field, Value::String(rest.to_owned()))),
        };
        if let Some((stray, field, value)) = leaked
            && !field.is_empty()
            && !fixed.contains_key(field)
        {
            let map = fixed.to_mut();
            map.remove(key);
            map.insert(field.to_owned(), value);
            if word(stray) {
                map.entry(stray).or_insert(Value::Bool(true));
            }
        }
    }
    fixed
}

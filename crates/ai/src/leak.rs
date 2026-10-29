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

static MESSAGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    // Incident: every message's first leaked call was `leak-0`, so a cut keyed by that id answered
    // the wrong turn's call. The message's own number leads the id and no two share one.
    let message = MESSAGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    for (n, (name, arguments)) in calls.into_iter().enumerate() {
        content.push(Content::ToolCall {
            id: format!("leak-{message}-{n}"),
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

/// The text with its well-formed blocks cut out, and the calls they spelled; a block that does not
/// parse, or one the model only quoted as code, stays in the text.
pub fn recover(text: &str) -> Option<(String, Vec<SpelledCall>)> {
    let quoted = quoted(text);
    let inside = |at: usize| quoted.iter().any(|(from, to)| at >= *from && at < *to);
    let mut kept = String::new();
    let mut calls = Vec::new();
    let mut base = 0;
    while let Some(found) = text.get(base..).and_then(|tail| tail.find(OPEN)) {
        let start = base.saturating_add(found);
        let Some(end) = text.get(start..).and_then(|tail| tail.find(CLOSE)) else {
            break;
        };
        let after = start.saturating_add(end).saturating_add(CLOSE.len());
        let inner = text.get(start.saturating_add(OPEN.len())..start.saturating_add(end));
        match inner.filter(|_| !inside(start)).and_then(parse_block) {
            Some(call) => {
                kept.push_str(text.get(base..start).unwrap_or_default());
                calls.push(call);
            }
            None => kept.push_str(text.get(base..after).unwrap_or_default()),
        }
        base = after;
    }
    if calls.is_empty() {
        return None;
    }
    kept.push_str(text.get(base..).unwrap_or_default());
    Some((kept.trim().to_owned(), calls))
}

/// The byte ranges a reader sees as code: a fenced block, or an inline span closed by a run of the
/// same backticks. Incident: a message quoting this module's own example ran the call it quoted.
fn quoted(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence: Option<(usize, char, usize)> = None;
    let mut inline: Option<(usize, usize)> = None;
    let mut at: usize = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at = at.saturating_add(line.len());
        let body = line.trim_start_matches([' ', '\t']);
        let indent = line.len() - body.len();
        let Some(head) = body.chars().next() else {
            inline = None;
            continue;
        };
        let run = body
            .chars()
            .take_while(|character| *character == head)
            .count();
        let fenced = run >= 3 && (head == '`' || head == '~');
        if let Some((width, character, from)) = fence {
            if fenced
                && character == head
                && run >= width
                && body.trim_end().chars().all(|c| c == character)
            {
                ranges.push((from, at));
                fence = None;
            }
            continue;
        }
        if fenced {
            fence = Some((run, head, start.saturating_add(indent)));
            continue;
        }
        let mut search = 0;
        while let Some(found) = body.get(search..).and_then(|tail| tail.find('`')) {
            let column = search.saturating_add(found);
            let width = body
                .get(column..)
                .map_or(0, |tail| tail.chars().take_while(|c| *c == '`').count());
            let from = start.saturating_add(indent).saturating_add(column);
            match inline {
                Some((opened_at, opened)) if opened == width => {
                    ranges.push((opened_at, from.saturating_add(width)));
                    inline = None;
                }
                Some(_) => {}
                None => inline = Some((from, width)),
            }
            search = column.saturating_add(width.max(1));
        }
        inline = None;
    }
    if let Some((_, _, from)) = fence {
        ranges.push((from, at));
    }
    ranges
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

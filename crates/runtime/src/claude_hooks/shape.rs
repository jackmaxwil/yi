use std::path::Path;

use serde_json::{Map, Value, json};
use yi_types::event::ToolResult;

use super::{Call, Event, RESPONSE_CAP};

fn applies(args: &Map<String, Value>) -> bool {
    args.get("replace").is_some() && args.get("apply") == Some(&Value::Bool(true))
}

pub(super) fn claude_name(tool: &str, args: &Map<String, Value>) -> String {
    match tool {
        "bash" => "Bash".to_owned(),
        "read" => "Read".to_owned(),
        "write" => "Write".to_owned(),
        "edit" => "Edit".to_owned(),
        "grep" if applies(args) => "Edit".to_owned(),
        "grep" => "Grep".to_owned(),
        other if other.starts_with("mcp__") => other.to_owned(),
        other => format!("mcp__yi__{other}"),
    }
}

fn absolute(raw: &str, cwd: &Path) -> Value {
    match raw.contains("://") {
        true => Value::from(raw),
        false => {
            let path = yi_permission::lexical_normalize(&yi_permission::resolve_target(raw, cwd));
            Value::from(path.to_string_lossy().into_owned())
        }
    }
}

fn renamed(args: &Map<String, Value>, pairs: &[(&str, &str)]) -> Map<String, Value> {
    let mut out = Map::new();
    for (claude, yi) in pairs {
        if let Some(value) = args.get(*yi) {
            out.insert((*claude).to_owned(), value.clone());
        }
    }
    let named = |key: &str| pairs.iter().any(|(_, yi)| *yi == key);
    for (key, value) in args.iter().filter(|(key, _)| !named(key)) {
        out.insert(key.clone(), value.clone());
    }
    out
}

const BASH: [(&str, &str); 1] = [("command", "command")];
const READ: [(&str, &str); 3] = [
    ("file_path", "path"),
    ("offset", "offset"),
    ("limit", "limit"),
];
const WRITE: [(&str, &str); 2] = [("file_path", "path"), ("content", "content")];
const GREP: [(&str, &str); 5] = [
    ("pattern", "pattern"),
    ("path", "path"),
    ("glob", "include"),
    ("-i", "ignore_case"),
    ("multiline", "multiline"),
];

/// The changed span of a file as whole lines, with the lines both texts share trimmed.
fn changed_span(before: &str, after: &str) -> (String, String) {
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let (head, tail) = yi_types::diff::changed_edges(&old, &new);
    let span = |lines: &[&str]| {
        let end = lines.len().saturating_sub(tail);
        lines.get(head..end).unwrap_or_default().concat()
    };
    (span(&old), span(&new))
}

/// Claude's Edit input per file a patch changes; `patch` itself only when it is that file's alone.
fn edit_inputs(call: &Call) -> Vec<(Value, Option<String>)> {
    let mut files: Vec<_> = Vec::new();
    for path in crate::permission::extract_targets("edit", &call.args, &call.cwd) {
        let path = yi_permission::lexical_normalize(&path);
        if !files.contains(&path) {
            files.push(path);
        }
    }
    let patch = (call.args.get("patch")).filter(|_| files.len() == 1);
    let input = |path: &Path| {
        let mut input = Map::from_iter([("file_path".to_owned(), json!(path.to_string_lossy()))]);
        let change = (call.edits.iter())
            .find(|change| yi_permission::lexical_normalize(&change.path) == path);
        let notice = match change {
            Some(change) => {
                let (old, new) = changed_span(&change.before, &change.after);
                input.insert("old_string".to_owned(), Value::from(old));
                input.insert("new_string".to_owned(), Value::from(new));
                input.insert("replace_all".to_owned(), Value::Bool(false));
                None
            }
            None => Some(format!(
                "[Claude Code Edit hook input for {} has no old_string or new_string: Yi could not prepare this file's edit]",
                path.display()
            )),
        };
        input.extend(patch.map(|patch| ("patch".to_owned(), patch.clone())));
        (Value::Object(input), notice)
    };
    files.iter().map(|path| input(path)).collect()
}

/// One `tool_input` per file the call touches, with a row where its shape is short of Claude's.
pub(super) fn tool_inputs(call: &Call) -> Vec<(Value, Option<String>)> {
    let (tool, args, cwd) = (call.tool.as_str(), &call.args, call.cwd.as_path());
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    let mut input = match tool {
        "bash" => {
            let mut input = renamed(args, &BASH);
            if let Some(secs) = input.remove("timeout_secs").and_then(|secs| secs.as_u64()) {
                input.insert("timeout".to_owned(), Value::from(secs.saturating_mul(1000)));
            }
            input
        }
        "read" => renamed(args, &READ),
        "write" => renamed(args, &WRITE),
        "edit" => return edit_inputs(call),
        "grep" if applies(args) => {
            let pairs = [("old_string", "pattern"), ("new_string", "replace")];
            let mut input = renamed(args, &pairs);
            input.insert(
                "file_path".to_owned(),
                Value::from(text("path").unwrap_or(".")),
            );
            input.insert("replace_all".to_owned(), Value::Bool(true));
            input.remove("apply");
            input.remove("path");
            input
        }
        "grep" => {
            let mut input = renamed(args, &GREP);
            let mode = match (input.remove("files_with_matches"), input.remove("count")) {
                (_, Some(Value::Bool(true))) => "count",
                (Some(Value::Bool(true)), _) => "files_with_matches",
                _ => "content",
            };
            input.insert("output_mode".to_owned(), Value::from(mode));
            input
        }
        _ => args.clone(),
    };
    for key in ["file_path", "path"] {
        let raw = input.get(key).and_then(Value::as_str).map(str::to_owned);
        if let Some(raw) = raw.filter(|_| matches!(tool, "read" | "write" | "grep")) {
            input.insert(key.to_owned(), absolute(&raw, cwd));
        }
    }
    vec![(Value::Object(input), None)]
}

pub(crate) fn yi_args(
    call: &Call,
    updated: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let (tool, args) = (call.tool.as_str(), &call.args);
    let back = |pairs: &[(&str, &str)], mut updated: Map<String, Value>| {
        let mut out = Map::new();
        for (claude, yi) in pairs {
            if let Some(value) = updated.remove(*claude) {
                out.insert((*yi).to_owned(), value);
            }
        }
        out.extend(updated);
        out
    };
    let refused = |why: &str| Err(format!("its updatedInput cannot be applied: {why}"));
    match tool {
        "bash" => {
            let mut updated = updated;
            updated.remove("description");
            if updated
                .remove("run_in_background")
                .is_some_and(|value| value != Value::Bool(false))
            {
                return refused("Yi has no run_in_background; pass wait instead");
            }
            let timeout = updated.remove("timeout");
            let mut out = back(&BASH, updated);
            match timeout.as_ref().map(Value::as_u64) {
                None => {}
                Some(Some(ms)) if ms % 1000 == 0 => {
                    out.insert("timeout_secs".to_owned(), Value::from(ms / 1000));
                }
                Some(_) => return refused("timeout is not a whole number of seconds in ms"),
            }
            Ok(out)
        }
        "read" => Ok(back(&READ, updated)),
        "write" => Ok(back(&WRITE, updated)),
        "edit" => {
            let sent = edit_inputs(call);
            let same = |file: &Value| {
                let keys = ["file_path", "old_string", "new_string", "replace_all"];
                (keys.iter()).all(|key| updated.get(*key).is_none_or(|v| Some(v) == file.get(*key)))
            };
            match (sent.as_slice(), updated.get("patch")) {
                ([(only, _)], Some(patch)) if same(only) => {
                    Ok(Map::from_iter([("patch".to_owned(), patch.clone())]))
                }
                _ => refused(
                    "an edit maps back only from the patch of a one-file edit, its other fields as sent",
                ),
            }
        }
        "grep" if applies(args) => {
            refused("a grep rewrite has no Claude Code input to map back from")
        }
        "grep" => {
            let mut updated = updated;
            let mode = updated.remove("output_mode");
            let mut out = back(&GREP, updated);
            match mode.as_ref() {
                None => {}
                Some(Value::String(mode)) if mode == "content" => {}
                Some(Value::String(mode)) if mode == "files_with_matches" => {
                    out.insert("files_with_matches".to_owned(), Value::Bool(true));
                }
                Some(Value::String(mode)) if mode == "count" => {
                    out.insert("count".to_owned(), Value::Bool(true));
                }
                Some(Value::String(other)) => return refused(&format!("output_mode {other:?}")),
                Some(_) => return refused("output_mode is not a string"),
            }
            Ok(out)
        }
        _ => Ok(updated),
    }
}

fn capped(value: &mut Value, field: &str) {
    match value {
        Value::String(text) if text.len() > RESPONSE_CAP => {
            let total = text.len();
            let mut kept = RESPONSE_CAP;
            while !text.is_char_boundary(kept) {
                kept = kept.saturating_sub(1);
            }
            let head = text.get(..kept).unwrap_or_default();
            *text = format!(
                "{head}\n[… kept {kept} of {total} bytes of this {field} field (RESPONSE_CAP {RESPONSE_CAP}); a hook has no call that reads the rest]"
            );
        }
        Value::Array(items) => items.iter_mut().for_each(|value| capped(value, field)),
        Value::Object(fields) => fields.values_mut().for_each(|value| capped(value, field)),
        _ => {}
    }
}

fn flag(details: &Value, key: &str) -> bool {
    details.get(key) == Some(&Value::Bool(true))
}

/// The doc gives Bash's and Write's result objects; for every other tool it is silent, so
/// the response is Yi's result text.
fn tool_response(tool: &str, args: &Map<String, Value>, result: &ToolResult, cwd: &Path) -> Value {
    let text = yi_types::message::join_text(&result.content, "\n");
    match tool {
        "bash" => json!({
            "stdout": text,
            "stderr": "",
            "interrupted": flag(&result.details, "cancelled") || flag(&result.details, "timedOut"),
            "isImage": false,
        }),
        "write" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
            json!({"filePath": absolute(path, cwd)})
        }
        _ => Value::String(text),
    }
}

/// A post event's input: a failure's `error` opens `Exit code N` for bash, as Claude Code's does.
pub(super) fn after(call: Call, result: &ToolResult, failed: bool) -> (Event, Call) {
    let text = yi_types::message::join_text(&result.content, "\n");
    if !failed {
        let response = tool_response(&call.tool, &call.args, result, &call.cwd);
        let call = Call {
            response: Some(response),
            ..call
        };
        return (Event::PostToolUse, call);
    }
    let error = match result.details.get("exitCode").and_then(Value::as_i64) {
        Some(code) if call.tool == "bash" => format!("Exit code {code}\n{text}"),
        _ => text,
    };
    let interrupted = yi_types::event::ToolErrorKind::from_details(&result.details)
        == Some(yi_types::event::ToolErrorKind::Aborted.as_str())
        || flag(&result.details, "cancelled") && !flag(&result.details, "timedOut");
    let call = Call {
        error: Some(error),
        interrupted,
        ..call
    };
    (Event::PostToolUseFailure, call)
}

pub(super) fn replaced_output(tool: &str, value: &Value) -> Option<String> {
    match (tool, value) {
        ("bash", Value::Object(fields)) => {
            let stdout = fields.get("stdout")?.as_str()?;
            let stderr = fields
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(match stderr.is_empty() {
                true => stdout.to_owned(),
                false => format!("{stdout}\n{stderr}"),
            })
        }
        ("write", _) | (_, Value::Object(_) | Value::Array(_)) => None,
        (_, Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

pub(super) fn stdin(event: Event, call: &Call, name: &str, tool_input: Value) -> Vec<u8> {
    let mut input = Map::new();
    let mut put = |key: &str, value: Value| {
        input.insert(key.to_owned(), value);
    };
    if let Some(id) = &call.session_id {
        put("session_id", Value::from(id.as_str()));
    }
    if let Some(path) = &call.transcript_path {
        put(
            "transcript_path",
            Value::from(path.to_string_lossy().into_owned()),
        );
    }
    put("cwd", Value::from(call.cwd.to_string_lossy().into_owned()));
    if let Some(mode) = call.permission_mode {
        put("permission_mode", Value::from(mode));
    }
    if let Some(agent) = &call.agent_id {
        put("agent_id", Value::from(agent.as_str()));
    }
    put("hook_event_name", Value::from(event.as_str()));
    put("tool_name", Value::from(name));
    // Yi's own tools take an MCP name, so a policy keyed on `mcp_server.source` can tell them apart.
    if name.starts_with("mcp__yi__") {
        put("mcp_server", json!({"name": "yi", "source": "sdk"}));
    }
    put("tool_input", tool_input);
    put("tool_use_id", Value::from(call.tool_use_id.as_str()));
    if let (Event::PostToolUse, Some(response)) = (event, &call.response) {
        let mut response = response.clone();
        capped(&mut response, "tool_response");
        put("tool_response", response);
    }
    if let (Event::PostToolUseFailure, Some(error)) = (event, &call.error) {
        let mut error = Value::from(error.as_str());
        capped(&mut error, "error");
        put("error", error);
        put("is_interrupt", Value::Bool(call.interrupted));
    }
    serde_json::to_vec(&Value::Object(input)).unwrap_or_default()
}

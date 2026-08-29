use std::path::Path;

use serde_json::{Map, Value};

/// Host-authored next steps: deterministic, at most two lines, and immutable
/// once written, because a recomputed line would move transcript bytes.
pub const NEXT: &str = "next: ";

pub fn spawned(name: &str, session_dir: &Path) -> String {
    format!(
        "{NEXT}await rlm.wait(120) blocks until this child reports; rlm.send('{name}', 'line') steers it; its transcript is {}/*.jsonl",
        session_dir.display()
    )
}

pub fn child_finished(name: &str) -> String {
    format!(
        "{NEXT}await rlm.result('{name}', schema=…) validates the answer host-side; the child stays addressable for follow-ups"
    )
}

pub fn grid_empty() -> Option<String> {
    Some(format!(
        "{NEXT}an empty grid answer means it cannot prove the relationship, not that the code is absent; grep to close the gap"
    ))
}

pub fn compacted(session_file: Option<&Path>) -> String {
    match session_file {
        Some(path) => format!(
            "{NEXT}the window now holds a summary plus the recent turns; the full history stays in {}",
            path.display()
        ),
        None => format!("{NEXT}the window now holds a summary plus the recent turns"),
    }
}

pub fn call_template(tool: &str, schema: &Value, arguments: &Map<String, Value>) -> String {
    let properties = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut shape = Map::new();
    if let Some(properties) = properties {
        for (key, spec) in properties {
            let wanted = required.contains(&key.as_str()) || arguments.contains_key(key);
            if !wanted {
                continue;
            }
            let value = arguments.get(key).cloned().unwrap_or_else(|| {
                Value::String(format!(
                    "<{}>",
                    spec.get("type").and_then(Value::as_str).unwrap_or("value")
                ))
            });
            shape.insert(key.clone(), value);
        }
    }
    let rendered = serde_json::to_string(&Value::Object(shape)).unwrap_or_else(|_| "{}".to_owned());
    format!("{NEXT}call {tool} as {rendered}")
}

pub fn append(result: &mut yi_types::event::ToolResult, line: &str) {
    use yi_types::message::Content;
    match result.content.last_mut() {
        Some(Content::Text { text, .. }) => {
            text.push('\n');
            text.push_str(line);
        }
        _ => result.content.push(Content::Text {
            text: line.to_owned(),
            text_signature: None,
        }),
    }
}

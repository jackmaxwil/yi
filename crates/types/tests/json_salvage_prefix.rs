use std::error::Error;

use serde_json::{Map, Value};
use yi_types::json_salvage::parse_streaming_json;

fn recorded_tool_call(line: usize, tool: &str) -> Result<Value, Box<dyn Error>> {
    let path = crate::workspace_root()?.join("crates/tui/tests/fixtures/sessions/01a0d6e4.jsonl");
    let body = std::fs::read_to_string(&path)?;
    let row = body.lines().nth(line.saturating_sub(1)).ok_or("row")?;
    let entry: Value = serde_json::from_str(row)?;
    let content = entry
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .ok_or("content")?;
    content
        .iter()
        .find(|item| {
            item.get("type").and_then(Value::as_str) == Some("toolCall")
                && item.get("name").and_then(Value::as_str) == Some(tool)
        })
        .and_then(|item| item.get("arguments"))
        .cloned()
        .ok_or_else(|| format!("no {tool} call on row {line}").into())
}

fn consistent(partial: &Value, complete: &Value) -> bool {
    match (partial, complete) {
        (Value::Null, _) => true,
        (Value::String(partial), Value::String(complete)) => complete.starts_with(partial),
        (Value::Array(partial), Value::Array(complete)) => {
            partial.len() <= complete.len()
                && partial
                    .iter()
                    .zip(complete)
                    .all(|(partial, complete)| consistent(partial, complete))
        }
        (Value::Object(partial), Value::Object(complete)) => partial.iter().all(|(key, value)| {
            complete
                .get(key)
                .is_some_and(|complete| consistent(value, complete))
        }),
        _ => partial == complete,
    }
}

fn completed_keys(partial: &Map<String, Value>, complete: &Value) -> usize {
    partial
        .iter()
        .filter(|(key, value)| complete.get(key.as_str()) == Some(*value))
        .count()
}

#[test]
fn every_prefix_of_a_real_tool_call_salvages_a_consistent_object() -> Result<(), Box<dyn Error>> {
    for (line, tool) in [(40, "bash"), (15, "todo")] {
        let complete = recorded_tool_call(line, tool)?;
        let stream = serde_json::to_string(&complete)?;
        let mut completed = 0;
        for end in 1..=stream.len() {
            let Some(prefix) = stream.get(..end) else {
                continue;
            };
            let partial = parse_streaming_json(prefix);
            for (key, value) in &partial {
                let full = complete
                    .get(key)
                    .ok_or_else(|| format!("{tool} row {line} prefix {end} invents {key}"))?;
                assert!(
                    consistent(value, full),
                    "{tool} row {line} prefix {end} key {key} reads {value} against {full}"
                );
            }
            let now = completed_keys(&partial, &complete);
            assert!(
                now >= completed,
                "{tool} row {line} prefix {end} drops completed keys: {completed} -> {now}"
            );
            completed = now;
        }
    }
    Ok(())
}

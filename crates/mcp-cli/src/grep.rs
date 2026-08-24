use serde_json::{Value, json};

use crate::sessions::SessionsStore;

fn matches(haystack: Option<&str>, needle: &str) -> bool {
    haystack.is_some_and(|text| text.to_lowercase().contains(needle))
}

fn matching_tools(snapshot: &Value, needle: &str) -> Vec<Value> {
    snapshot
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| {
                    matches(tool.get("name").and_then(Value::as_str), needle)
                        || matches(tool.get("description").and_then(Value::as_str), needle)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Progressive discovery (design §5.2): case-insensitive substring search
/// over cached connect-time snapshots — the model greps instead of listing,
/// and no server is contacted. Exit convention: Some = matches, None = none.
pub fn grep_sessions(
    store: &SessionsStore,
    sessions: &[String],
    pattern: &str,
    max_results: Option<usize>,
) -> Option<Value> {
    let needle = pattern.to_lowercase();
    let mut results = Vec::new();
    let mut remaining = max_results.unwrap_or(usize::MAX);
    for name in sessions {
        if remaining == 0 {
            break;
        }
        let Some(snapshot) = store.read_snapshot(name) else {
            continue;
        };
        let mut tools = matching_tools(&snapshot, &needle);
        tools.truncate(remaining);
        remaining = remaining.saturating_sub(tools.len());
        let instructions = snapshot
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|text| text.to_lowercase().contains(&needle))
            .map(str::to_owned);
        if !tools.is_empty() || instructions.is_some() {
            results.push(json!({
                "sessionName": name,
                "tools": tools,
                "instructions": instructions,
            }));
        }
    }
    if results.is_empty() {
        None
    } else {
        Some(Value::Array(results))
    }
}

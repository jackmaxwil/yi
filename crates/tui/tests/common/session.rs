use yi_types::message::AgentMessage;
// A scrubbed v4 session fixture's message entries, ordered by `seq`.

#[allow(dead_code, reason = "the shared harness serves several test binaries")]
pub fn session_entries(text: &str) -> Result<Vec<AgentMessage>, serde_json::Error> {
    use serde_json::Value;
    let mut entries: Vec<(u64, AgentMessage)> = Vec::new();
    for line in text.lines() {
        let row: Value = serde_json::from_str(line)?;
        let field = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
        if (field("kind"), field("type"), field("lane"))
            != (
                Some("entry".to_owned()),
                Some("message".to_owned()),
                Some("main".to_owned()),
            )
        {
            continue;
        }
        let Some(message) = row.get("message") else {
            continue;
        };
        let seq = row.get("seq").and_then(Value::as_u64).unwrap_or(0);
        entries.push((seq, serde_json::from_value(message.clone())?));
    }
    entries.sort_by_key(|(seq, _)| *seq);
    Ok(entries.into_iter().map(|(_, message)| message).collect())
}

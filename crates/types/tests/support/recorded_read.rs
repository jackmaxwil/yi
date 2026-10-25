use {serde_json::Value, std::error::Error};
/// The `read` tool result tui fixture session 01a129b6 recorded, its text mapped by `text`.
pub fn recorded_read(text: impl FnOnce(&str) -> String) -> Result<Value, Box<dyn Error>> {
    let line = include_str!("../../../tui/tests/fixtures/sessions/01a129b6.jsonl")
        .lines()
        .find(|line| line.contains("\"role\":\"toolResult\""))
        .ok_or("the fixture lost its tool result")?;
    let mut message = serde_json::from_str::<Value>(line)?["message"].take();
    let text = text(message["content"][0]["text"].as_str().unwrap_or_default());
    message["content"][0]["text"] = Value::String(text);
    Ok(message)
}

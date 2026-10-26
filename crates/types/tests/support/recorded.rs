//! A line the assistant said in the recorded session `01a0d6e4` (scrubbed by `evals/record.py`):
//! a model's own wording, case and length. Shared by `#[path]` includes from other crates' tests.

use std::error::Error;

pub fn recorded_line(phrase: &str) -> Result<String, Box<dyn Error>> {
    for row in include_str!("../../../tui/tests/fixtures/sessions/01a0d6e4.jsonl").lines() {
        let row: serde_json::Value = serde_json::from_str(row)?;
        let blocks = row["message"]["content"].as_array().into_iter().flatten();
        let lines = blocks
            .filter_map(|block| block["text"].as_str())
            .flat_map(str::lines);
        if let Some(line) = lines.into_iter().find(|line| line.contains(phrase)) {
            return Ok(line.to_owned());
        }
    }
    Err("the recording lost its line".into())
}

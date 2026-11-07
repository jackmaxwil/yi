use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext};
use yi_tui::cell::{ToolCell, TranscriptMode};
use yi_types::message::Content;

use crate::scratch::Scratch;
use crate::tool_cell_support::{rendered, tool_cell};

type TestResult = Result<(), Box<dyn Error>>;

fn cell(
    tools: &[Arc<dyn Tool>],
    name: &str,
    argument: &str,
    input: Map<String, Value>,
    context: &ToolContext,
) -> Result<ToolCell, Box<dyn Error>> {
    let tool = tools
        .iter()
        .find(|tool| tool.name() == name)
        .ok_or_else(|| format!("no {name} tool"))?;
    let output = tool.execute(input, context);
    let text = match output.result.content.first() {
        Some(Content::Text { text, .. }) => text.clone(),
        _ => return Err(format!("{name} returned no text").into()),
    };
    Ok(tool_cell(
        name,
        argument,
        &text,
        &output.result.details,
        output.is_error,
    ))
}

fn row(line: &str) -> &str {
    line.strip_prefix("│ ").unwrap_or(line).trim()
}

#[test]
fn real_tool_results_drive_read_grep_and_edit_cards() -> TestResult {
    let dir = Scratch::new("yi-real-tool-cells")?;
    std::fs::write(
        dir.join("sample.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )?;
    let mut context = ToolContext::new(dir.to_path_buf());
    let tools = yi_tools::builtin_tools();
    let args = |pairs: &[(&str, Value)]| {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    };
    let read = cell(
        &tools,
        "read",
        "sample.rs",
        args(&[("path", json!("sample.rs"))]),
        &context,
    )?;
    let header = read
        .preview
        .first()
        .ok_or("read returned no hashline header")?;
    let edit = cell(
        &tools,
        "edit",
        "sample.rs",
        args(&[(
            "patch",
            json!(format!(
                "{header}\nPUT 2.=2:\n+    println!(\"changed\");\n+    println!(\"changed again\");"
            )),
        )]),
        &context,
    )?;
    context.program = true;
    let grep = cell(
        &tools,
        "grep",
        "sample.rs",
        args(&[("pattern", json!("changed")), ("path", json!("sample.rs"))]),
        &context,
    )?;

    assert_eq!(read.digest.as_deref(), Some("3 lines"));
    let read_text = read.preview.join("\n");
    assert_eq!(
        ToolCell::digest_of("read", &read_text, &Value::Null, false).as_deref(),
        Some("3 lines")
    );
    let read_body = rendered(&read, 100, TranscriptMode::Verbose).join("\n");
    assert!(
        read_body
            .lines()
            .any(|line| row(line).starts_with("1 ") && line.contains("fn main()")),
        "{read_body}"
    );
    assert!(!read_body.contains("1:fn main()"), "{read_body}");
    assert!(!read_body.contains("sample.rs#"), "{read_body}");

    assert_eq!(grep.digest.as_deref(), Some("2 hits"));
    let grep_body = rendered(&grep, 100, TranscriptMode::Verbose).join("\n");
    assert_eq!(
        grep_body
            .lines()
            .filter(|line| row(line) == "sample.rs")
            .count(),
        1,
        "{grep_body}"
    );
    assert!(
        grep_body
            .lines()
            .any(|line| row(line).starts_with("2 ") && line.contains("changed")),
        "{grep_body}"
    );
    assert!(
        grep_body
            .lines()
            .any(|line| row(line).starts_with("3 ") && line.contains("changed again")),
        "{grep_body}"
    );
    assert!(!grep_body.contains("sample.rs:"), "{grep_body}");

    assert_eq!(edit.digest.as_deref(), Some("updated; +2 -1 at line 2"));
    let edit_body = rendered(&edit, 100, TranscriptMode::Verbose).join("\n");
    assert!(edit_body.contains("changed"), "{edit_body}");
    Ok(())
}

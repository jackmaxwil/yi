use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_types::message::Content;

use crate::scratch::Scratch;

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
    Ok(ToolCell {
        name: name.to_owned(),
        call_id: String::new(),
        status: if output.is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        },
        summary: ToolCell::summary_of(name, argument),
        digest: ToolCell::digest_of(name, &text, &output.result.details, output.is_error),
        preview: text.lines().map(str::to_owned).collect(),
        elapsed_ms: 0,
        calls: 1,
        details: output.result.details,
    })
}

fn rendered(cell: &ToolCell, mode: TranscriptMode) -> String {
    Cell::Tool(cell.clone())
        .lines(100, &Theme::new(ColorTier::Ansi16, true), mode, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn real_tool_results_drive_read_grep_and_edit_cards() -> TestResult {
    let dir = Scratch::new("yi-real-tool-cells")?;
    std::fs::write(
        dir.join("sample.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )?;
    let context = ToolContext::new(dir.to_path_buf());
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
            json!(format!("{header}\nPUT 2.=2:\n+    println!(\"changed\");")),
        )]),
        &context,
    )?;
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
    let read_body = rendered(&read, TranscriptMode::Verbose);
    assert!(read_body.contains("fn main()"), "{read_body}");
    assert!(!read_body.contains("1:fn main()"), "{read_body}");
    assert!(!read_body.contains("sample.rs#"), "{read_body}");

    assert_eq!(grep.digest.as_deref(), Some("1 hit"));
    let grep_body = rendered(&grep, TranscriptMode::Verbose);
    assert!(grep_body.contains("sample.rs"), "{grep_body}");
    assert!(grep_body.contains("changed"), "{grep_body}");

    assert_eq!(
        edit.digest.as_deref(),
        Some("updated; first change at line 2")
    );
    let edit_body = rendered(&edit, TranscriptMode::Verbose);
    assert!(edit_body.contains("changed"), "{edit_body}");
    Ok(())
}

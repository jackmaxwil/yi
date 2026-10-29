use std::error::Error;
use std::fs;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::{HashlineEditTool, HashlineReadTool, SharedHashline};
use yi_tools::{Tool, ToolContext, ToolOutput};

use crate::tools::{output_text, temp_dir};

type TestResult = Result<(), Box<dyn Error>>;

const CAP: usize = 16 * 1024 * 1024;

fn path_args(key: &str, value: &str) -> Map<String, Value> {
    let mut args = Map::new();
    args.insert(key.to_owned(), json!(value));
    args
}

fn read(state: &SharedHashline, path: &str, context: &ToolContext) -> ToolOutput {
    HashlineReadTool::new(Arc::clone(state)).execute(path_args("path", path), context)
}

fn edit(state: &SharedHashline, patch: &str, context: &ToolContext) -> ToolOutput {
    let tool = HashlineEditTool {
        state: Arc::clone(state),
        freeform_grammar: false,
    };
    tool.execute(path_args("patch", patch), context)
}

fn tag(output: &ToolOutput) -> Result<String, Box<dyn Error>> {
    let text = output_text(output);
    let header = text.lines().next().ok_or("empty output")?;
    let (_, tail) = header.rsplit_once('#').ok_or("no tag")?;
    Ok(tail.trim_end_matches(']').to_owned())
}

fn text_of(bytes: usize) -> String {
    let line = format!("{}\n", "x".repeat(1023));
    let mut text = line.repeat(bytes / 1024);
    text.push_str(&"y".repeat(bytes % 1024));
    text
}

#[test]
fn a_file_at_the_read_cap_reads_and_one_byte_past_names_the_cap() -> TestResult {
    let dir = temp_dir("read-cap")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    fs::write(dir.join("at.txt"), text_of(CAP))?;
    fs::write(dir.join("past.txt"), text_of(CAP + 1))?;

    let at = read(&state, "at.txt", &context);
    assert!(!at.is_error, "{}", output_text(&at));
    let past = read(&state, "past.txt", &context);
    let text = output_text(&past);
    assert!(past.is_error, "{text}");
    for needle in [
        "0 of 16777217 bytes read",
        "READ_FILE_CAP 16777216",
        "grep path=",
    ] {
        assert!(text.contains(needle), "no {needle:?}: {text}");
    }
    Ok(())
}

/// Past the 64 MiB the snapshot store holds, one view evicted every other path's tags.
#[test]
fn a_huge_read_leaves_other_files_tags_editable() -> TestResult {
    let dir = temp_dir("read-cap-evict")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    fs::write(dir.join("a.txt"), "one\ntwo\n")?;
    fs::write(dir.join("huge.txt"), text_of(65 * 1024 * 1024))?;

    let tag = tag(&read(&state, "a.txt", &context))?;
    let _huge = read(&state, "huge.txt", &context);
    let edited = edit(
        &state,
        &format!("[a.txt#{tag}]\nPUT 2.=2:\n+TWO\n"),
        &context,
    );
    assert!(!edited.is_error, "{}", output_text(&edited));
    assert_eq!(fs::read_to_string(dir.join("a.txt"))?, "one\nTWO\n");
    Ok(())
}

#[test]
fn a_text_file_quoting_the_pdf_marker_edits_and_a_pdf_does_not() -> TestResult {
    let dir = temp_dir("read-pdf-marker")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    fs::write(
        dir.join("notes.md"),
        "# Notes\n\nA PDF opens with %PDF-1.7.\n",
    )?;
    let pdf = "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n";
    fs::write(dir.join("real.pdf"), pdf)?;

    let notes = tag(&read(&state, "notes.md", &context))?;
    let patch = format!("[notes.md#{notes}]\nPUT 1.=1:\n+# Reading notes\n");
    let edited = edit(&state, &patch, &context);
    assert!(!edited.is_error, "{}", output_text(&edited));

    let real = tag(&read(&state, "real.pdf", &context))?;
    let refused = edit(
        &state,
        &format!("[real.pdf#{real}]\nPUT 1.=1:\n+x\n"),
        &context,
    );
    assert!(
        output_text(&refused).contains("is a binary document"),
        "{}",
        output_text(&refused)
    );
    Ok(())
}

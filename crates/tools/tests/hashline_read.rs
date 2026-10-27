use std::error::Error;
use std::fs;

use serde_json::{Map, Value, json};
use yi_tools::hashline::tool::HashlineReadTool;
use yi_tools::{Tool, ToolContext, ToolOutput};
use yi_types::message::Content;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn text(output: &ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|part| match part {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

fn read(dir: &Scratch, arguments: Map<String, Value>) -> ToolOutput {
    let state = yi_tools::hashline::tool::shared_hashline_state();
    HashlineReadTool::new(state).execute(arguments, &ToolContext::new(dir.to_path_buf()))
}

#[test]
fn an_existing_bracketed_path_reads_as_a_file() -> TestResult {
    let dir = Scratch::new("yi-tools-read-brackets")?;
    fs::create_dir_all(dir.join("app/[id]"))?;
    fs::write(
        dir.join("app/[id]/page.tsx"),
        "export default function Page() {}\n",
    )?;
    let output = read(&dir, args(&[("path", json!("app/[id]/page.tsx"))]));
    assert!(!output.is_error, "{}", text(&output));
    assert!(text(&output).contains("1:export default function Page() {}"));
    let first = text(&output);
    let tag = first
        .lines()
        .next()
        .and_then(|line| line.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag")?;
    let tagged = read(
        &dir,
        args(&[("path", json!(format!("app/[id]/page.tsx#{tag}")))]),
    );
    assert!(!tagged.is_error, "{}", text(&tagged));
    assert!(text(&tagged).contains("1:export default function Page() {}"));
    Ok(())
}

#[test]
fn string_offsets_and_limits_parse_or_name_the_invalid_field() -> TestResult {
    let dir = Scratch::new("yi-tools-read-string-args")?;
    let body: String = (1..=205).map(|line| format!("row {line}\n")).collect();
    fs::write(dir.join("rows.txt"), body)?;
    let output = read(
        &dir,
        args(&[
            ("path", json!("rows.txt")),
            ("offset", json!("200")),
            ("limit", json!("1")),
        ]),
    );
    assert!(text(&output).contains("200:row 200"), "{}", text(&output));
    assert!(!text(&output).contains("199:row 199"));
    for (field, value) in [("offset", "later"), ("limit", "many")] {
        let output = read(
            &dir,
            args(&[("path", json!("rows.txt")), (field, json!(value))]),
        );
        assert!(output.is_error);
        assert!(text(&output).contains(field), "{}", text(&output));
    }
    Ok(())
}

#[test]
fn zero_limits_and_multiline_find_needles_are_refused_with_their_rules() -> TestResult {
    let dir = Scratch::new("yi-tools-read-refusals")?;
    fs::write(dir.join("rows.txt"), "first\nsecond\n")?;
    let zero = read(
        &dir,
        args(&[("path", json!("rows.txt")), ("limit", json!(0))]),
    );
    assert!(zero.is_error);
    assert!(text(&zero).contains("limit must be a positive integer"));
    let find = read(
        &dir,
        args(&[
            ("path", json!("rows.txt")),
            ("find", json!("first\nsecond")),
        ]),
    );
    assert!(find.is_error);
    assert!(text(&find).contains("find must be a single line"));
    Ok(())
}

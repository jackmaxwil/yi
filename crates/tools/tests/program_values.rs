//! A kernel program's values: whole past the model's caps, every fuse named where it cuts, and
//! a program's reads kept apart from what the model has seen.
use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn tools() -> Vec<Arc<dyn Tool>> {
    yi_tools::builtin_tools()
}

fn tool<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> Result<&'a Arc<dyn Tool>, String> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .ok_or(format!("no {name}"))
}

fn call(tool: &Arc<dyn Tool>, input: Value, context: &ToolContext) -> yi_tools::ToolOutput {
    let input: Map<String, Value> = input.as_object().cloned().unwrap_or_default();
    tool.execute(input, context)
}

fn program(dir: &Scratch) -> ToolContext {
    let mut context = ToolContext::new(dir.to_path_buf());
    context.program = true;
    context
}

fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

#[test]
fn a_program_read_from_an_offset_runs_to_the_end_of_the_file() -> TestResult {
    let dir = Scratch::new("yi-values-offset")?;
    std::fs::write(dir.join("f.txt"), numbered(5000))?;
    let tools = tools();
    let read = tool(&tools, "read")?;
    let output = call(
        read,
        json!({"path": "f.txt", "offset": 2500}),
        &program(&dir),
    );
    let value = output.value.ok_or("no value")?;
    let lines = value["lines"].as_array().ok_or("no lines")?;
    let last = lines.last().map(|line| line["n"].clone());
    assert_eq!(
        (lines.len(), last),
        (2501, Some(json!(5000))),
        "2,000 is the model's page"
    );
    assert_eq!(value["cut"], Value::Null);
    Ok(())
}

/// The model may edit only lines it was shown; a program's whole-file read must not lend it more.
#[test]
fn a_program_read_lets_a_program_edit_its_lines_and_the_model_none() -> TestResult {
    let dir = Scratch::new("yi-values-seen")?;
    std::fs::write(dir.join("f.txt"), numbered(3000))?;
    std::fs::write(dir.join("g.txt"), numbered(3000))?;
    let tools = tools();
    // A refused edit shows the lines it names and marks them seen, so each caller gets a file.
    let last_line = |name: &str| -> Result<Value, Box<dyn Error>> {
        let read = call(tool(&tools, "read")?, json!({"path": name}), &program(&dir));
        let tag = read.value.ok_or("no value")?["tag"].clone();
        let tag = tag.as_str().unwrap_or_default().to_owned();
        Ok(json!({"patch": format!("[{name}#{tag}]\nPUT 3000.=3000:\n+edited\n")}))
    };
    let model = ToolContext::new(dir.to_path_buf());
    let refused = call(tool(&tools, "edit")?, last_line("f.txt")?, &model);
    assert!(
        refused.is_error,
        "the model edited a line only a program read"
    );
    let edited = call(tool(&tools, "edit")?, last_line("g.txt")?, &program(&dir));
    assert!(!edited.is_error, "{:?}", edited.result.content);
    assert!(std::fs::read_to_string(dir.join("g.txt"))?.ends_with("edited\n"));
    Ok(())
}

/// Each fuse trips here and names itself, kept against total, with the call that gets the rest.
#[test]
fn every_program_fuse_names_its_cut() -> TestResult {
    let dir = Scratch::new("yi-values-fuse")?;
    let half = 33 * 1024 * 1024;
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), format!("needle\n{}\n", "x".repeat(half)))?;
    }
    let tools = tools();
    let grep = call(
        tool(&tools, "grep")?,
        json!({"pattern": "needle"}),
        &program(&dir),
    );
    let value = grep.value.ok_or("no grep value")?;
    let cut = value["cut"].as_str().unwrap_or_default();
    assert!(cut.contains("the 64 MiB program fuse"), "{cut}");
    let line = "y".repeat(1023);
    let lines: String = (0..66 * 1024).map(|_| format!("{line}\n")).collect();
    std::fs::write(dir.join("long.txt"), lines)?;
    let read = call(
        tool(&tools, "read")?,
        json!({"path": "long.txt"}),
        &program(&dir),
    );
    let value = read.value.ok_or("no read value")?;
    let cut = value["cut"].as_str().unwrap_or_default();
    let kept = value["lines"].as_array().map_or(0, Vec::len);
    let rest = format!("offset={}", kept + 1);
    assert!(
        cut.contains("of 67584: the program fuse is 64 MiB") && cut.contains(&rest),
        "{cut}"
    );
    std::fs::write(dir.join("many.txt"), "hit\n".repeat(100_001))?;
    let grep = call(
        tool(&tools, "grep")?,
        json!({"pattern": "hit"}),
        &program(&dir),
    );
    let value = grep.value.ok_or("no grep value")?;
    let cut = value["cut"].as_str().unwrap_or_default();
    let kept = value["matches"].as_array().map_or(0, Vec::len);
    assert_eq!(kept, 100_000);
    assert!(
        cut.contains("of 100001: the program fuse is 100000 items")
            && cut.contains("narrow the pattern or the path"),
        "{cut}"
    );
    let read = call(
        tool(&tools, "read")?,
        json!({"path": "many.txt"}),
        &program(&dir),
    );
    let value = read.value.ok_or("no read value")?;
    let cut = value["cut"].as_str().unwrap_or_default();
    let kept = value["lines"].as_array().map_or(0, Vec::len);
    assert_eq!(kept, 100_000);
    let rest = format!("offset={}", kept + 1);
    assert!(
        cut.contains("of 100001: the program fuse is 100000 items") && cut.contains(&rest),
        "{cut}"
    );
    std::fs::write(dir.join("one.txt"), "z".repeat(65 * 1024 * 1024))?;
    let read = call(
        tool(&tools, "read")?,
        json!({"path": "one.txt"}),
        &program(&dir),
    );
    let cut = read.value.ok_or("no value")?["cut"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(cut.starts_with("[line 1 alone is 68157440 bytes"), "{cut}");
    Ok(())
}

#[test]
fn a_backgrounded_command_hands_a_program_its_job_then_its_output() -> TestResult {
    let dir = Scratch::new("yi-values-job")?;
    let tools = tools();
    let bash = tool(&tools, "bash")?;
    let started = call(
        bash,
        json!({"command": "sleep 6; echo done", "wait": 5}),
        &program(&dir),
    );
    let job = started.value.ok_or("no value")?["job"].clone();
    assert!(job.is_u64(), "{:?}", started.result.content);
    let polled = call(bash, json!({"job": job, "wait": 30}), &program(&dir));
    let value = polled.value.ok_or("no polled value")?;
    assert_eq!(
        (value["exitCode"].clone(), value["job"].clone()),
        (json!(0), job)
    );
    assert!(
        value["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("done")
    );
    Ok(())
}

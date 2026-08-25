use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{BashTool, GlobTool, GrepTool, Tool, ToolContext, WriteTool, discover_exec_tools};
use yi_types::message::Content;

type TestResult = Result<(), Box<dyn Error>>;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> Result<TempDir, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-tools-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(TempDir(dir))
}

fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn output_text(output: &yi_tools::ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn write_then_read_round_trips_through_the_working_directory() -> TestResult {
    let dir = temp_dir("read-write")?;
    let context = ToolContext::new(dir.0.clone());

    let written = WriteTool::default().execute(
        args(&[
            ("path", json!("notes/hello.txt")),
            ("content", json!("line one\nline two\nline three")),
        ]),
        &context,
    );
    assert!(!written.is_error, "{}", output_text(&written));

    let read = yi_tools::hashline::tool::HashlineReadTool {
        state: yi_tools::hashline::tool::shared_hashline_state(),
    }
    .execute(
        args(&[
            ("path", json!("notes/hello.txt")),
            ("offset", json!(2)),
            ("limit", json!(1)),
        ]),
        &context,
    );
    assert!(!read.is_error);
    let text = output_text(&read);
    assert!(
        text.lines()
            .next()
            .is_some_and(|l| l.starts_with("[notes/hello.txt#")),
        "{text}"
    );
    assert!(text.contains("2:line two"), "{text}");
    Ok(())
}

#[test]
fn read_reports_a_missing_file_as_a_tool_error() -> TestResult {
    let dir = temp_dir("read-missing")?;
    let context = ToolContext::new(dir.0.clone());
    let read = yi_tools::hashline::tool::HashlineReadTool {
        state: yi_tools::hashline::tool::shared_hashline_state(),
    }
    .execute(args(&[("path", json!("absent.txt"))]), &context);
    assert!(read.is_error);
    Ok(())
}

#[test]
fn glob_matches_relative_patterns_and_skips_git() -> TestResult {
    let dir = temp_dir("glob")?;
    fs::create_dir_all(dir.0.join("src"))?;
    fs::create_dir_all(dir.0.join(".git"))?;
    fs::write(dir.0.join("src/a.rs"), "")?;
    fs::write(dir.0.join("src/b.txt"), "")?;
    fs::write(dir.0.join(".git/c.rs"), "")?;
    let context = ToolContext::new(dir.0.clone());

    let found = GlobTool.execute(args(&[("pattern", json!("**/*.rs"))]), &context);
    let text = output_text(&found);
    assert!(text.contains("a.rs"));
    assert!(!text.contains("b.txt"));
    assert!(!text.contains(".git"));
    Ok(())
}

#[test]
fn grep_returns_path_line_hits_and_respects_case_flag() -> TestResult {
    let dir = temp_dir("grep")?;
    fs::write(dir.0.join("one.txt"), "alpha\nNEEDLE here\nomega")?;
    let context = ToolContext::new(dir.0.clone());

    let missed = GrepTool.execute(args(&[("pattern", json!("needle"))]), &context);
    assert_eq!(output_text(&missed), "No matches found");

    let hit = GrepTool.execute(
        args(&[("pattern", json!("needle")), ("ignore_case", json!(true))]),
        &context,
    );
    let text = output_text(&hit);
    assert!(text.contains("one.txt:2:NEEDLE here"), "{text}");
    Ok(())
}

#[test]
fn bash_reports_output_exit_code_and_stderr() -> TestResult {
    let dir = temp_dir("bash")?;
    let context = ToolContext::new(dir.0.clone());

    let ok = BashTool.execute(args(&[("command", json!("echo hello"))]), &context);
    assert!(!ok.is_error);
    assert_eq!(output_text(&ok).trim(), "hello");

    let failed = BashTool.execute(
        args(&[("command", json!("echo oops >&2; exit 3"))]),
        &context,
    );
    assert!(failed.is_error);
    let text = output_text(&failed);
    assert!(text.contains("oops"));
    assert!(text.contains("exit code: 3"));
    Ok(())
}

#[test]
fn bash_kills_a_running_command_when_cancelled() -> TestResult {
    let dir = temp_dir("bash-cancel")?;
    let mut context = ToolContext::new(dir.0.clone());
    context.cancelled = Arc::new(|| true);

    let start = std::time::Instant::now();
    let aborted = BashTool.execute(args(&[("command", json!("sleep 30"))]), &context);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(aborted.is_error);
    assert!(output_text(&aborted).contains("[command aborted]"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn discovers_and_runs_an_exec_tool_via_the_schema_contract() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("exec")?;
    let script = dir.0.join("greet");
    fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--schema\" ]; then\n  printf '{\"name\":\"greet\",\"description\":\"greets\",\"input_schema\":{\"type\":\"object\"},\"kind\":\"read\"}'\n  exit 0\nfi\nread line\nprintf 'greeting for %s' \"$line\"\n",
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    fs::write(dir.0.join("not-executable.txt"), "ignored")?;

    let tools = discover_exec_tools(&dir.0);
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.name(), "greet");

    let context = ToolContext::new(dir.0.clone());
    let output = tool.execute(args(&[("who", json!("yi"))]), &context);
    assert!(!output.is_error, "{}", output_text(&output));
    assert!(output_text(&output).contains("greeting for"));
    Ok(())
}

#[test]
fn glob_skips_gitignored_paths() -> TestResult {
    let dir = temp_dir("gitignore")?;
    let root = &dir.0;
    fs::write(root.join(".gitignore"), "target/\n*.log\n!keep.log\n")?;
    fs::write(root.join("main.rs"), "")?;
    fs::write(root.join("noisy.log"), "")?;
    fs::write(root.join("keep.log"), "")?;
    fs::create_dir_all(root.join("target/debug"))?;
    fs::write(root.join("target/debug/main.rs"), "")?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), "")?;
    fs::write(root.join("src/.gitignore"), "lib.rs\n")?;

    let listed = yi_tools::list_files(root, 100);
    assert!(listed.contains(&"main.rs".to_owned()));
    assert!(listed.contains(&"keep.log".to_owned()));
    assert!(!listed.contains(&"noisy.log".to_owned()));
    assert!(!listed.iter().any(|path| path.starts_with("target")));
    assert!(!listed.contains(&"src/lib.rs".to_owned()));

    let context = ToolContext::new(root.clone());
    let output = GlobTool.execute(args(&[("pattern", json!("**/*.rs"))]), &context);
    let text = output_text(&output);
    assert!(text.contains("main.rs"));
    assert!(!text.contains("target"));
    Ok(())
}

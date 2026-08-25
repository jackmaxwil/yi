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

#[test]
fn checkpoint_restore_reverts_a_turn() -> TestResult {
    let project = temp_dir("checkpoint-project")?;
    let shadow = temp_dir("checkpoint-shadow")?;
    fs::write(project.0.join("kept.txt"), "before\n")?;
    fs::write(project.0.join("removed.txt"), "gone\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow.0, &project.0)?;
    let turn_start = checkpoints.capture()?;

    fs::write(project.0.join("kept.txt"), "after\n")?;
    fs::remove_file(project.0.join("removed.txt"))?;
    fs::write(project.0.join("created.txt"), "new\n")?;

    let mut changed = checkpoints.restore(&turn_start)?;
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    let names: Vec<String> = changed
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    assert_eq!(names, ["created.txt", "kept.txt", "removed.txt"]);
    assert_eq!(fs::read_to_string(project.0.join("kept.txt"))?, "before\n");
    assert_eq!(fs::read_to_string(project.0.join("removed.txt"))?, "gone\n");
    assert!(!project.0.join("created.txt").exists());
    Ok(())
}

#[test]
fn diff_renders_a_unified_patch() -> TestResult {
    let pre = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
    let post = "one\ntwo\nCHANGED\nfour\nfive\nsix\nseven\n";
    let patch = yi_tools::patch(pre, post, std::path::Path::new("/tmp/sample.txt"));
    assert_eq!(
        patch.as_str(),
        "--- a//tmp/sample.txt\n\
         +++ b//tmp/sample.txt\n\
         @@ -1,6 +1,6 @@\n\
         \x20one\n\
         \x20two\n\
         -three\n\
         +CHANGED\n\
         \x20four\n\
         \x20five\n\
         \x20six\n"
    );
    Ok(())
}

#[test]
fn diff_of_identical_files_is_empty() -> TestResult {
    let patch = yi_tools::patch("same\n", "same\n", std::path::Path::new("/tmp/x"));
    assert!(patch.is_empty());
    Ok(())
}

#[test]
fn diff_applies_with_git_apply() -> TestResult {
    let dir = temp_dir("diff-apply")?;
    let root = fs::canonicalize(&dir.0)?;
    let file = root.join("sample.txt");
    let pre = "alpha\nbravo\ncharlie\ndelta\n";
    let post = "alpha\nBRAVO\ncharlie\ndelta\necho\n";
    fs::write(&file, pre)?;
    let patch = yi_tools::patch(pre, post, &file);
    let patch_file = root.join("change.patch");
    fs::write(&patch_file, patch.as_str())?;
    #[expect(
        clippy::disallowed_methods,
        reason = "git is the external ground truth for patch syntax"
    )]
    let applied = std::process::Command::new("git")
        .args(["apply", "--unsafe-paths", "--directory", "/"])
        .arg(&patch_file)
        .current_dir(&root)
        .output()?;
    assert!(
        applied.status.success(),
        "git apply rejected the patch: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(fs::read_to_string(&file)?, post);
    Ok(())
}

#[test]
fn checkpoint_diff_reports_what_changed_between_captures() -> TestResult {
    let project = temp_dir("checkpoint-diff")?;
    let shadow = temp_dir("checkpoint-diff-shadow")?;
    fs::write(project.0.join("notes.txt"), "alpha\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow.0, &project.0)?;
    let first = checkpoints.capture()?;
    fs::write(project.0.join("notes.txt"), "beta\n")?;
    let second = checkpoints.capture()?;

    let patch = checkpoints.diff(&first, &second)?;
    assert!(patch.as_str().contains("notes.txt"), "{}", patch.as_str());
    assert!(patch.as_str().contains("-alpha"), "{}", patch.as_str());
    assert!(patch.as_str().contains("+beta"), "{}", patch.as_str());
    assert!(checkpoints.diff(&first, &first)?.is_empty());
    Ok(())
}

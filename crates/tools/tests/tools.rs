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

    let missed = GrepTool::default().execute(args(&[("pattern", json!("needle"))]), &context);
    assert_eq!(output_text(&missed), "No matches found");

    let hit = GrepTool::default().execute(
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

/// Two handles on one project are two handles on one shadow index, and git
/// treats `index.lock` as a hard failure rather than a wait. The turn-end
/// capture is started after the session reports idle, so it is still holding
/// that index while `/undo` opens its own handle — undo answered
/// "git add failed: fatal: Unable to create ... index.lock" instead of its
/// outcome, and the drive gate read that as a frame that never arrived.
#[test]
fn checkpoints_on_one_project_serialize_across_handles() -> TestResult {
    let project = temp_dir("checkpoint-concurrent")?;
    let shadow = temp_dir("checkpoint-concurrent-shadow")?;
    // Enough of a tree that `git add --all` is wide enough to overlap; a
    // two-file project closes the window without proving anything.
    for index in 0..600 {
        fs::write(project.0.join(format!("f{index}.txt")), "x".repeat(2048))?;
    }
    let failures = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let (shadow, project, failures) =
            (shadow.0.clone(), project.0.clone(), Arc::clone(&failures));
        workers.push(std::thread::spawn(move || {
            for _ in 0..8 {
                let attempt = yi_tools::Checkpoints::open(&shadow, &project)
                    .and_then(|checkpoints| checkpoints.capture());
                if let Err(error) = attempt
                    && let Ok(mut failures) = failures.lock()
                {
                    failures.push(error.to_string());
                }
            }
        }));
    }
    for worker in workers {
        let _joined = worker.join();
    }
    let failures = failures.lock().map_err(|error| error.to_string())?;
    assert!(failures.is_empty(), "captures raced: {failures:?}");
    Ok(())
}

#[test]
fn bash_output_is_reduced_and_recoverable() -> TestResult {
    let dir = temp_dir("reduce-bash")?;
    let tool = BashTool;
    let mut context = ToolContext::new(dir.0.clone());
    context.recovery_dir = Some(dir.0.join("tool-output"));
    let output = tool.execute(
        args(&[(
            "command",
            json!("for i in $(seq 1 2000); do echo line-$i; done"),
        )]),
        &context,
    );
    let text: String = output
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(text.contains("lines omitted"), "{text}");
    assert!(text.contains("line-1\n"), "the head survives: {text}");
    assert!(text.contains("line-2000"), "the tail survives: {text}");

    let recovery = text
        .rsplit_once("[full output: ")
        .and_then(|(_, tail)| tail.split_once(']'))
        .map(|(path, _)| PathBuf::from(path))
        .ok_or("no recovery path")?;
    let full = fs::read_to_string(&recovery)?;
    assert!(full.contains("line-1000"), "the tee keeps what was dropped");
    assert!(full.len() > text.len());
    Ok(())
}

#[test]
fn a_lossy_reduction_without_a_tee_returns_raw() -> TestResult {
    let raw: String = (1..4_000).map(|n| format!("line-{n}\n")).collect();
    let reduced = yi_tools::reduce("ls -R", &raw, "", 0, None, None);
    assert_eq!(reduced.text, raw);
    assert!(reduced.recovery.is_none());
    Ok(())
}

#[test]
fn a_verbose_command_is_left_alone() -> TestResult {
    let raw: String = (1..4_000).map(|n| format!("line-{n}\n")).collect();
    let reduced = yi_tools::reduce("cargo test -- --nocapture", &raw, "", 0, None, None);
    assert_eq!(reduced.text, raw);
    assert!(reduced.recovery.is_none());
    Ok(())
}

#[test]
fn a_failing_cargo_run_keeps_its_diagnostics() -> TestResult {
    let mut raw = String::new();
    for n in 1..200 {
        raw.push_str(&format!("   Compiling crate-{n} v0.1.0\n"));
    }
    raw.push_str("error[E0425]: cannot find value `nope` in this scope\n");
    raw.push_str("  --> src/lib.rs:3:5\n");
    let dir = temp_dir("reduce-cargo")?;
    let reduced = yi_tools::reduce("cargo build", &raw, "", 101, Some(&dir.0), None);
    assert!(reduced.text.contains("E0425"), "{}", reduced.text);
    assert!(reduced.out_bytes < reduced.raw_bytes);
    Ok(())
}

#[test]
fn a_slow_command_backgrounds_and_can_be_polled() -> TestResult {
    let dir = temp_dir("background")?;
    let tool = BashTool;
    let mut context = ToolContext::new(dir.0.clone());
    context.auto_background = Some(std::time::Duration::from_millis(200));
    let started = tool.execute(args(&[("command", json!("sleep 1; echo woke"))]), &context);
    let announcement = text_of(&started.result.content);
    assert!(
        announcement.contains("Backgrounded as job"),
        "{announcement}"
    );
    let job = started
        .result
        .details
        .get("job")
        .and_then(Value::as_u64)
        .ok_or("no job id")?;

    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(5))]), &context);
    let finished = text_of(&polled.result.content);
    assert!(finished.contains("woke"), "{finished}");
    assert!(finished.contains("finished (exit 0)"), "{finished}");
    Ok(())
}

#[test]
fn without_auto_background_a_command_holds_the_turn() -> TestResult {
    let dir = temp_dir("no-background")?;
    let tool = BashTool;
    let context = ToolContext::new(dir.0.clone());
    let output = tool.execute(
        args(&[("command", json!("sleep 0.3; echo done"))]),
        &context,
    );
    assert!(text_of(&output.result.content).contains("done"));
    Ok(())
}

#[test]
fn an_inspecting_shell_command_is_not_flagged_irreversible() -> TestResult {
    let tool = BashTool;
    for command in [
        "ls -la && git log --oneline -5 2>/dev/null",
        "cat Cargo.toml | grep name",
        "RUST_LOG=debug rg needle src; wc -l src/lib.rs",
    ] {
        assert!(
            !tool.irreversible(&args(&[("command", json!(command))])),
            "reads nothing back: {command}"
        );
    }
    for command in [
        "rm -rf build",
        "ls -la && cargo build",
        "git status && git commit -m x",
        "echo hi > notes.txt",
        "cat $(rm -rf /tmp/x)",
    ] {
        assert!(
            tool.irreversible(&args(&[("command", json!(command))])),
            "changes something: {command}"
        );
    }
    Ok(())
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .map(|block| match block {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn grep_context_windows_merge_split_and_clip_at_file_edges() -> TestResult {
    let dir = temp_dir("grep-context")?;
    // Hits on the first and last lines force both clips, the pair at 4 and 5
    // makes two windows overlap, and the run of x/y/z leaves a real gap.
    fs::write(
        dir.0.join("c.txt"),
        "hit\nb\nc\nhit\nhit\nf\ng\nhit\nx\ny\nz\nhit",
    )?;
    let context = ToolContext::new(dir.0.clone());

    let out = GrepTool::default().execute(
        args(&[("pattern", json!("hit")), ("context", json!(1))]),
        &context,
    );
    let text = output_text(&out);
    // Lines 1-9 and 11-12 once each, plus one gap marker. A repeated line
    // would push this over 12.
    assert_eq!(text.lines().count(), 12, "{text}");
    assert_eq!(
        text.lines().filter(|line| *line == "--").count(),
        1,
        "{text}"
    );
    assert!(text.contains("c.txt:1:hit"), "{text}");
    assert!(text.contains("c.txt-2-b"), "{text}");
    assert!(text.contains("c.txt:4:hit"), "{text}");
    assert!(text.contains("c.txt:5:hit"), "{text}");
    assert!(text.contains("c.txt:12:hit"), "{text}");
    assert!(text.contains("c.txt-9-x"), "{text}");
    assert!(!text.contains("-10-"), "{text}");
    Ok(())
}

#[test]
fn grep_context_is_clamped_and_defaults_to_bare_hits() -> TestResult {
    let dir = temp_dir("grep-clamp")?;
    let body: String = (1..=200)
        .map(|n| {
            if n == 100 {
                "hit\n".to_owned()
            } else {
                format!("line {n}\n")
            }
        })
        .collect();
    fs::write(dir.0.join("big.txt"), body)?;
    let context = ToolContext::new(dir.0.clone());

    let bare = GrepTool::default().execute(args(&[("pattern", json!("hit"))]), &context);
    assert_eq!(output_text(&bare).lines().count(), 1);

    let clamped = GrepTool::default().execute(
        args(&[("pattern", json!("hit")), ("context", json!(9_999))]),
        &context,
    );
    assert_eq!(output_text(&clamped).lines().count(), 21);
    Ok(())
}

/// Without the patch on the result, the transcript and every ACP client fall
/// back to the one-line digest: an edit renders with no diff body at all.
#[test]
fn a_write_carries_its_patch_and_line_counts() -> TestResult {
    let dir = temp_dir("write-patch")?;
    let context = ToolContext::new(dir.0.clone());
    let tool = WriteTool::default();

    let created = tool.execute(
        args(&[("path", json!("a.txt")), ("content", json!("one\ntwo\n"))]),
        &context,
    );
    assert_eq!(created.result.details["added"], json!(2));
    assert_eq!(created.result.details["removed"], json!(0));

    let edited = tool.execute(
        args(&[("path", json!("a.txt")), ("content", json!("one\nTWO\n"))]),
        &context,
    );
    let patch = edited.result.details["patch"]
        .as_str()
        .ok_or("write result carried no patch")?;
    assert!(patch.contains("-two"), "{patch}");
    assert!(patch.contains("+TWO"), "{patch}");
    assert_eq!(edited.result.details["added"], json!(1));
    assert_eq!(edited.result.details["removed"], json!(1));
    Ok(())
}

/// `details` is stored in the session file and replayed from it, so the base
/// read is skipped past the cap rather than growing the record without bound.
/// No base means no patch — a whole-file addition would be a false report.
#[test]
fn a_write_over_a_file_past_the_cap_claims_no_patch() -> TestResult {
    let dir = temp_dir("write-cap")?;
    let context = ToolContext::new(dir.0.clone());
    let tool = WriteTool::default();
    let big = "x".repeat(yi_tools::DETAIL_CAP + 1);

    let first = tool.execute(
        args(&[("path", json!("big.txt")), ("content", json!(big))]),
        &context,
    );
    assert!(!first.is_error, "{}", output_text(&first));

    let second = tool.execute(
        args(&[("path", json!("big.txt")), ("content", json!("small\n"))]),
        &context,
    );
    assert!(!second.is_error, "{}", output_text(&second));
    assert!(
        second.result.details.get("patch").is_none(),
        "{:?}",
        second.result.details
    );
    Ok(())
}

struct OneCell(yi_types::kernel::ExecuteResult);

impl yi_tools::KernelBridge for OneCell {
    fn execute_cell(
        &self,
        _code: &str,
        _cancelled: &yi_tools::CancelFlag,
    ) -> Result<yi_tools::KernelCellOutcome, String> {
        Ok(yi_tools::KernelCellOutcome {
            result: self.0.clone(),
            kernel_restarted: false,
        })
    }
}

/// A kernel edit becomes a patch here, where every other patch is computed. The
/// point is the LCS: a whole-file dump would mark all four lines changed.
#[test]
fn a_kernel_edit_arrives_as_a_real_patch() -> TestResult {
    let dir = temp_dir("kernel-diff")?;
    let context = ToolContext::new(dir.0.clone());
    let tool = yi_tools::IpythonTool {
        bridge: Arc::new(OneCell(yi_types::kernel::ExecuteResult {
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            diffs: vec![yi_types::kernel::KernelDiffDisplay {
                path: "notes.txt".to_owned(),
                old_str: "one\ntwo\nthree\nfour\n".to_owned(),
                new_str: "one\ntwo\nTHREE\nfour\n".to_owned(),
                start_line: None,
            }],
            attachments: Vec::new(),
            sent_agent_messages: Vec::new(),
            status: yi_types::kernel::ExecuteStatus::Ok,
            error: None,
            duration_ms: 12,
        })),
    };

    let output = tool.execute(args(&[("code", json!("edit()"))]), &context);
    let patch = output.result.details["diffs"][0]["patch"]
        .as_str()
        .ok_or("kernel diff carried no patch")?;
    assert!(patch.contains("--- a/notes.txt"), "{patch}");
    assert!(
        patch.contains("-three") && patch.contains("+THREE"),
        "{patch}"
    );
    let removed = patch.lines().filter(|l| l.starts_with("-o")).count();
    assert_eq!(removed, 0, "unchanged lines are not marked: {patch}");
    Ok(())
}

/// Grep v2: regex opt-in, type filter, offset paging with an exact next
/// call, and a distinguishable no-more state.
#[test]
fn grep_v2_regex_type_filter_and_offset_paging() -> TestResult {
    let dir = temp_dir("grep-v2")?;
    fs::write(dir.0.join("a.rs"), "fn alpha() {}\nfn beta() {}\n")?;
    fs::write(dir.0.join("b.py"), "def alpha():\n    pass\n")?;
    let context = ToolContext::new(dir.0.clone());
    let grep = GrepTool::default();

    let literal = grep.execute(args(&[("pattern", json!("fn a"))]), &context);
    assert!(output_text(&literal).contains("a.rs:1:fn alpha"));

    let rx = grep.execute(
        args(&[
            ("pattern", json!("fn (alpha|beta)")),
            ("regex", json!(true)),
        ]),
        &context,
    );
    assert_eq!(output_text(&rx).lines().count(), 2, "{}", output_text(&rx));

    let bad = grep.execute(
        args(&[("pattern", json!("fn (")), ("regex", json!(true))]),
        &context,
    );
    assert!(bad.is_error);
    assert_eq!(
        bad.result.details.get("errorKind").and_then(Value::as_str),
        Some("invalid_args")
    );

    let typed = grep.execute(
        args(&[("pattern", json!("alpha")), ("type", json!("py"))]),
        &context,
    );
    let text = output_text(&typed);
    assert!(text.contains("b.py"), "{text}");
    assert!(!text.contains("a.rs"), "{text}");

    let paged = grep.execute(
        args(&[("pattern", json!("alpha")), ("offset", json!(50))]),
        &context,
    );
    assert!(
        output_text(&paged).contains("beyond the 2 collected matches"),
        "{}",
        output_text(&paged)
    );
    Ok(())
}

/// With a hashline store attached, grep mints a `[path#TAG]` header and
/// records the shown rows as seen, so a hit can anchor an edit directly.
#[test]
fn grep_mints_snapshot_tags_when_hashline_attached() -> TestResult {
    let dir = temp_dir("grep-tags")?;
    fs::write(dir.0.join("x.rs"), "one\ntwo\nthree\n")?;
    let context = ToolContext::new(dir.0.clone());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let grep = GrepTool {
        hashline: Some(std::sync::Arc::clone(&state)),
    };
    let out = grep.execute(
        args(&[("pattern", json!("two")), ("context", json!(1))]),
        &context,
    );
    let text = output_text(&out);
    assert!(text.contains("#"), "tag header missing: {text}");
    assert!(text.contains("2:two"), "{text}");
    let canonical = dir.0.join("x.rs").canonicalize()?.display().to_string();
    let guard = state.lock().map_err(|_| "poisoned")?;
    let snapshot = guard.snapshots.head(&canonical).ok_or("no snapshot")?;
    let seen = snapshot.seen_lines.as_ref().ok_or("no seen lines")?;
    assert!(seen.contains(&1) && seen.contains(&2) && seen.contains(&3));
    Ok(())
}

/// Read v2: multi-range windows in one call, elision markers between them,
/// and the long-line clip that never joins the seen set.
#[test]
fn read_ranges_and_line_clip() -> TestResult {
    let dir = temp_dir("read-v2")?;
    let body: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    fs::write(dir.0.join("r.txt"), &body)?;
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let read = yi_tools::hashline::tool::HashlineReadTool {
        state: std::sync::Arc::clone(&state),
    };
    let context = ToolContext::new(dir.0.clone());
    let out = read.execute(
        args(&[
            ("path", json!("r.txt")),
            ("ranges", json!([[2, 4], [50, 52]])),
        ]),
        &context,
    );
    let text = output_text(&out);
    assert!(
        text.contains("2:line 2") && text.contains("52:line 52"),
        "{text}"
    );
    assert!(text.contains("[lines 5-49 not shown]"), "{text}");
    assert!(!text.contains("line 30"), "{text}");

    let both = read.execute(
        args(&[
            ("path", json!("r.txt")),
            ("ranges", json!([[1, 2]])),
            ("offset", json!(1)),
        ]),
        &context,
    );
    assert!(both.is_error);

    let long = format!("short\n{}\n", "x".repeat(5_000));
    fs::write(dir.0.join("wide.txt"), &long)?;
    let out = read.execute(args(&[("path", json!("wide.txt"))]), &context);
    let text = output_text(&out);
    assert!(text.contains("were clipped"), "{text}");
    assert!(text.contains("sed -n '2p'"), "{text}");
    let canonical = dir.0.join("wide.txt").canonicalize()?.display().to_string();
    let guard = state.lock().map_err(|_| "poisoned")?;
    let snapshot = guard.snapshots.head(&canonical).ok_or("no snapshot")?;
    let seen = snapshot.seen_lines.as_ref().ok_or("no seen lines")?;
    assert!(seen.contains(&1) && !seen.contains(&2), "clipped row seen");
    Ok(())
}

/// P1/P2: a multi-window read that stops short names its next offset, and the
/// byte budget stops on bytes rather than lines, naming the line to resume at.
#[test]
fn read_footers_name_the_next_offset() -> TestResult {
    let dir = temp_dir("read-footers")?;
    let body: String = (1..=200).map(|n| format!("line {n}\n")).collect();
    fs::write(dir.0.join("r.txt"), &body)?;
    let read = yi_tools::hashline::tool::HashlineReadTool {
        state: yi_tools::hashline::tool::shared_hashline_state(),
    };
    let context = ToolContext::new(dir.0.clone());
    let windows = read.execute(
        args(&[
            ("path", json!("r.txt")),
            ("ranges", json!([[2, 4], [50, 52]])),
        ]),
        &context,
    );
    let text = output_text(&windows);
    assert!(text.contains("continue with offset=53"), "{text}");

    let wide: String = (1..=400)
        .map(|n| format!("{n} {}\n", "x".repeat(400)))
        .collect();
    fs::write(dir.0.join("wide.txt"), &wide)?;
    let capped = read.execute(args(&[("path", json!("wide.txt"))]), &context);
    let text = output_text(&capped);
    assert!(text.contains("byte budget"), "{text}");
    let stopped: u64 = text
        .rsplit_once("continue with offset=")
        .and_then(|(_, tail)| tail.trim_end_matches(']').parse().ok())
        .ok_or("no resume offset")?;
    assert!(stopped > 1 && stopped < 400, "stopped mid-file: {text}");
    assert!(
        capped.result.details.get("byteCapped") == Some(&json!(true)),
        "{:?}",
        capped.result.details
    );

    // An explicit limit scales the budget, so the same file gets further in.
    let bigger = read.execute(
        args(&[("path", json!("wide.txt")), ("limit", json!(400))]),
        &context,
    );
    let text = output_text(&bigger);
    let further: u64 = text
        .rsplit_once("continue with offset=")
        .and_then(|(_, tail)| tail.trim_end_matches(']').parse().ok())
        .unwrap_or(u64::MAX);
    assert!(further > stopped, "explicit limit read further: {text}");
    Ok(())
}

/// P3: hits that exactly fill the collection cap are not a truncation, and an
/// offset past the end names what it ran off — matches, or files in
/// `files_with_matches` mode.
#[test]
fn grep_cap_and_offset_footers_are_honest() -> TestResult {
    let dir = temp_dir("grep-caps")?;
    let body: String = (1..=2_000).map(|n| format!("needle {n}\n")).collect();
    fs::write(dir.0.join("full.txt"), &body)?;
    let grep = yi_tools::GrepTool::default();
    let context = ToolContext::new(dir.0.clone());
    let exact = grep.execute(args(&[("pattern", json!("needle"))]), &context);
    let text = output_text(&exact);
    assert!(!text.contains("match collection stopped"), "{text}");
    assert!(!text.contains("at least"), "{text}");
    assert_eq!(exact.result.details.get("hits"), Some(&json!(2_000)));

    let past = grep.execute(
        args(&[("pattern", json!("needle")), ("offset", json!(5_000))]),
        &context,
    );
    let text = output_text(&past);
    assert!(text.contains("collected matches"), "{text}");

    let past_files = grep.execute(
        args(&[
            ("pattern", json!("needle")),
            ("offset", json!(9)),
            ("files_with_matches", json!(true)),
        ]),
        &context,
    );
    let text = output_text(&past_files);
    assert!(text.contains("beyond the 1 matching files"), "{text}");
    Ok(())
}

/// Bash results carry a command category for `yi stats`.
#[test]
fn bash_details_carry_a_command_category() -> TestResult {
    let dir = temp_dir("bash-cat")?;
    let context = ToolContext::new(dir.0.clone());
    let out = BashTool.execute(args(&[("command", json!("ls"))]), &context);
    assert_eq!(
        out.result.details.get("category").and_then(Value::as_str),
        Some("list_files")
    );
    Ok(())
}

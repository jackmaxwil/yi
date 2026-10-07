use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{BashTool, GrepTool, Tool, ToolContext, ToolKind, WriteTool, discover_exec_tools};
use yi_types::message::Content;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> std::io::Result<Scratch> {
    Scratch::new(&format!("yi-tools-{tag}"))
}

fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn read_tool() -> yi_tools::hashline::tool::HashlineReadTool {
    yi_tools::hashline::tool::HashlineReadTool::new(
        yi_tools::hashline::tool::shared_hashline_state(),
    )
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
    let context = ToolContext::new(dir.to_path_buf());

    let written = WriteTool::default().execute(
        args(&[
            ("path", json!("notes/hello.txt")),
            ("content", json!("line one\nline two\nline three")),
        ]),
        &context,
    );
    assert!(!written.is_error, "{}", output_text(&written));

    let read = yi_tools::hashline::tool::HashlineReadTool::new(
        yi_tools::hashline::tool::shared_hashline_state(),
    )
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
    let context = ToolContext::new(dir.to_path_buf());
    let read = yi_tools::hashline::tool::HashlineReadTool::new(
        yi_tools::hashline::tool::shared_hashline_state(),
    )
    .execute(args(&[("path", json!("absent.txt"))]), &context);
    assert!(read.is_error);
    Ok(())
}

/// Incident: grep prints hits under `[pkg/more.py#TAG]` and a reader passed that `path#TAG` to
/// read, which answered "No such file or directory" (#993).
#[test]
fn read_takes_the_path_hash_tag_form_that_grep_prints() -> TestResult {
    let dir = temp_dir("read-tagged")?;
    fs::create_dir_all(dir.join("pkg"))?;
    let body: String = (1..=80).map(|n| format!("café line {n}\n")).collect();
    fs::write(dir.join("pkg/more.py"), &body)?;
    let context = ToolContext::new(dir.to_path_buf());
    let read = |path: &str| {
        read_tool().execute(
            args(&[
                ("path", json!(path)),
                ("offset", json!(60)),
                ("limit", json!(5)),
            ]),
            &context,
        )
    };
    let first = output_text(&read("pkg/more.py"));
    let tag = first
        .split("pkg/more.py#")
        .nth(1)
        .and_then(|rest| rest.get(..4))
        .ok_or("the read prints no tag")?
        .to_owned();

    let current = read(&format!("pkg/more.py#{tag}"));
    assert!(!current.is_error, "{}", output_text(&current));
    let current = output_text(&current);
    assert!(current.contains("café line 60"), "{current}");
    assert!(!current.contains("not the current"), "{current}");

    fs::write(
        dir.join("pkg/more.py"),
        body.replace("line 60", "line sixty"),
    )?;
    let stale = read(&format!("pkg/more.py#{tag}"));
    assert!(!stale.is_error, "{}", output_text(&stale));
    let stale = output_text(&stale);
    assert!(
        stale.contains(&format!("pkg/more.py#{tag} is not the current version")),
        "{stale}"
    );
    assert!(stale.contains("line sixty"), "{stale}");

    fs::write(dir.join("odd#ABCD"), "named with a hash\n".repeat(80))?;
    let named = output_text(&read("odd#ABCD"));
    assert!(named.contains("named with a hash"), "{named}");

    assert!(read("absent.py#ABCD").is_error);
    Ok(())
}

/// Incident: a read of `~/.ssh/id_rsa` failed on `<cwd>/~/.ssh/id_rsa`, so the permission
/// check judged a path in the workspace while the model meant HOME's.
#[test]
fn a_tilde_path_reads_under_home() -> TestResult {
    let dir = temp_dir("read-tilde")?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?);
    let name = format!("yi-tools-absent-{}", std::process::id());
    let read = read_tool().execute(
        args(&[("path", json!(format!("~/{name}")))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text = output_text(&read);
    assert!(read.is_error, "{text}");
    assert!(
        text.contains(&home.join(&name).display().to_string()),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_glob_read_matches_relative_patterns_and_skips_git() -> TestResult {
    let dir = temp_dir("glob")?;
    fs::create_dir_all(dir.join("src"))?;
    fs::create_dir_all(dir.join(".git"))?;
    fs::write(dir.join("src/a.rs"), "")?;
    fs::write(dir.join("src/b.txt"), "")?;
    fs::write(dir.join(".git/c.rs"), "")?;
    let context = ToolContext::new(dir.to_path_buf());

    let found = read_tool().execute(args(&[("path", json!("**/*.rs"))]), &context);
    let text = output_text(&found);
    assert!(text.contains("[src/a.rs#"), "{text}");
    assert!(!text.contains("b.txt"));
    assert!(!text.contains(".git"));
    Ok(())
}

#[test]
fn grep_returns_path_line_hits_and_respects_case_flag() -> TestResult {
    let dir = temp_dir("grep")?;
    fs::write(dir.join("one.txt"), "alpha\nNEEDLE here\nomega")?;
    let context = ToolContext::new(dir.to_path_buf());

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

/// Script classes are how a search finds CJK or Greek text; the dist build links only some Unicode
/// tables (vendor/syntect), so a property it lacks has to say so rather than read as a typo.
#[test]
fn grep_unicode_classes_match_on_the_linked_tables() -> TestResult {
    let dir = temp_dir("grep-unicode")?;
    fs::write(
        dir.join("i18n.txt"),
        "plain ascii\n\u{4f60}\u{597d}\n\u{3b1}\u{3b2}\u{3b3}\n",
    )?;
    let context = ToolContext::new(dir.to_path_buf());
    let grep = GrepTool::default();
    for (pattern, line) in [
        (r"\p{Han}+", "i18n.txt:2:"),
        (r"\p{Greek}+", "i18n.txt:3:"),
        (r"^\p{Script=Latin}+ ", "i18n.txt:1:"),
        (r"\p{L}\p{L}$", "i18n.txt:2:"),
    ] {
        let hit = grep.execute(args(&[("pattern", json!(pattern))]), &context);
        let text = output_text(&hit);
        assert!(text.contains(line), "{pattern}: {text}");
    }
    let unknown = grep.execute(args(&[("pattern", json!(r"\p{Klingon}"))]), &context);
    let text = output_text(&unknown);
    assert!(unknown.is_error, "{text}");
    assert!(text.contains("Age and the grapheme"), "{text}");
    Ok(())
}

#[test]
fn bash_reports_output_exit_code_and_stderr() -> TestResult {
    let dir = temp_dir("bash")?;
    let context = ToolContext::new(dir.to_path_buf());

    let ok = BashTool::default().execute(args(&[("command", json!("echo hello"))]), &context);
    assert!(!ok.is_error);
    assert_eq!(output_text(&ok).trim(), "hello");

    let failed = BashTool::default().execute(
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
    let mut context = ToolContext::new(dir.to_path_buf());
    context.cancelled = Arc::new(|| true);

    let start = std::time::Instant::now();
    let aborted = BashTool::default().execute(args(&[("command", json!("sleep 30"))]), &context);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(aborted.is_error);
    assert!(output_text(&aborted).contains("[command aborted]"));
    Ok(())
}

#[test]
fn a_second_time_limit_in_four_bash_calls_asks_for_a_new_method() -> TestResult {
    let dir = temp_dir("bash-ceiling-nudge")?;
    let tool = BashTool::default();
    let context = ToolContext::new(dir.to_path_buf());
    let run = |command: &str, limit: u64| {
        let output = tool.execute(
            args(&[("command", json!(command)), ("timeout_secs", json!(limit))]),
            &context,
        );
        output_text(&output)
    };
    // ctr-optimization's shape: a timed-out wait, a call that finishes, the wait again.
    let first = run("sleep 5", 1);
    assert!(!first.contains("hit their time limit"), "{first}");
    assert_eq!(run("true", 1), "(no output)");
    let own = run("timeout 1 sleep 5", 30);
    assert!(
        own.contains("[2 of the last 4 bash calls hit their time limit;"),
        "an exit 124 from the command's own timeout counts: {own}"
    );
    let third = run("sleep 5", 1);
    assert!(
        third.contains("[timed out after 1s]\n") && third.contains("[3 of the last 4"),
        "{third}"
    );
    assert!(
        !third.contains("timeout_secs"),
        "the nudge never says to raise the limit: {third}"
    );
    Ok(())
}

#[test]
fn a_capped_timeout_secs_opens_every_result_of_the_call() -> TestResult {
    let dir = temp_dir("bash-timeout-cap")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    let capped = "[timeout_secs 900 capped at 600]\n";
    // telecom-entity-resolution's asks in the 2026-09-11 v4 sweep: these three returned within
    // 66 s, and the third's arguments sent again ran to the 600 s kill, on the same result path.
    for command in [
        "cd /app && timeout 900 python solve.py",
        "cd /app && timeout 900 python3 solve.py",
        "cd /app && timeout 900 ~/.yi/kernel-venv-8286f6bd/bin/python solve.py",
    ] {
        let output = BashTool::default().execute(
            args(&[("command", json!(command)), ("timeout_secs", json!(900))]),
            &context,
        );
        let text = output_text(&output);
        assert!(text.starts_with(capped), "{text}");
    }
    // The long run under a TUI's auto_background, with `sleep 2` standing in for its 605 s.
    context.auto_background = Some(std::time::Duration::from_millis(200));
    let output = BashTool::default().execute(
        args(&[("command", json!("sleep 2")), ("timeout_secs", json!(900))]),
        &context,
    );
    let text = output_text(&output);
    assert!(text.contains("still running after"), "{text}");
    assert!(text.starts_with(capped), "{text}");
    context.auto_background = None;
    let command = "cd /app && timeout 580 ~/.yi/kernel-venv-8286f6bd/bin/python solve.py";
    let output = BashTool::default().execute(
        args(&[("command", json!(command)), ("timeout_secs", json!(600))]),
        &context,
    );
    let text = output_text(&output);
    assert!(
        !text.contains("capped at"),
        "an ask at the ceiling says nothing: {text}"
    );
    Ok(())
}

#[test]
fn bash_times_out_kills_the_command_and_says_how_to_raise_the_limit() -> TestResult {
    let dir = temp_dir("bash-timeout")?;
    let context = ToolContext::new(dir.to_path_buf());
    let start = std::time::Instant::now();
    let output = BashTool::default().execute(
        args(&[("command", json!("sleep 5")), ("timeout_secs", json!(1))]),
        &context,
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert!(output.is_error);
    let text = output_text(&output);
    assert!(
        text.contains(
            "[timed out after 1s; pass timeout_secs up to 600 for a longer run, or narrow the command]"
        ),
        "{text}"
    );
    assert!(!text.contains("[command aborted]"));
    assert!(!text.contains("[group kill failed"), "{text}");
    assert_eq!(output.result.details["timedOut"], json!(true));
    Ok(())
}

#[test]
fn a_child_that_leaves_the_group_cannot_hold_the_timeout_open() -> TestResult {
    let dir = temp_dir("bash-escapee")?;
    let context = ToolContext::new(dir.to_path_buf());
    let start = std::time::Instant::now();
    let output = BashTool::default().execute(
        args(&[
            ("command", json!("python3 -c \"import subprocess,os; subprocess.Popen(['sleep','30'], start_new_session=True); os._exit(0)\"; sleep 30")),
            ("timeout_secs", json!(1)),
        ]),
        &context,
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(12),
        "{:?}",
        start.elapsed()
    );
    assert!(output.is_error);
    assert!(
        output_text(&output).contains("[timed out after 1s"),
        "{}",
        output_text(&output)
    );
    Ok(())
}

#[test]
fn a_shorter_auto_background_wins_over_the_timeout() -> TestResult {
    let dir = temp_dir("bash-auto-bg")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    context.auto_background = Some(std::time::Duration::from_millis(200));
    let output = BashTool::default().execute(
        args(&[("command", json!("sleep 2")), ("timeout_secs", json!(1))]),
        &context,
    );
    assert!(!output.is_error);
    assert!(output_text(&output).contains("still running after"));
    Ok(())
}

#[test]
fn bash_refuses_a_root_walk_before_spawning() -> TestResult {
    let dir = temp_dir("bash-root-walk")?;
    let context = ToolContext::new(dir.to_path_buf());
    let start = std::time::Instant::now();
    let output = BashTool::default().execute(
        args(&[("command", json!("find / -name slug.py"))]),
        &context,
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert!(output.is_error);
    assert_eq!(
        output_text(&output),
        "[refused: `find /` walks the whole filesystem; search from the cwd, add -maxdepth N, or name the directory you expect]"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn discovers_and_runs_an_exec_tool_via_the_schema_contract() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("exec")?;
    let script = dir.join("greet");
    fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--schema\" ]; then\n  printf '{\"name\":\"greet\",\"description\":\"greets\",\"input_schema\":{\"type\":\"object\"},\"kind\":\"read\"}'\n  exit 0\nfi\nread line\nprintf 'greeting for %s' \"$line\"\n",
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    fs::write(dir.join("not-executable.txt"), "ignored")?;

    let tools = discover_exec_tools(&dir);
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.name(), "greet");

    let context = ToolContext::new(dir.to_path_buf());
    let output = tool.execute(args(&[("who", json!("yi"))]), &context);
    assert!(!output.is_error, "{}", output_text(&output));
    assert!(output_text(&output).contains("greeting for"));
    Ok(())
}

#[test]
fn glob_skips_gitignored_paths() -> TestResult {
    let dir = temp_dir("gitignore")?;
    let root = &dir;
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

    let context = ToolContext::new(root.to_path_buf());
    let output = read_tool().execute(args(&[("path", json!("**/*.rs"))]), &context);
    let text = output_text(&output);
    assert!(text.contains("main.rs"));
    assert!(!text.contains("target"));
    Ok(())
}

#[test]
fn checkpoint_restore_reverts_a_turn() -> TestResult {
    let project = temp_dir("checkpoint-project")?;
    let shadow = temp_dir("checkpoint-shadow")?;
    fs::write(project.join("kept.txt"), "before\n")?;
    fs::write(project.join("removed.txt"), "gone\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow, &project)?;
    let turn_start = checkpoints.capture()?;

    fs::write(project.join("kept.txt"), "after\n")?;
    fs::remove_file(project.join("removed.txt"))?;
    fs::write(project.join("created.txt"), "new\n")?;

    let mut changed = checkpoints.restore(&turn_start, None)?;
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    let names: Vec<String> = changed
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    assert_eq!(names, ["created.txt", "kept.txt", "removed.txt"]);
    assert_eq!(fs::read_to_string(project.join("kept.txt"))?, "before\n");
    assert_eq!(fs::read_to_string(project.join("removed.txt"))?, "gone\n");
    assert!(!project.join("created.txt").exists());
    Ok(())
}

#[test]
fn checkpoint_restore_keeps_what_was_edited_after_the_turn() -> TestResult {
    let project = temp_dir("checkpoint-kept-project")?;
    let shadow = temp_dir("checkpoint-kept-shadow")?;
    fs::write(project.join("a.txt"), "before\n")?;
    fs::write(project.join("b.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow, &project)?;
    let turn_start = checkpoints.capture()?;
    fs::write(project.join("a.txt"), "turn\n")?;
    fs::write(project.join("n.txt"), "turn\n")?;
    let turn_end = checkpoints.capture()?;
    fs::write(project.join("a.txt"), "hand\n")?;
    fs::write(project.join("b.txt"), "hand\n")?;
    fs::write(project.join("c.txt"), "hand\n")?;

    let mut changed = checkpoints.restore(&turn_start, Some(&turn_end))?;
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    let listed: Vec<(String, yi_tools::ChangeKind)> = changed
        .iter()
        .map(|change| (change.path.display().to_string(), change.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("a.txt".to_owned(), yi_tools::ChangeKind::Kept),
            ("n.txt".to_owned(), yi_tools::ChangeKind::Deleted),
        ]
    );
    assert_eq!(fs::read_to_string(project.join("a.txt"))?, "hand\n");
    assert_eq!(fs::read_to_string(project.join("b.txt"))?, "hand\n");
    assert_eq!(fs::read_to_string(project.join("c.txt"))?, "hand\n");
    assert!(!project.join("n.txt").exists());
    Ok(())
}

/// Dies with the shadow snapshotting itself when `yi` starts in `~`: each capture stages the
/// last one's objects, so the tree moves with nothing edited and every capture runs slower.
#[test]
fn a_capture_leaves_out_a_shadow_that_lives_inside_the_project() -> TestResult {
    let project = temp_dir("checkpoint-home-project")?;
    fs::write(project.join("notes.txt"), "alpha\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&project.join(".yi/checkpoints"), &project)?;
    let first = checkpoints.capture()?;
    assert_eq!(
        checkpoints.capture()?,
        first,
        "nothing edited, yet the tree moved"
    );
    assert_eq!(checkpoints.show(&first, "notes.txt")?, "alpha\n");
    Ok(())
}

#[test]
fn checkpoint_restore_undoes_a_rename() -> TestResult {
    let project = temp_dir("checkpoint-rename-project")?;
    let shadow = temp_dir("checkpoint-rename-shadow")?;
    fs::write(project.join("a.txt"), "same content either name\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow, &project)?;
    let turn_start = checkpoints.capture()?;
    fs::rename(project.join("a.txt"), project.join("b.txt"))?;
    let turn_end = checkpoints.capture()?;

    let changed = checkpoints.restore(&turn_start, Some(&turn_end))?;
    assert_eq!(changed.len(), 2, "{changed:?}");
    assert_eq!(
        fs::read_to_string(project.join("a.txt"))?,
        "same content either name\n"
    );
    assert!(!project.join("b.txt").exists());
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
    let root = fs::canonicalize(&dir)?;
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

// Dies with the private index in `materialize`: share the shadow index and a materialized
// verification tree becomes the next capture, or the checkout gets rewritten.
#[test]
fn checkpoint_materialize_writes_the_captured_tree_elsewhere() -> TestResult {
    let project = temp_dir("checkpoint-materialize-project")?;
    let shadow = temp_dir("checkpoint-materialize-shadow")?;
    let into = temp_dir("checkpoint-materialize-into")?;
    fs::write(project.join("kept.txt"), "before\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow, &project)?;
    let captured = checkpoints.capture()?;
    fs::write(project.join("kept.txt"), "after\n")?;
    checkpoints.materialize(&captured, &into.join("tree"))?;
    assert_eq!(fs::read_to_string(into.join("tree/kept.txt"))?, "before\n");
    assert_eq!(fs::read_to_string(project.join("kept.txt"))?, "after\n");
    assert!(!into.join("tree.index").exists());
    let again = checkpoints.capture()?;
    assert_ne!(again, captured, "the shadow index still tracks the project");
    Ok(())
}

#[test]
fn checkpoint_diff_reports_what_changed_between_captures() -> TestResult {
    let project = temp_dir("checkpoint-diff")?;
    let shadow = temp_dir("checkpoint-diff-shadow")?;
    fs::write(project.join("notes.txt"), "alpha\n")?;
    let checkpoints = yi_tools::Checkpoints::open(&shadow, &project)?;
    let first = checkpoints.capture()?;
    fs::write(project.join("notes.txt"), "beta\n")?;
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
        fs::write(project.join(format!("f{index}.txt")), "x".repeat(2048))?;
    }
    let failures = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let (shadow, project, failures) = (
            shadow.to_path_buf(),
            project.to_path_buf(),
            Arc::clone(&failures),
        );
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
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
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

/// The reducer once cut at 2 KiB and pointed at a spill file no rollout read (77 pointers,
/// 0 reads across rows 0018-0023); under the floor the whole output rides the result.
#[test]
fn a_result_under_eight_kib_is_never_reduced() -> TestResult {
    let dir = temp_dir("reduce-floor")?;
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let output = tool.execute(
        args(&[(
            "command",
            json!("for i in $(seq 1 400); do echo row-$i; done"),
        )]),
        &context,
    );
    let text = output_text(&output);
    assert!(
        text.contains("row-1\n") && text.contains("row-400"),
        "{text}"
    );
    assert!(
        !text.contains("lines omitted") && !text.contains("[full output:"),
        "{text}"
    );
    Ok(())
}

/// Duplicates and progress frames compress before the middle is cut, and the cut is a byte
/// budget: the tail rows a 700-line dump once hid are in the result.
#[test]
fn progress_and_duplicate_lines_compress_before_the_middle_is_cut() -> TestResult {
    let dir = temp_dir("reduce-compress")?;
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let command = "for i in $(seq 1 200); do echo 'duplicate row'; done; printf 'progress 1%%\\rprogress 50%%\\rprogress 100%%\\n'; for i in $(seq 1 1500); do echo row-$i; done";
    let output = tool.execute(args(&[("command", json!(command))]), &context);
    let text = output_text(&output);
    assert!(text.contains("duplicate row [×200]"), "{text}");
    assert!(
        text.contains("progress 100%") && !text.contains("progress 1%"),
        "{text}"
    );
    assert!(text.contains("row-1500"), "the tail survives: {text}");
    let shown = text.lines().filter(|line| line.starts_with("row-")).count();
    assert!(
        shown > 500,
        "a byte budget shows more than 120 lines: {shown}"
    );
    assert!(
        text.contains("lines omitted:") && text.contains("[full output:"),
        "{text}"
    );
    Ok(())
}

/// Incident: the capture kept a command's first 30 000 bytes, so a long build or test run lost
/// its last lines, where the verdict is.
#[test]
fn a_capped_bash_stream_keeps_the_tail_with_the_verdict() -> TestResult {
    let dir = temp_dir("bash-capped")?;
    let context = ToolContext::new(dir.to_path_buf());
    let verdict = "test result: FAILED. 3 passed; 1 failed";
    let command = format!("for i in $(seq 1 5000); do echo row-$i; done; echo '{verdict}'");
    let output = BashTool::default().execute(args(&[("command", json!(command))]), &context);
    let text = output_text(&output);
    let last: Vec<&str> = text.lines().rev().take(3).collect();
    assert!(text.contains(verdict), "the tail is gone: {last:?}");
    let streamed: usize = (1..=5000)
        .map(|n| format!("row-{n}\n").len())
        .sum::<usize>()
        + verdict.len()
        + 1;
    let marker = format!(
        "[{} bytes omitted from the middle]",
        streamed - yi_tools::OUTPUT_CAP
    );
    assert!(text.contains(&marker), "no {marker}: {last:?}");
    assert!(text.starts_with("row-1\nrow-2\n"), "the head is gone");
    assert_eq!(output.result.details["truncated"], json!(true));
    Ok(())
}

/// Dogfood 2026-09-27: a cut stream named no file, and the reducer's tee kept the cut text under
/// a `[full output: P]` label. Held in memory, through a file, left unreduced (`--verbose`) or
/// split over both streams (interleaved as they arrived), every line survives.
#[test]
fn a_cut_bash_stream_names_a_file_with_every_byte() -> TestResult {
    let dir = temp_dir("bash-spill")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let cases = [
        ("seq 1 8000", 8_000),
        ("seq 1 20000", 20_000),
        ("seq 1 8000 # --verbose", 8_000),
        ("seq 1 8000; seq 8001 16000 >&2", 16_000),
    ];
    for (command, count) in cases {
        let output = BashTool::default().execute(args(&[("command", json!(command))]), &context);
        let text = output_text(&output);
        let path = (text.lines().last())
            .and_then(|line| line.strip_prefix("[full output: ")?.strip_suffix(']'))
            .ok_or_else(|| format!("{command}: the pointer is not last: {text}"))?;
        let bytes: usize = (1..=count).map(|n| format!("{n}\n").len()).sum();
        let whole = fs::read_to_string(path)?;
        assert_eq!(whole.len(), bytes, "{command}");
        let mut rows: Vec<u32> = whole.lines().flat_map(str::parse).collect();
        rows.sort_unstable();
        assert!(
            rows.iter().copied().eq(1..=count),
            "{command}: {} rows",
            rows.len()
        );
        let read = read_tool().execute(args(&[("path", json!(path))]), &context);
        assert!(
            !read.is_error && output_text(&read).contains(":100\n"),
            "{command}"
        );
    }
    // Kept only where a pointer names it: a 13,893-byte output left whole leaves no file.
    let whole =
        BashTool::default().execute(args(&[("command", json!("seq 1 3000 # -v"))]), &context);
    assert!(!output_text(&whole).contains("[full output:"));
    // `seq 1 8000` twice is one file: equal bytes share their name.
    assert_eq!(
        fs::read_dir(dir.join("tool-output"))?.count(),
        cases.len() - 1
    );
    Ok(())
}

/// A job past its wait reports its cut the same way, once it settles.
#[test]
fn a_backgrounded_jobs_report_names_its_spill() -> TestResult {
    let dir = temp_dir("jobs-spill")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let after = Some(std::time::Duration::from_millis(100));
    let limit = std::time::Duration::from_secs(60);
    let run =
        yi_tools::run_or_background("sleep 1; seq 1 20000", &context, after, limit, None, None)?;
    let yi_tools::Run::Backgrounded(id) = run else {
        return Err("the job finished before its wait".into());
    };
    yi_tools::jobs::registry().wait_settled(Some(id), limit);
    let output = yi_tools::jobs::registry()
        .report(id)
        .ok_or("no report")?
        .output;
    let path = (output.lines().last())
        .and_then(|line| line.strip_prefix("[full output: ")?.strip_suffix(']'))
        .ok_or("the report names no spill")?;
    assert_eq!(fs::read_to_string(path)?.lines().count(), 20_000);
    Ok(())
}

#[test]
fn a_lossy_reduction_without_a_tee_returns_raw() -> TestResult {
    let raw: String = (1..4_000).map(|n| format!("line-{n}\n")).collect();
    let reduced = yi_tools::reduce("ls -R", &raw, "", 0, None, None, None);
    assert_eq!(reduced.text, raw);
    assert!(reduced.recovery.is_none());
    Ok(())
}

#[test]
fn a_verbose_command_is_left_alone() -> TestResult {
    let raw: String = (1..4_000).map(|n| format!("line-{n}\n")).collect();
    let reduced = yi_tools::reduce("cargo test -- --nocapture", &raw, "", 0, None, None, None);
    assert_eq!(reduced.text, raw);
    assert!(reduced.recovery.is_none());
    Ok(())
}

#[test]
fn a_failing_cargo_run_keeps_its_diagnostics() -> TestResult {
    let mut raw = String::new();
    // Past the 8 KiB floor: 200 lines rode whole once the floor rose from 2 KiB.
    for n in 1..400 {
        raw.push_str(&format!("   Compiling crate-{n} v0.1.0\n"));
    }
    raw.push_str("error[E0425]: cannot find value `nope` in this scope\n");
    raw.push_str("  --> src/lib.rs:3:5\n");
    let dir = temp_dir("reduce-cargo")?;
    let reduced = yi_tools::reduce("cargo build", &raw, "", 101, None, Some(&dir), None);
    assert!(reduced.text.contains("E0425"), "{}", reduced.text);
    assert!(reduced.out_bytes < reduced.raw_bytes);
    Ok(())
}

fn reduce_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/reduce/{name}"))
}

/// Incident: a two-crate build printed its one error at byte 23920 of 48279, the middle that
/// the capture cut and the red path's line cap both dropped, under either exit status.
#[test]
fn a_red_workspace_build_keeps_its_error_through_the_reducer() -> TestResult {
    let dir = temp_dir("reduce-red-workspace")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let build = reduce_fixture("red-workspace-build.txt");
    let streamed = usize::try_from(fs::metadata(&build)?.len())?;
    let cases = [
        (format!("cat '{}'; exit 101", build.display()), 0),
        (
            format!("cat '{}'; echo EXIT=$?", build.display()),
            "EXIT=0\n".len(),
        ),
    ];
    for (command, extra) in cases {
        let output = BashTool::default().execute(args(&[("command", json!(command))]), &context);
        let text = output_text(&output);
        assert!(text.contains("error[E0308]"), "{command}: {text}");
        assert!(text.contains("b/src/main.rs:121:36"), "{command}: {text}");
        let cap = yi_tools::OUTPUT_CAP;
        let row = format!(
            "[capture cut: kept {cap} of {} bytes (OUTPUT_CAP {cap} per stream); every byte: ",
            streamed + extra
        );
        assert!(
            text.lines().any(|line| line.starts_with(&row)),
            "{command}: no {row}: {text}"
        );
    }
    Ok(())
}

#[test]
fn the_omitted_lines_notice_names_lines_of_the_file_it_points_at() -> TestResult {
    let dir = temp_dir("reduce-notice-lines")?;
    let mut context = ToolContext::new(dir.to_path_buf());
    context.recovery_dir = Some(dir.join("tool-output"));
    let output = BashTool::default().execute(args(&[("command", json!("seq 1 20000"))]), &context);
    let text = output_text(&output);
    let range = (text.lines())
        .find_map(|line| line.split_once(" lines omitted: ")?.1.strip_suffix(']'))
        .and_then(|range| range.split_once('-'))
        .ok_or_else(|| format!("no notice: {text}"))?;
    let (from, to): (usize, usize) = (range.0.parse()?, range.1.parse()?);
    let path = (text.lines())
        .find_map(|line| line.strip_prefix("[full output: ")?.strip_suffix(']'))
        .ok_or_else(|| format!("no pointer: {text}"))?;
    let whole = fs::read_to_string(path)?;
    let spill: Vec<&str> = whole.lines().collect();
    let shown: std::collections::HashSet<&str> = text.lines().collect();
    let at = |n: usize| {
        spill
            .get(n.wrapping_sub(1))
            .copied()
            .unwrap_or("<past the file>")
    };
    assert!(
        shown.contains(at(from - 1)),
        "line {} is not shown",
        from - 1
    );
    assert!(shown.contains(at(to + 1)), "line {} is not shown", to + 1);
    let named_but_shown: Vec<usize> = (from..=to).filter(|n| shown.contains(at(*n))).collect();
    assert!(named_but_shown.is_empty(), "shown: {named_but_shown:?}");
    Ok(())
}

#[test]
fn a_slow_command_backgrounds_and_can_be_polled() -> TestResult {
    let dir = temp_dir("background")?;
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.auto_background = Some(std::time::Duration::from_millis(200));
    let started = tool.execute(args(&[("command", json!("sleep 1; echo woke"))]), &context);
    let announcement = text_of(&started.result.content);
    assert!(
        announcement.contains("still running after"),
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

/// Dies with the 200 ms poll back in the job wait: a job ending just after the wait began was
/// seen a poll late, so five waits took a second or more.
#[test]
fn a_job_wait_returns_as_the_job_settles() -> TestResult {
    let dir = temp_dir("background-settle")?;
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.auto_background = Some(std::time::Duration::from_millis(80));
    let mut waited = std::time::Duration::ZERO;
    for _ in 0..5 {
        let started = tool.execute(args(&[("command", json!("sleep 0.1"))]), &context);
        let job = started.result.details.get("job").and_then(Value::as_u64);
        let job = job.ok_or("no job id")?;
        let polling = std::time::Instant::now();
        let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(5))]), &context);
        waited += polling.elapsed();
        let finished = text_of(&polled.result.content);
        assert!(finished.contains("finished (exit 0)"), "{finished}");
    }
    assert!(
        waited < std::time::Duration::from_millis(500),
        "five waits took {waited:?}"
    );
    Ok(())
}

#[test]
fn without_auto_background_a_command_holds_the_turn() -> TestResult {
    let dir = temp_dir("no-background")?;
    let tool = BashTool::default();
    let context = ToolContext::new(dir.to_path_buf());
    let output = tool.execute(
        args(&[("command", json!("sleep 0.3; echo done"))]),
        &context,
    );
    assert!(text_of(&output.result.content).contains("done"));
    Ok(())
}

/// Issue #758: the dogfood's `sleep 20` with `wait=5` held the turn for 20 s, since `wait` was
/// read only by a poll. `echo began` is the output so far the job announcement carries.
#[test]
fn a_wait_backgrounds_a_command_still_running() -> TestResult {
    let dir = temp_dir("bash-wait")?;
    let (tool, context) = (BashTool::default(), ToolContext::new(dir.to_path_buf()));
    let clock = std::time::Instant::now();
    let command = json!("echo began; sleep 8; echo slept");
    let started = tool.execute(args(&[("command", command), ("wait", json!(5))]), &context);
    let text = text_of(&started.result.content);
    assert!(
        clock.elapsed() < std::time::Duration::from_secs(7),
        "{text}"
    );
    let details = &started.result.details;
    assert_eq!(details["backgrounded"], json!(true), "{text}");
    assert_eq!(
        (&details["afterMs"], &details["timeoutSecs"]),
        (&json!(5000), &json!(300))
    );
    assert!(text.contains("began") && !text.contains("slept"), "{text}");
    let job = details["job"].as_u64().ok_or("no job id")?;
    assert!(
        text.contains(&format!(
            "only if it exits while this turn is still running; bash job={job} wait=<s> waits for it now"
        )),
        "{text}"
    );
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(10))]), &context);
    let finished = text_of(&polled.result.content);
    assert!(
        finished.contains("finished (exit 0)") && finished.contains("slept"),
        "{finished}"
    );
    Ok(())
}

/// The kill counts from the command's start: from the backgrounding it would land at 13 s.
#[test]
fn a_backgrounded_job_still_dies_at_timeout_secs() -> TestResult {
    let dir = temp_dir("bash-wait-timeout")?;
    let (tool, context) = (BashTool::default(), ToolContext::new(dir.to_path_buf()));
    let clock = std::time::Instant::now();
    let started = tool.execute(
        args(&[
            ("command", json!("sleep 30")),
            ("wait", json!(5)),
            ("timeout_secs", json!(8)),
        ]),
        &context,
    );
    let text = text_of(&started.result.content);
    let job = started.result.details["job"].as_u64().ok_or(text)?;
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(10))]), &context);
    let killed = text_of(&polled.result.content);
    assert!(
        killed.contains(&format!(
            "job {job} killed (timeout_secs or an interrupt): sleep 30"
        )),
        "{killed}"
    );
    let at = clock.elapsed();
    assert!(at < std::time::Duration::from_secs(11), "killed at {at:?}");
    Ok(())
}

#[test]
fn a_wait_not_below_timeout_secs_runs_in_the_turn() -> TestResult {
    let dir = temp_dir("bash-wait-reaches")?;
    let context = ToolContext::new(dir.to_path_buf());
    let output = BashTool::default().execute(
        args(&[
            ("command", json!("sleep 30")),
            ("wait", json!(1)),
            ("timeout_secs", json!(5)),
        ]),
        &context,
    );
    let text = text_of(&output.result.content);
    assert_eq!(output.result.details["timedOut"], json!(true), "{text}");
    assert!(
        text.contains("[wait 1s (clamped to 5s) is not below timeout_secs 5s, so it ran in the turn; to get the turn back, pass a wait below timeout_secs or raise timeout_secs above the wait]"),
        "{text}"
    );
    Ok(())
}

/// A long line and many lines, per `.ruler/045-loud-caps.md`: the tail is bounded and says so,
/// and the total counts what the command printed, not the 30,000 bytes the live buffer keeps.
#[test]
fn the_output_so_far_names_its_cut() -> TestResult {
    let dir = temp_dir("bash-wait-tail")?;
    let command = "seq 1 20000; head -c 20000 /dev/zero | tr '\\0' x; echo; echo last; sleep 6";
    let started = BashTool::default().execute(
        args(&[("command", json!(command)), ("wait", json!(5))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text = text_of(&started.result.content);
    let job = started.result.details["job"]
        .as_u64()
        .ok_or_else(|| text.clone())?;
    assert!(text.len() < 3_000, "{} bytes", text.len());
    assert!(text.ends_with("last"), "{text}");
    assert!(
        text.contains(&format!("[the last 2048 of 128900 bytes printed so far (the preview keeps 20 lines, at most 2048 bytes); bash job={job} wait=<s> returns its output once it exits]")),
        "{text}"
    );
    Ok(())
}

/// A context whose interrupt fires once `fire` is set, as Esc or the deadline fires a session's.
fn interruptible(dir: &std::path::Path) -> (ToolContext, Arc<std::sync::atomic::AtomicBool>) {
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut context = ToolContext::new(dir.to_path_buf());
    let flag = Arc::clone(&fired);
    context.cancelled = Arc::new(move || flag.load(std::sync::atomic::Ordering::SeqCst));
    (context, fired)
}

/// Nothing else stops a job: there is no kill verb, and the watchdog dies with yi (#819).
#[test]
fn an_interrupt_after_backgrounding_kills_the_job() -> TestResult {
    let dir = temp_dir("bash-wait-interrupt")?;
    let (context, fire) = interruptible(&dir);
    let tool = BashTool::default();
    let started = tool.execute(
        args(&[("command", json!("sleep 7; echo kept")), ("wait", json!(5))]),
        &context,
    );
    let job = started.result.details["job"].as_u64().ok_or("no job id")?;
    fire.store(true, std::sync::atomic::Ordering::SeqCst);
    let poll = ToolContext::new(dir.to_path_buf());
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(10))]), &poll);
    let killed = text_of(&polled.result.content);
    assert!(
        killed.contains(&format!(
            "job {job} killed (timeout_secs or an interrupt): sleep 7; echo kept"
        )),
        "{killed}"
    );
    Ok(())
}

/// The text tells the model to end its turn on this poll, so Esc must reach it within a second.
#[test]
fn an_interrupt_ends_a_poll_within_a_second() -> TestResult {
    let dir = temp_dir("bash-wait-poll-interrupt")?;
    let tool = BashTool::default();
    let started = tool.execute(
        args(&[("command", json!("sleep 9")), ("wait", json!(5))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let job = started.result.details["job"].as_u64().ok_or("no job id")?;
    let (poll, fire) = interruptible(&dir);
    let clock = std::time::Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        fire.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(12))]), &poll);
    let at = clock.elapsed();
    let text = text_of(&polled.result.content);
    assert!(
        text.contains(&format!("job {job} still running: sleep 9")),
        "{text}"
    );
    assert!(
        at < std::time::Duration::from_secs(3),
        "returned after {at:?}"
    );
    Ok(())
}

/// The completion loop woke on an inline command's settle and could take it before the call
/// marked it reported, so a command whose output the call returned was announced again.
#[test]
fn a_command_that_finishes_in_the_turn_is_never_announced() -> TestResult {
    let dir = temp_dir("bash-inline")?;
    let owner = yi_tools::jobs::JobOwner::mint();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let (mut seen, mut taken) = (0, Vec::new());
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                seen = yi_tools::jobs::registry().wait_settle(seen);
                taken.extend(yi_tools::jobs::registry().take_finished(owner));
            }
            taken
        })
    };
    let tool = BashTool::default();
    let mut context = ToolContext::new(dir.to_path_buf());
    context.job_owner = Some(owner);
    let output = tool.execute(args(&[("command", json!("echo inline"))]), &context);
    assert!(text_of(&output.result.content).contains("inline"));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let elsewhere = temp_dir("bash-inline-wake")?;
    let _one_more_settle_wakes_the_listener = tool.execute(
        args(&[("command", json!("true"))]),
        &ToolContext::new(elsewhere.to_path_buf()),
    );
    let taken = listener.join().map_err(|_| "the listener panicked")?;
    let taken: Vec<String> = taken.iter().map(yi_tools::JobReport::headline).collect();
    assert!(taken.is_empty(), "{taken:?}");
    Ok(())
}

/// A job a poll returned finished is not sent again as an `<async_result>`.
#[test]
fn a_polled_job_is_not_reported_again() -> TestResult {
    let dir = temp_dir("bash-wait-polled")?;
    let (tool, mut context) = (BashTool::default(), ToolContext::new(dir.to_path_buf()));
    let owner = yi_tools::jobs::JobOwner::mint();
    context.job_owner = Some(owner);
    let started = tool.execute(
        args(&[("command", json!("sleep 6")), ("wait", json!(5))]),
        &context,
    );
    let job = started.result.details["job"].as_u64().ok_or("no job id")?;
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(10))]), &context);
    assert!(text_of(&polled.result.content).contains("finished (exit 0)"));
    let reported = yi_tools::jobs::registry().take_finished(owner);
    assert!(reported.is_empty(), "{reported:?}");
    Ok(())
}

/// A poll that gives up hands the job back, so its result is still reported when it exits, and
/// only to the session that started it.
#[test]
fn a_poll_that_gives_up_leaves_the_job_to_its_report() -> TestResult {
    let dir = temp_dir("bash-wait-gives-up")?;
    let (tool, mut context) = (BashTool::default(), ToolContext::new(dir.to_path_buf()));
    let owner = yi_tools::jobs::JobOwner::mint();
    context.job_owner = Some(owner);
    let started = tool.execute(
        args(&[("command", json!("sleep 11")), ("wait", json!(5))]),
        &context,
    );
    let job = started.result.details["job"].as_u64().ok_or("no job id")?;
    let polled = tool.execute(args(&[("job", json!(job)), ("wait", json!(5))]), &context);
    assert!(text_of(&polled.result.content).contains("still running"));
    let id = yi_tools::jobs::JobId(job);
    yi_tools::jobs::registry().wait_settled(Some(id), std::time::Duration::from_secs(10));
    let other = yi_tools::jobs::JobOwner::mint();
    let taken_elsewhere = yi_tools::jobs::registry().take_finished(other);
    assert!(
        taken_elsewhere.is_empty(),
        "another session's loop took it: {taken_elsewhere:?}"
    );
    let reported: Vec<u64> = yi_tools::jobs::registry()
        .take_finished(owner)
        .iter()
        .map(|r| r.id.0)
        .collect();
    assert_eq!(reported, [job]);
    Ok(())
}

#[test]
fn an_inspecting_shell_command_is_not_flagged_irreversible() -> TestResult {
    let tool = BashTool::default();
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

/// The gate judges a reversible read only by what a read can leak (D180), so a read-kind
/// tool whose call writes must not pass for one.
#[test]
fn a_grep_that_rewrites_is_flagged_irreversible() {
    let grep = GrepTool::default();
    let preview = args(&[("pattern", json!("x")), ("replace", json!("y"))]);
    assert!(!grep.irreversible(&preview), "a preview writes nothing");
    assert!(grep.irreversible(&preview_args()), "apply writes every hit");
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
        dir.join("c.txt"),
        "hit\nb\nc\nhit\nhit\nf\ng\nhit\nx\ny\nz\nhit",
    )?;
    let context = ToolContext::new(dir.to_path_buf());

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
    fs::write(dir.join("big.txt"), body)?;
    let context = ToolContext::new(dir.to_path_buf());

    let bare = GrepTool::default().execute(args(&[("pattern", json!("hit"))]), &context);
    assert_eq!(output_text(&bare).lines().count(), 1);

    let clamped = GrepTool::default().execute(
        args(&[("pattern", json!("hit")), ("context", json!(9_999))]),
        &context,
    );
    let text = output_text(&clamped);
    assert_eq!(text.lines().count(), 22, "{text}");
    assert!(
        text.ends_with("[context clamped to 10 lines per side; asked 9999]"),
        "{text}"
    );
    Ok(())
}

/// Without the patch on the result, the transcript and every ACP client fall
/// back to the one-line digest: an edit renders with no diff body at all.
#[test]
fn a_write_carries_its_patch_and_line_counts() -> TestResult {
    let dir = temp_dir("write-patch")?;
    let context = ToolContext::new(dir.to_path_buf());
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
    let context = ToolContext::new(dir.to_path_buf());
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
        _recovery_dir: Option<&std::path::Path>,
    ) -> Result<yi_tools::KernelCellOutcome, String> {
        Ok(yi_tools::KernelCellOutcome {
            result: self.0.clone(),
            kernel_restarted: false,
            notes: Vec::new(),
        })
    }
}

/// A kernel edit becomes a patch here, where every other patch is computed. The
/// point is the LCS: a whole-file dump would mark all four lines changed.
#[test]
fn a_kernel_edit_arrives_as_a_real_patch() -> TestResult {
    let dir = temp_dir("kernel-diff")?;
    let context = ToolContext::new(dir.to_path_buf());
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
    fs::write(dir.join("a.rs"), "fn alpha() {}\nfn beta() {}\n")?;
    fs::write(dir.join("b.py"), "def alpha():\n    pass\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let grep = GrepTool::default();

    let literal = grep.execute(
        args(&[("pattern", json!("fn alpha(")), ("literal", json!(true))]),
        &context,
    );
    assert!(output_text(&literal).contains("a.rs:1:fn alpha"));

    let rx = grep.execute(args(&[("pattern", json!("fn (alpha|beta)"))]), &context);
    assert_eq!(output_text(&rx).lines().count(), 2, "{}", output_text(&rx));

    let bad = grep.execute(args(&[("pattern", json!("fn ("))]), &context);
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
    fs::write(dir.join("x.rs"), "one\ntwo\nthree\n")?;
    let context = ToolContext::new(dir.to_path_buf());
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
    let canonical = dir.join("x.rs").canonicalize()?.display().to_string();
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
    fs::write(dir.join("r.txt"), &body)?;
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let read = yi_tools::hashline::tool::HashlineReadTool::new(std::sync::Arc::clone(&state));
    let context = ToolContext::new(dir.to_path_buf());
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
    fs::write(dir.join("wide.txt"), &long)?;
    let out = read.execute(args(&[("path", json!("wide.txt"))]), &context);
    let text = output_text(&out);
    assert!(text.contains("were clipped"), "{text}");
    assert!(text.contains("sed -n '2p'"), "{text}");
    let canonical = dir.join("wide.txt").canonicalize()?.display().to_string();
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
    fs::write(dir.join("r.txt"), &body)?;
    let read = yi_tools::hashline::tool::HashlineReadTool::new(
        yi_tools::hashline::tool::shared_hashline_state(),
    );
    let context = ToolContext::new(dir.to_path_buf());
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
    fs::write(dir.join("wide.txt"), &wide)?;
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
    fs::write(dir.join("full.txt"), &body)?;
    let grep = yi_tools::GrepTool::default();
    let context = ToolContext::new(dir.to_path_buf());
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
    let context = ToolContext::new(dir.to_path_buf());
    let out = BashTool::default().execute(args(&[("command", json!("ls"))]), &context);
    assert_eq!(
        out.result.details.get("category").and_then(Value::as_str),
        Some("list_files")
    );
    Ok(())
}

#[test]
fn a_bash_view_of_one_file_carries_an_edit_anchor() -> TestResult {
    let dir = temp_dir("bash-bridge")?;
    fs::write(dir.join("a.txt"), "one\ntwo\nthree\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let mut bash = BashTool::default();
    bash.hashline = Some(std::sync::Arc::clone(&state));
    let viewed = bash.execute(args(&[("command", json!("cat a.txt"))]), &context);
    let text = output_text(&viewed);
    assert!(text.starts_with("[a.txt#"), "{text}");
    let tag = text
        .lines()
        .next()
        .and_then(|header| header.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag")?;
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state: std::sync::Arc::clone(&state),
        freeform_grammar: false,
    }
    .execute(
        args(&[("patch", json!(format!("[a.txt#{tag}]\nPUT 2.=2:\n+TWO\n")))]),
        &context,
    );
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(fs::read_to_string(dir.join("a.txt"))?, "one\nTWO\nthree\n");

    let piped = bash.execute(args(&[("command", json!("cat a.txt | head -1"))]), &context);
    assert!(
        !output_text(&piped).starts_with('['),
        "{}",
        output_text(&piped)
    );
    assert_eq!(piped.result.details["bridge"], json!("compound"));
    Ok(())
}

/// Runs `command` through a bridged bash, then a one-line PUT at `line`; returns the edit's text
/// and whether it was refused as never displayed.
fn bash_then_put(
    dir: &Scratch,
    context: &ToolContext,
    command: &str,
    path: &str,
    line: u64,
) -> Result<(bool, String), Box<dyn Error>> {
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let mut bash = BashTool::default();
    bash.hashline = Some(Arc::clone(&state));
    let viewed = bash.execute(args(&[("command", json!(command))]), context);
    let text = output_text(&viewed);
    let tag = text
        .lines()
        .next()
        .and_then(|header| header.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or_else(|| format!("no tag in {text}"))?;
    let old = fs::read_to_string(dir.join(path))?;
    let current = old
        .split('\n')
        .nth(usize::try_from(line)?.saturating_sub(1))
        .ok_or("line past the end")?
        .to_owned();
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state,
        freeform_grammar: false,
    }
    .execute(
        args(&[(
            "patch",
            json!(format!(
                "[{path}#{tag}]\nPUT {line}.={line}:\n+{current} // x\n"
            )),
        )]),
        context,
    );
    let message = output_text(&edit);
    Ok((
        edit.is_error && message.contains("never displayed"),
        message,
    ))
}

/// A bare `cat` whose output was cut at the capture ceiling showed its head and tail, yet marked
/// every line seen, so an edit anchored in the cut middle went through blind.
#[test]
fn a_truncated_cat_leaves_the_cut_lines_unseen() -> TestResult {
    let dir = temp_dir("bash-cat-truncated")?;
    let body: String = (1..=4000).map(|n| format!("row {n:05}\n")).collect();
    fs::write(dir.join("big.txt"), &body)?;
    let context = ToolContext::new(dir.to_path_buf());
    let (refused, message) = bash_then_put(&dir, &context, "cat big.txt", "big.txt", 2000)?;
    assert!(refused, "{message}");
    let (refused, message) = bash_then_put(&dir, &context, "cat big.txt", "big.txt", 5)?;
    assert!(!refused, "{message}");
    Ok(())
}

/// The reducer replaced the middle of a bare `cat` with an omitted-lines marker; those lines
/// were counted seen from the raw capture the model never got.
#[test]
fn a_reduced_cat_leaves_the_omitted_lines_unseen() -> TestResult {
    let dir = temp_dir("bash-cat-reduced")?;
    let body: String = (1..=1500).map(|n| format!("row {n:05}\n")).collect();
    fs::write(dir.join("mid.txt"), &body)?;
    let mut context = ToolContext::new(dir.to_path_buf());
    let recovery = dir.join("recovery");
    fs::create_dir_all(&recovery)?;
    context.recovery_dir = Some(recovery);
    let (refused, message) = bash_then_put(&dir, &context, "cat mid.txt", "mid.txt", 700)?;
    assert!(refused, "{message}");
    let (refused, message) = bash_then_put(&dir, &context, "cat mid.txt", "mid.txt", 10)?;
    assert!(!refused, "{message}");
    Ok(())
}

/// A `}` shown by sed between two lines it belongs with was seen; alone it stays unseen,
/// because a bare `}` is in almost any output.
#[test]
fn a_short_line_shown_beside_its_neighbour_is_seen() -> TestResult {
    let dir = temp_dir("bash-short-line")?;
    fs::write(
        dir.join("f.rs"),
        "fn a() {\n    one();\n}\nfn b() {\n    two();\n}\n",
    )?;
    let context = ToolContext::new(dir.to_path_buf());
    let (refused, message) = bash_then_put(&dir, &context, "sed -n '6p' f.rs", "f.rs", 6)?;
    assert!(refused, "a lone brace counted as seen: {message}");
    let (refused, message) = bash_then_put(&dir, &context, "sed -n '5,6p' f.rs", "f.rs", 6)?;
    assert!(!refused, "{message}");
    Ok(())
}

#[test]
fn a_read_only_command_is_a_read_kind_call() {
    let bash = BashTool::default();
    let table = [
        ("cat f", ToolKind::Read),
        ("rg -n x crates", ToolKind::Read),
        ("ls -la && git log -3", ToolKind::Read),
        ("grid uses X", ToolKind::Read),
        ("grid survey", ToolKind::Exec),
        ("cat f > g", ToolKind::Exec),
        ("cargo test", ToolKind::Exec),
        ("rm f", ToolKind::Exec),
    ];
    for (command, kind) in table {
        assert_eq!(
            bash.kind_for(&args(&[("command", json!(command))])),
            kind,
            "{command}"
        );
    }
}

#[test]
fn a_directory_reads_as_a_listing_with_skeletons() -> TestResult {
    let dir = temp_dir("read-dir")?;
    fs::create_dir_all(dir.join("src/sub"))?;
    fs::write(dir.join("src/a.rs"), "pub fn alpha() {}\nstruct Beta;\n")?;
    fs::write(dir.join("src/notes.txt"), "plain\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let out = read_tool().execute(args(&[("path", json!("src"))]), &context);
    let text = output_text(&out);
    assert!(text.starts_with("[src]\nsub/\na.rs  "), "{text}");
    assert!(text.contains("notes.txt  6 B"), "{text}");
    assert!(
        text.contains("[skeleton: 1 of 1 source files, up to 8 heads each — read a file for the rest]\na.rs: pub fn alpha() {}; struct Beta;"),
        "{text}"
    );
    Ok(())
}

/// A read of a build or dataset directory listed every entry with no cap and no count.
#[test]
fn a_large_directory_listing_names_its_cut() -> TestResult {
    let dir = temp_dir("read-dir-cap")?;
    fs::create_dir_all(dir.join("big/sub"))?;
    for index in 0..250 {
        fs::write(dir.join(format!("big/f{index:03}.bin")), "x")?;
    }
    let context = ToolContext::new(dir.to_path_buf());
    let text = output_text(&read_tool().execute(args(&[("path", json!("big"))]), &context));
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.get(1), Some(&"sub/"), "{text}");
    assert_eq!(rows.get(200), Some(&"f198.bin  1 B"), "{text}");
    assert_eq!(
        rows.get(201),
        Some(&"[showing 200 of 251 entries — read a narrower path]"),
        "{text}"
    );
    assert!(!text.contains("f199.bin"), "{text}");
    Ok(())
}

/// The clip hint named only the first clipped line and left the path unquoted, so a path with a
/// space or an apostrophe broke the command it suggested.
#[test]
fn every_clipped_line_is_named_in_one_runnable_sed() -> TestResult {
    let dir = temp_dir("read-clip-all")?;
    let wide = "x".repeat(5_000);
    fs::write(
        dir.join("it's notes.txt"),
        format!("short\n{wide}\nmid\n{wide}\n{wide}\nend\n"),
    )?;
    let context = ToolContext::new(dir.to_path_buf());
    let text =
        output_text(&read_tool().execute(args(&[("path", json!("it's notes.txt"))]), &context));
    let command = r"sed -n '2p;4,5p' 'it'\''s notes.txt'";
    assert!(text.contains(command), "{text}");
    let shown =
        output_text(&BashTool::default().execute(args(&[("command", json!(command))]), &context));
    assert_eq!(shown.matches(&wide).count(), 3, "{shown}");
    Ok(())
}

/// An edit between two grep pages moved the next offset onto different matches with no notice.
#[test]
fn a_grep_page_after_the_matches_changed_says_so() -> TestResult {
    let dir = temp_dir("grep-sweep")?;
    let hits = |count: usize| -> String { (0..count).map(|n| format!("hit {n}\n")).collect() };
    fs::write(dir.join("a.txt"), hits(5))?;
    fs::write(dir.join("b.txt"), hits(300))?;
    let context = ToolContext::new(dir.to_path_buf());
    let grep = GrepTool {
        hashline: Some(yi_tools::hashline::tool::shared_hashline_state()),
    };
    let page = |offset: usize| {
        output_text(&grep.execute(
            args(&[("pattern", json!("hit")), ("offset", json!(offset))]),
            &context,
        ))
    };
    let first = page(0);
    assert!(first.contains("continue with offset=200"), "{first}");
    let steady = page(200);
    assert!(!steady.contains("matches changed"), "{steady}");
    page(0);
    fs::write(dir.join("a.txt"), hits(1))?;
    let moved = page(200);
    assert!(
        moved
            .contains("[matches changed since the last page; offsets now count the current sweep]"),
        "{moved}"
    );
    Ok(())
}

/// Grep listed files in the filesystem's order, which differs between machines and after a rename.
#[test]
fn grep_lists_files_in_name_order() -> TestResult {
    let dir = temp_dir("grep-order")?;
    let names = [
        "q.txt", "c.txt", "x.txt", "a.txt", "m.txt", "b.txt", "z.txt", "k.txt",
    ];
    for name in names {
        fs::write(dir.join(name), "needle\n")?;
    }
    let context = ToolContext::new(dir.to_path_buf());
    let text = output_text(&GrepTool::default().execute(
        args(&[
            ("pattern", json!("needle")),
            ("files_with_matches", json!(true)),
        ]),
        &context,
    ));
    let listed: Vec<&str> = names
        .iter()
        .filter_map(|name| text.find(name).map(|at| (at, *name)))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    assert_eq!(listed, sorted, "{text}");
    Ok(())
}

#[test]
fn find_shows_the_block_and_the_references_in_one_read() -> TestResult {
    let dir = temp_dir("read-find")?;
    fs::write(
        dir.join("lib.rs"),
        "fn helper() {\n    1\n}\n\nfn main() {\n    helper();\n}\n",
    )?;
    fs::write(dir.join("other.rs"), "use crate::helper;\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let out = read_tool().execute(
        args(&[("path", json!("lib.rs")), ("find", json!("fn helper"))]),
        &context,
    );
    let text = output_text(&out);
    assert!(text.starts_with("[lib.rs#"), "{text}");
    assert!(
        text.contains("[find: block at lines 1-3 of 7]\n1:fn helper() {\n2:    1\n3:}\n"),
        "{text}"
    );
    assert!(
        !text.contains("5:fn main"),
        "the window is the block: {text}"
    );
    assert!(text.contains("[refs: 2 of 2 for helper]"), "{text}");
    assert!(text.contains("6:    helper();"), "{text}");
    assert!(text.contains("[other.rs#"), "{text}");

    let miss = read_tool().execute(
        args(&[("path", json!("lib.rs")), ("find", json!("fn helpers"))]),
        &context,
    );
    assert!(miss.is_error);
    assert!(
        output_text(&miss).contains("nearest by"),
        "{}",
        output_text(&miss)
    );
    Ok(())
}

#[test]
fn a_read_cut_by_the_line_cap_ends_with_the_skeleton() -> TestResult {
    let dir = temp_dir("read-skeleton")?;
    let mut big = String::new();
    for index in 0..2_100 {
        big.push_str(&format!("let x{index} = {index};\n"));
    }
    big.push_str("fn tail() {}\n");
    fs::write(dir.join("big.rs"), &big)?;
    let context = ToolContext::new(dir.to_path_buf());
    let out = read_tool().execute(args(&[("path", json!("big.rs"))]), &context);
    let text = output_text(&out);
    assert!(
        text.contains("[skeleton: 1 top-level declarations]\n  2101  fn tail()"),
        "{text}"
    );
    let small = read_tool().execute(
        args(&[("path", json!("big.rs")), ("limit", json!(5))]),
        &context,
    );
    assert!(!output_text(&small).contains("[skeleton]"));
    Ok(())
}

#[test]
fn grep_def_count_and_block_modes() -> TestResult {
    let dir = temp_dir("grep-modes")?;
    fs::write(
        dir.join("a.rs"),
        "fn alpha() {\n    beta();\n}\nfn beta() {\n    alpha();\n    alpha();\n}\n",
    )?;
    let context = ToolContext::new(dir.to_path_buf());
    let grep = GrepTool {
        hashline: Some(yi_tools::hashline::tool::shared_hashline_state()),
    };
    let def = grep.execute(
        args(&[("pattern", json!("alpha")), ("def", json!(true))]),
        &context,
    );
    let text = output_text(&def);
    assert!(
        text.starts_with("[a.rs#") && text.ends_with("]\n1:fn alpha() {"),
        "{text}"
    );
    let count = grep.execute(
        args(&[("pattern", json!("alpha")), ("count", json!(true))]),
        &context,
    );
    assert!(
        output_text(&count).trim().starts_with("3 "),
        "{}",
        output_text(&count)
    );
    let block = grep.execute(
        args(&[("pattern", json!("fn beta")), ("block", json!(true))]),
        &context,
    );
    let text = output_text(&block);
    assert!(
        text.contains("4:fn beta() {\n5-    alpha();\n6-    alpha();\n7-}"),
        "{text}"
    );
    let many = grep.execute(
        args(&[("pattern", json!(["fn alpha", "fn beta"]))]),
        &context,
    );
    let text = output_text(&many);
    assert!(
        text.contains("1:fn alpha() {") && text.contains("4:fn beta() {"),
        "{text}"
    );
    let multi = grep.execute(
        args(&[
            ("pattern", json!("beta\\(\\);\\n\\}")),
            ("multiline", json!(true)),
        ]),
        &context,
    );
    assert!(
        output_text(&multi).contains("\n2:    beta();"),
        "{}",
        output_text(&multi)
    );
    Ok(())
}

#[test]
fn grep_replace_previews_then_applies_and_tags() -> TestResult {
    let dir = temp_dir("grep-replace")?;
    fs::write(dir.join("a.rs"), "fn old_name() {}\n")?;
    fs::write(dir.join("b.rs"), "old_name();\r\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let grep = GrepTool {
        hashline: Some(Arc::clone(&state)),
    };
    let preview = grep.execute(
        args(&[
            ("pattern", json!("old_([a-z]+)")),
            ("replace", json!("new_$1")),
        ]),
        &context,
    );
    let text = output_text(&preview);
    assert!(text.contains("+fn new_name() {}"), "{text}");
    assert!(text.contains("[preview: 2 files would change"), "{text}");
    assert_eq!(fs::read_to_string(dir.join("a.rs"))?, "fn old_name() {}\n");
    assert_eq!(
        grep.kind_for(&args(&[("pattern", json!("x")), ("replace", json!("y"))])),
        ToolKind::Read
    );

    let applied = grep.execute(preview_args(), &context);
    assert!(
        output_text(&applied).contains("applied to 2 of 2 files"),
        "{}",
        output_text(&applied)
    );
    assert_eq!(fs::read_to_string(dir.join("a.rs"))?, "fn new_name() {}\n");
    assert_eq!(fs::read_to_string(dir.join("b.rs"))?, "new_name();\r\n");
    assert_eq!(grep.kind_for(&preview_args()), ToolKind::Write);
    let canonical = dir.join("a.rs").canonicalize()?.display().to_string();
    let guard = state.lock().map_err(|_| "poisoned")?;
    assert!(
        guard.snapshots.head(&canonical).is_some(),
        "the rewrite is tagged"
    );
    Ok(())
}

/// A grep apply lands files the way `write` and `edit` do: a rewrite that breaks a Python file
/// says so, and the result carries the patch a diff pane renders.
#[test]
fn a_grep_apply_carries_the_syntax_verdict_and_its_patch() -> TestResult {
    if !on_path("python3") {
        return Ok(());
    }
    let dir = temp_dir("grep-apply-land")?;
    fs::write(dir.join("a.py"), "x = 1\n")?;
    // A later file that still parses must not wash out the earlier failure.
    fs::write(dir.join("b.py"), "s = '= 1'\n")?;
    let applied = GrepTool::default().execute(
        args(&[
            ("pattern", json!("= 1")),
            ("replace", json!("= (")),
            ("apply", json!(true)),
        ]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text = output_text(&applied);
    assert!(text.contains("a.py: syntax: error line 1"), "{text}");
    let patch = applied.result.details["patch"]
        .as_str()
        .ok_or(text.clone())?;
    assert!(
        patch.contains("-x = 1") && patch.contains("+x = ("),
        "{patch}"
    );
    // ACP renders the patch headers as paths, so they name the file absolutely, as write's do.
    let absolute = dir.join("a.py").canonicalize()?.display().to_string();
    assert!(patch.contains(&format!("+++ b/{absolute}")), "{patch}");
    let syntax = applied.result.details["syntax"]
        .as_str()
        .unwrap_or_default();
    assert!(syntax.starts_with("syntax: error"), "{syntax}");
    // No checker ran on a text file, so no verdict is claimed.
    fs::write(dir.join("notes.txt"), "x = 1\n")?;
    let unchecked = GrepTool::default().execute(
        args(&[
            ("pattern", json!("= 1")),
            ("replace", json!("= 2")),
            ("include", json!("*.txt")),
            ("apply", json!(true)),
        ]),
        &ToolContext::new(dir.to_path_buf()),
    );
    assert_eq!(
        unchecked.result.details["syntax"],
        Value::Null,
        "{}",
        output_text(&unchecked)
    );
    Ok(())
}

/// Approval of a write is approval of the named path; through a symlink the bytes land at its
/// target, a file nobody reviewed.
#[cfg(unix)]
#[test]
fn a_write_through_a_symlink_is_refused() -> TestResult {
    let dir = temp_dir("write-symlink")?;
    fs::write(dir.join("target.txt"), "kept\n")?;
    std::os::unix::fs::symlink(dir.join("target.txt"), dir.join("link.txt"))?;
    let refused = WriteTool::default().execute(
        args(&[("path", json!("link.txt")), ("content", json!("lost\n"))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    assert!(refused.is_error, "{}", output_text(&refused));
    assert!(
        output_text(&refused).contains("is a symlink"),
        "{}",
        output_text(&refused)
    );
    assert_eq!(fs::read_to_string(dir.join("target.txt"))?, "kept\n");
    Ok(())
}

fn preview_args() -> Map<String, Value> {
    args(&[
        ("pattern", json!("old_([a-z]+)")),
        ("replace", json!("new_$1")),
        ("apply", json!(true)),
    ])
}

/// A file's skeleton at the per-file cap shows every head; one past it names kept of total, the
/// cap and the call that lists the rest.
#[test]
fn a_skeleton_one_past_its_cap_names_the_cut() -> TestResult {
    let dir = temp_dir("skeleton-cap")?;
    let heads = |count: usize| {
        (0..count)
            .map(|n| format!("fn f{n}() {{}}\n"))
            .collect::<String>()
    };
    fs::write(dir.join("at.rs"), heads(8))?;
    fs::write(dir.join("over.rs"), heads(9))?;
    let listing = read_tool().execute(
        args(&[("path", json!("."))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text = output_text(&listing);
    let row = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(name))
            .map(str::to_owned)
    };
    let at = row("at.rs:").ok_or(text.clone())?;
    assert!(at.ends_with("fn f7() {}"), "{text}");
    let over = row("over.rs:").ok_or(text.clone())?;
    assert!(
        over.ends_with("fn f7() {}; [8 of 9 heads, cap 8 per file — grep def=true for all]"),
        "{text}"
    );
    // A glob shows files whole until its byte budget; one past the budget is shown as heads.
    let padding = "// padding\n".repeat(6_000);
    fs::write(dir.join("big.rs"), format!("{}{padding}", heads(13)))?;
    let glob = read_tool().execute(
        args(&[("path", json!("big*.rs"))]),
        &ToolContext::new(dir.to_path_buf()),
    );
    let text = output_text(&glob);
    assert!(
        text.contains("  fn f11() {}\n  [12 of 13 heads, cap 12 per file — grep def=true for all]"),
        "{text}"
    );
    Ok(())
}

#[test]
fn every_cut_view_names_its_cap() -> TestResult {
    let dir = temp_dir("loud-caps")?;
    let mut many = String::new();
    for index in 0..45 {
        many.push_str(&format!("fn f{index}() {{}}\n"));
    }
    for index in 0..2_100 {
        many.push_str(&format!("let x{index} = 1;\n"));
    }
    fs::write(dir.join("many.rs"), &many)?;
    fs::write(dir.join("notes.txt"), "alpha beta\n")?;
    fs::write(dir.join("blob.bin"), b"alpha\0beta")?;
    let context = ToolContext::new(dir.to_path_buf());

    let capped = read_tool().execute(args(&[("path", json!("many.rs"))]), &context);
    let text = output_text(&capped);
    assert!(
        text.contains("[skeleton: first 40 of 45 top-level declarations"),
        "{text}"
    );

    let plain = read_tool().execute(
        args(&[("path", json!("notes.txt")), ("find", json!("beta"))]),
        &context,
    );
    let text = output_text(&plain);
    assert!(
        text.contains("[find: no enclosing block; lines 1-1 of 1 around the hit at line 1"),
        "{text}"
    );

    let listing = read_tool().execute(args(&[("path", json!("."))]), &context);
    let text = output_text(&listing);
    assert!(text.contains("many.rs: fn f0() {}; fn f1() {};"), "{text}");
    assert!(text.contains("[8 of 45 heads, cap 8 per file"), "{text}");

    let grep = GrepTool::default();
    let block = grep.execute(
        args(&[
            ("pattern", json!("alpha")),
            ("block", json!(true)),
            ("context", json!(99)),
        ]),
        &context,
    );
    let text = output_text(&block);
    assert!(
        text.contains("[line 1 opens no block; context shown instead]"),
        "{text}"
    );
    assert!(
        text.contains("[context clamped to 10 lines per side; asked 99]"),
        "{text}"
    );
    assert!(text.contains("[1 binary files skipped"), "{text}");
    Ok(())
}

/// A chain that stopped is named as such: the model read a stopped `&&` as a
/// truncation and cited numbers from segments that never ran.
#[test]
fn a_stopped_chain_says_so_beside_the_exit_code() -> TestResult {
    let tool = BashTool::default();
    let context = ToolContext::new(std::env::temp_dir());
    let output = tool.execute(
        Map::from_iter([(
            "command".to_owned(),
            json!("echo first && false && echo never"),
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
    assert!(text.contains("exit code: 1"), "{text}");
    // The annotation cannot know which segment failed, and in 15 of 59 F0e chained failures
    // it was the last one, so it states the rule rather than claiming the fact (#476).
    assert!(
        text.contains("[exit 1 inside a && chain: any segment after the failing one did not run]"),
        "{text}"
    );
    assert!(!text.contains("never"), "{text}");
    let plain = tool.execute(
        Map::from_iter([("command".to_owned(), json!("false"))]),
        &context,
    );
    let plain: String = plain
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(
        !plain.contains("chain stopped"),
        "a single command has no chain: {plain}"
    );
    Ok(())
}

fn chain_notice_for(command: &str) -> String {
    let output = BashTool::default().execute(
        args(&[("command", json!(command))]),
        &ToolContext::new(std::env::temp_dir()),
    );
    output_text(&output)
}

const CHAIN_NOTICE: &str = "inside a && chain";

#[test]
fn chain_notice_absent_when_the_list_ends_after_a_semicolon() -> TestResult {
    let text = chain_notice_for("true && true; false");
    assert!(text.contains("exit code: 1"), "{text}");
    assert!(!text.contains(CHAIN_NOTICE), "{text}");
    Ok(())
}

#[test]
fn chain_notice_present_when_the_last_operator_is_and() -> TestResult {
    let text = chain_notice_for("false && echo x");
    assert_eq!(text.matches(CHAIN_NOTICE).count(), 1, "{text}");
    Ok(())
}

#[test]
fn chain_notice_ignores_a_quoted_and() -> TestResult {
    let text = chain_notice_for("echo 'a && b'; false");
    assert!(text.contains("exit code: 1"), "{text}");
    assert!(!text.contains(CHAIN_NOTICE), "{text}");
    Ok(())
}

#[test]
fn chain_notice_present_after_a_failed_redirect() -> TestResult {
    let text = chain_notice_for("echo x > /nonexistent-dir/f && echo y");
    assert_eq!(text.matches(CHAIN_NOTICE).count(), 1, "{text}");
    assert!(!text.contains("\ny\n"), "{text}");
    Ok(())
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[test]
fn an_edit_that_breaks_python_says_so_in_its_result() -> TestResult {
    if !on_path("python3") {
        return Ok(());
    }
    let dir = temp_dir("syntax-py")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let write = WriteTool {
        hashline: Some(Arc::clone(&state)),
    };
    let written = write.execute(
        args(&[
            ("path", json!("a.py")),
            ("content", json!("def f():\n    return 1\n")),
        ]),
        &context,
    );
    let text = output_text(&written);
    assert!(text.ends_with("\nsyntax: ok"), "{text}");
    assert_eq!(written.result.details["syntax"], json!("syntax: ok"));

    let read = yi_tools::hashline::tool::HashlineReadTool::new(Arc::clone(&state))
        .execute(args(&[("path", json!("a.py"))]), &context);
    let tag = output_text(&read)
        .lines()
        .next()
        .and_then(|header| header.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag")?;
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state: Arc::clone(&state),
        freeform_grammar: false,
    }
    .execute(
        args(&[(
            "patch",
            json!(format!("[a.py#{tag}]\nPUT 1.=1:\n+def f(:\n")),
        )]),
        &context,
    );
    assert!(!edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);
    assert!(text.contains("syntax: error line"), "{text}");
    assert!(
        edit.result.details["syntax"]
            .as_str()
            .is_some_and(|line| line.starts_with("syntax: error line")),
        "{}",
        edit.result.details
    );
    Ok(())
}

#[test]
fn a_write_of_bad_json_reports_the_line() -> TestResult {
    let dir = temp_dir("syntax-json")?;
    let context = ToolContext::new(dir.to_path_buf());
    let written = WriteTool::default().execute(
        args(&[
            ("path", json!("a.json")),
            ("content", json!("{\n  \"a\": 1,\n  \"b\": oops\n}\n")),
        ]),
        &context,
    );
    let text = output_text(&written);
    assert!(text.contains("syntax: error line 3:"), "{text}");
    Ok(())
}

#[test]
fn a_file_with_no_checker_gets_no_verdict() -> TestResult {
    let dir = temp_dir("syntax-none")?;
    let context = ToolContext::new(dir.to_path_buf());
    let written = WriteTool::default().execute(
        args(&[("path", json!("a.zzz")), ("content", json!("anything\n"))]),
        &context,
    );
    let text = output_text(&written);
    assert!(!text.contains("syntax:"), "{text}");
    assert_eq!(written.result.details["syntax"], Value::Null);
    Ok(())
}

/// Incident: `write` minted the edit tag and never showed it, so 30 F0e edits after a write
/// cited an invented tag such as `#unknown` or the prompt's own `#A1B2` (#473).
#[test]
fn a_write_result_carries_the_tag_an_edit_must_cite() -> TestResult {
    let dir = temp_dir("write-tag")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let written = WriteTool {
        hashline: Some(std::sync::Arc::clone(&state)),
    }
    .execute(
        args(&[
            ("path", json!("w.py")),
            ("content", json!("a = 1\nb = 2\nc = 3\n")),
        ]),
        &context,
    );
    let text = output_text(&written);
    assert!(text.starts_with("[w.py#"), "{text}");
    assert!(text.contains("Wrote "), "{text}");
    let tag = text
        .lines()
        .next()
        .and_then(|header| header.rsplit_once('#'))
        .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
        .ok_or("no tag on the write result")?;
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state: std::sync::Arc::clone(&state),
        freeform_grammar: false,
    }
    .execute(
        args(&[(
            "patch",
            json!(format!("[w.py#{tag}]\nPUT 2.=2:\n+b = 20\n")),
        )]),
        &context,
    );
    assert!(!edit.is_error, "{}", output_text(&edit));
    assert_eq!(
        fs::read_to_string(dir.join("w.py"))?,
        "a = 1\nb = 20\nc = 3\n"
    );
    Ok(())
}

/// A placeholder tag names the road back rather than only the rule it broke (#473).
#[test]
fn a_placeholder_tag_refusal_names_where_the_tag_comes_from() -> TestResult {
    let dir = temp_dir("write-tag-refusal")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    fs::write(dir.join("p.py"), "x = 1\n")?;
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state: std::sync::Arc::clone(&state),
        freeform_grammar: false,
    }
    .execute(
        args(&[("patch", json!("[p.py#unknown]\nPUT 1.=1:\n+x = 2\n"))]),
        &context,
    );
    assert!(edit.is_error, "{}", output_text(&edit));
    let text = output_text(&edit);
    assert!(text.contains("read, write or edit result"), "{text}");
    assert!(text.contains("omit it"), "{text}");
    Ok(())
}

/// Incident: 10 F0e edits anchored to lines the model had seen under the previous tag and
/// the edit result's 3-line hunk windows did not repeat, which read as never shown (#473).
#[test]
fn an_edit_carries_the_seen_lines_it_did_not_move() -> TestResult {
    let dir = temp_dir("seen-carry")?;
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    let body: String = (1..=60).map(|line| format!("line {line}\n")).collect();
    let written = WriteTool {
        hashline: Some(std::sync::Arc::clone(&state)),
    }
    .execute(
        args(&[("path", json!("long.txt")), ("content", json!(body))]),
        &context,
    );
    let tag_of = |text: &str| -> Option<String> {
        text.lines()
            .next()
            .and_then(|header| header.rsplit_once('#'))
            .map(|(_, tail)| tail.trim_end_matches(']').to_owned())
    };
    let first = tag_of(&output_text(&written)).ok_or("no write tag")?;
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state: std::sync::Arc::clone(&state),
        freeform_grammar: false,
    };
    let one = edit.execute(
        args(&[(
            "patch",
            json!(format!("[long.txt#{first}]\nPUT 10.=10:\n+TEN\n")),
        )]),
        &context,
    );
    assert!(!one.is_error, "{}", output_text(&one));
    let second = tag_of(&output_text(&one)).ok_or("no edit tag")?;
    // Line 50 is 40 lines outside the hunk window and was never reprinted; it is a line the
    // write showed and this edit did not touch, so it stays an anchor.
    let two = edit.execute(
        args(&[(
            "patch",
            json!(format!("[long.txt#{second}]\nPUT 50.=50:\n+FIFTY\n")),
        )]),
        &context,
    );
    assert!(!two.is_error, "{}", output_text(&two));
    let final_text = fs::read_to_string(dir.join("long.txt"))?;
    assert!(final_text.contains("TEN\n"), "{final_text}");
    assert!(final_text.contains("FIFTY\n"), "{final_text}");
    Ok(())
}

/// Incident: 11 of 179 F0e bash failures were a bashism refused by dash, 10 of them process
/// substitution, because the tool named `bash` spawned `sh -c` (#476).
#[test]
fn bash_runs_process_substitution() -> TestResult {
    let dir = temp_dir("bash-bashism")?;
    let context = ToolContext::new(dir.to_path_buf());
    let run = BashTool::default().execute(
        args(&[("command", json!("diff <(echo a) <(echo b)"))]),
        &context,
    );
    let text = output_text(&run);
    assert!(!text.contains("Syntax error"), "{text}");
    // diff ran and found a difference: exit 1, not the shell's exit 2.
    assert_eq!(run.result.details["exitCode"], json!(1), "{text}");
    assert!(text.contains("a"), "{text}");
    Ok(())
}

/// Incident: nine F0e `write` calls passed a JSON object as `content` and read the refusal's
/// "missing" as absent, so each repeated the same call (#472).
#[test]
fn a_wrong_typed_argument_is_not_reported_as_missing() -> TestResult {
    let dir = temp_dir("write-typed")?;
    let context = ToolContext::new(dir.to_path_buf());
    let written = WriteTool::default().execute(
        args(&[
            ("path", json!("a.json")),
            ("content", json!({"key": "value"})),
        ]),
        &context,
    );
    assert!(written.is_error);
    let text = output_text(&written);
    assert!(!text.contains("missing"), "{text}");
    assert!(text.contains("a JSON object, not a string"), "{text}");
    assert!(text.contains("json.dumps"), "{text}");
    Ok(())
}

/// A path that does not exist answered "No matches found", a clean miss over a typo.
#[test]
fn grep_refuses_a_missing_path_naming_it() -> TestResult {
    let dir = temp_dir("grep-missing-path")?;
    fs::write(dir.join("one.txt"), "needle\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let missing = GrepTool::default().execute(
        args(&[("pattern", json!("needle")), ("path", json!("nope/"))]),
        &context,
    );
    let text = output_text(&missing);
    assert!(missing.is_error, "{text}");
    assert!(text.contains("nope/"), "{text}");
    assert!(!text.contains("No matches found"), "{text}");
    Ok(())
}

/// A negative offset became 0 and paged from the top as if the request were fine.
#[test]
fn grep_refuses_a_negative_offset() -> TestResult {
    let dir = temp_dir("grep-negative-offset")?;
    fs::write(dir.join("one.txt"), "needle\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let refused = GrepTool::default().execute(
        args(&[("pattern", json!("needle")), ("offset", json!(-1))]),
        &context,
    );
    let text = output_text(&refused);
    assert!(refused.is_error, "{text}");
    assert!(text.contains("offset") && text.contains("-1"), "{text}");
    Ok(())
}

/// A negative context ran with 0 and said nothing, unlike the clamp of a large one.
#[test]
fn grep_reports_a_context_it_could_not_honour() -> TestResult {
    let dir = temp_dir("grep-negative-context")?;
    fs::write(dir.join("one.txt"), "needle\n")?;
    let context = ToolContext::new(dir.to_path_buf());
    let ran = GrepTool::default().execute(
        args(&[("pattern", json!("needle")), ("context", json!(-2))]),
        &context,
    );
    let text = output_text(&ran);
    assert!(text.contains("one.txt:1:needle"), "{text}");
    assert!(
        text.contains("[context ignored: asked -2, not a non-negative integer; 0 used]"),
        "{text}"
    );
    Ok(())
}

/// A process-wide cache or a PATH lookup is tested in a rerun of one test whose PATH is one
/// scratch dir; the dir's name is how the rerun knows it is one.
fn fake_path(tag: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("PATH")?);
    let name = path.file_name()?.to_str()?;
    name.starts_with(&format!("yi-tools-{tag}-"))
        .then_some(path)
}

fn rerun_on_path(test: &str, dir: &std::path::Path) -> TestResult {
    let rerun = yi_tools::command(std::env::current_exe()?)
        .args(["--exact", test])
        .env("PATH", dir)
        .output()?;
    let stdout = String::from_utf8_lossy(&rerun.stdout);
    assert!(
        rerun.status.success() && stdout.contains("1 passed"),
        "{stdout}"
    );
    Ok(())
}

#[cfg(unix)]
fn script(path: &std::path::Path, body: &str) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, format!("#!/bin/sh\n{body}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// The syntax verdict and `grid check` read the same settled file, so an edit waits for the
/// slower of the two, not their sum: each fake takes 1 s.
#[cfg(unix)]
#[test]
fn an_edit_runs_the_syntax_check_beside_the_grid_check() -> TestResult {
    let Some(dir) = fake_path("edit-overlap") else {
        let dir = temp_dir("edit-overlap")?;
        let clean = grid_fixture("check-clean.json");
        script(
            &dir.join("grid"),
            &format!("/bin/sleep 1\n/bin/cat '{}'\n", clean.display()),
        )?;
        script(&dir.join("rustfmt"), "/bin/sleep 1\n")?;
        return rerun_on_path("an_edit_runs_the_syntax_check_beside_the_grid_check", &dir);
    };
    fs::create_dir_all(dir.join(".grid"))?;
    fs::write(dir.join("a.rs"), "fn a() {}\n")?;
    let context = ToolContext::new(dir.clone());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    yi_tools::hashline::tool::HashlineReadTool::new(Arc::clone(&state))
        .execute(args(&[("path", json!("a.rs"))]), &context);
    let started = std::time::Instant::now();
    let edit = yi_tools::hashline::tool::HashlineEditTool {
        state,
        freeform_grammar: false,
    }
    .execute(
        args(&[("patch", json!("[a.rs]\nPUT 1.=1:\n+fn b() {}\n"))]),
        &context,
    );
    let took = started.elapsed();
    let text = output_text(&edit);
    assert!(text.contains("syntax: ok"), "{text}");
    assert!(text.contains("[grid check: clean]"), "{text}");
    assert!(took < std::time::Duration::from_millis(1800), "{took:?}");
    Ok(())
}

#[cfg(unix)]
fn grid_fixture(name: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/grid")
        .join(name)
}

/// A fake `grid` that answers `check` with the `check.json` beside it (to start, a real
/// `grid check --quick --json` from a repo where `src/alpha.rs` and `src/beta.rs` both drifted),
/// `resolve src/alpha.rs` with grid's own answer and any other file with grid's own miss, and
/// leaves a `ran` marker beside itself.
#[cfg(unix)]
fn fake_grid(dir: &std::path::Path) -> TestResult {
    fs::copy(grid_fixture("check-quick.json"), dir.join("check.json"))?;
    script(
        &dir.join("grid"),
        &format!(
            "d=$(/usr/bin/dirname \"$0\")\n: > \"$d/ran\"\n\
             [ \"$1\" = check ] && {{ /bin/cat \"$d/check.json\"; exit 3; }}\n\
             [ \"$1 $2\" = 'resolve src/alpha.rs' ] && {{ /bin/cat '{}'; exit 0; }}\n\
             /bin/cat '{}' >&2; exit 2\n",
            grid_fixture("resolve-alpha.json").display(),
            grid_fixture("resolve-miss.txt").display()
        ),
    )
}

#[cfg(unix)]
fn edit_at(dir: &std::path::Path, path: &str, line: &str) -> yi_tools::ToolOutput {
    let context = ToolContext::new(dir.to_path_buf());
    let state = yi_tools::hashline::tool::shared_hashline_state();
    yi_tools::hashline::tool::HashlineReadTool::new(Arc::clone(&state))
        .execute(args(&[("path", json!(path))]), &context);
    yi_tools::hashline::tool::HashlineEditTool {
        state,
        freeform_grammar: false,
    }
    .execute(
        args(&[("patch", json!(format!("[{path}]\nPUT 1.=1:\n+{line}\n")))]),
        &context,
    )
}

/// Dogfood 2026-09-27: every edit, even to a scratch file, printed the same unrelated drift,
/// and in an uncharted tree `grid check` surveyed and wrote `.grid/` there.
#[cfg(unix)]
#[test]
fn an_edit_in_an_uncharted_tree_never_asks_grid() -> TestResult {
    let Some(dir) = fake_path("grid-uncharted") else {
        let dir = temp_dir("grid-uncharted")?;
        fake_grid(&dir)?;
        return rerun_on_path("an_edit_in_an_uncharted_tree_never_asks_grid", &dir);
    };
    fs::write(dir.join("a.rs"), "fn a() {}\n")?;
    let edit = edit_at(&dir, "a.rs", "pub fn b() {}");
    let text = output_text(&edit);
    assert!(!dir.join("ran").exists(), "grid was spawned: {text}");
    assert!(!text.contains("[grid check"), "{text}");
    assert_eq!(edit.result.details["grid"], json!("skipped"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_charted_edit_shows_only_the_drift_of_what_it_edited() -> TestResult {
    let Some(dir) = fake_path("grid-scoped") else {
        let dir = temp_dir("grid-scoped")?;
        fake_grid(&dir)?;
        return rerun_on_path(
            "a_charted_edit_shows_only_the_drift_of_what_it_edited",
            &dir,
        );
    };
    fs::create_dir_all(dir.join(".grid"))?;
    fs::create_dir_all(dir.join("src"))?;
    fs::write(
        dir.join("src/alpha.rs"),
        "pub fn scale(x: u8) -> u8 {\n    x\n}\n",
    )?;
    let edit = edit_at(&dir, "src/alpha.rs", "pub fn b() {}");
    let text = output_text(&edit);
    assert!(
        text.contains("drift drift.user.total <-calls- drift.alpha.scale at src/user.rs:1"),
        "{text}"
    );
    assert!(!text.contains("drift.beta.base"), "{text}");
    assert!(
        text.contains(
            "[grid check: 1 suspect outside the edited files — bash: grid check --quick]"
        ),
        "{text}"
    );
    assert_eq!(edit.result.details["grid"], json!("findings"));
    Ok(())
}

/// A deleted definition that is still called is in no file's `grid resolve`, and an edited
/// file left with no definitions makes `resolve` exit 2; the dependent still shows.
#[cfg(unix)]
#[test]
fn an_edit_that_deletes_a_called_definition_shows_its_dependents() -> TestResult {
    let Some(dir) = fake_path("grid-deleted") else {
        let dir = temp_dir("grid-deleted")?;
        fake_grid(&dir)?;
        return rerun_on_path(
            "an_edit_that_deletes_a_called_definition_shows_its_dependents",
            &dir,
        );
    };
    fs::create_dir_all(dir.join(".grid"))?;
    fs::create_dir_all(dir.join("src"))?;
    fs::copy(grid_fixture("check-deleted.json"), dir.join("check.json"))?;
    fs::write(dir.join("src/beta.rs"), "pub fn gone() -> u8 { 1 }\n")?;
    let edit = edit_at(&dir, "src/beta.rs", "// gone");
    let text = output_text(&edit);
    assert!(
        text.contains(
            "[grid check]\ndrift drift.user.total <-calls- drift.beta.gone at src/user.rs:1"
        ),
        "{text}"
    );
    assert_eq!(edit.result.details["grid"], json!("findings"));
    Ok(())
}

/// From a crate directory the chart at the repository's top answers; a nested repository with
/// no chart of its own never borrows it.
#[cfg(unix)]
#[test]
fn a_subdirectory_uses_the_chart_above_it_within_its_repository() -> TestResult {
    let Some(dir) = fake_path("grid-subdir") else {
        let dir = temp_dir("grid-subdir")?;
        fake_grid(&dir)?;
        return rerun_on_path(
            "a_subdirectory_uses_the_chart_above_it_within_its_repository",
            &dir,
        );
    };
    fs::create_dir_all(dir.join(".grid"))?;
    fs::create_dir_all(dir.join("nested/src"))?;
    fs::write(dir.join("nested/.git"), "gitdir: elsewhere\n")?;
    fs::write(dir.join("nested/src/alpha.rs"), "fn a() {}\n")?;
    let nested = edit_at(&dir.join("nested"), "src/alpha.rs", "pub fn b() {}");
    assert_eq!(nested.result.details["grid"], json!("skipped"));
    assert!(!dir.join("ran").exists(), "{}", output_text(&nested));
    fs::create_dir_all(dir.join("src"))?;
    fs::write(dir.join("src/alpha.rs"), "fn a() {}\n")?;
    let text = output_text(&edit_at(&dir.join("src"), "alpha.rs", "pub fn b() {}"));
    assert!(
        text.contains("drift drift.user.total <-calls- drift.alpha.scale at src/user.rs:1"),
        "{text}"
    );
    assert!(!dir.join("src/.grid").exists());
    Ok(())
}

/// A real drift runs about 600 bytes a suspect, past the 30 KB display capture at about 50; an
/// answer that is not grid's JSON, or has no `drift.suspects`, is never read as clean.
#[cfg(unix)]
#[test]
fn a_grid_answer_is_read_whole_or_named_unavailable() -> TestResult {
    let Some(dir) = fake_path("grid-answer") else {
        let dir = temp_dir("grid-answer")?;
        fake_grid(&dir)?;
        return rerun_on_path("a_grid_answer_is_read_whole_or_named_unavailable", &dir);
    };
    fs::create_dir_all(dir.join(".grid"))?;
    fs::create_dir_all(dir.join("src"))?;
    fs::write(dir.join("src/alpha.rs"), "fn a() {}\n")?;
    let mut report: Value = serde_json::from_slice(&fs::read(dir.join("check.json"))?)?;
    let suspects = report
        .pointer_mut("/drift/suspects")
        .and_then(Value::as_array_mut)
        .ok_or("no suspects")?;
    let other = suspects.get(1).cloned().ok_or("no second suspect")?;
    suspects.extend(std::iter::repeat_n(other, 79));
    let big = serde_json::to_string(&report)?;
    assert!(big.len() > 40_000, "{}", big.len());
    fs::write(dir.join("check.json"), big)?;
    let text = output_text(&edit_at(&dir, "src/alpha.rs", "pub fn b() {}"));
    assert!(
        text.contains("<-calls- drift.alpha.scale at src/user.rs:1"),
        "{text}"
    );
    assert!(
        text.contains("[grid check: 80 suspects outside the edited files"),
        "{text}"
    );
    for answer in ["grid: something this change did was not checked", "{}"] {
        fs::write(dir.join("check.json"), answer)?;
        fs::write(dir.join("src/alpha.rs"), "fn a() {}\n")?;
        let edit = edit_at(&dir, "src/alpha.rs", "pub fn b() {}");
        let text = output_text(&edit);
        assert!(
            text.contains("[grid check: unavailable — the answer was not grid's JSON]"),
            "{text}"
        );
        assert_eq!(edit.result.details["grid"], json!("unparsed"));
    }
    Ok(())
}

/// A rustup proxy re-resolved the toolchain on every syntax check; it is asked once per
/// process and the binary it names runs directly.
#[cfg(unix)]
#[test]
fn a_rustup_proxy_is_asked_once_and_its_binary_runs_directly() -> TestResult {
    let Some(dir) = fake_path("rustup-proxy") else {
        let dir = temp_dir("rustup-proxy")?;
        let (log, real) = (dir.join("log"), dir.join("toolchain"));
        fs::create_dir_all(&real)?;
        script(
            &dir.join("rustup"),
            &format!(
                "case \"$0\" in *rustfmt) echo proxy >> {log};; *) echo \"rustup $1\" >> {log}; echo {real}/rustfmt;; esac\n",
                log = log.display(),
                real = real.display(),
            ),
        )?;
        std::os::unix::fs::symlink("rustup", dir.join("rustfmt"))?;
        script(
            &real.join("rustfmt"),
            &format!("echo direct >> {}\n", log.display()),
        )?;
        rerun_on_path(
            "a_rustup_proxy_is_asked_once_and_its_binary_runs_directly",
            &dir,
        )?;
        assert_eq!(
            fs::read_to_string(&log)?,
            "rustup which\ndirect\ndirect\ndirect\n"
        );
        return Ok(());
    };
    let context = ToolContext::new(dir);
    for index in 0..3 {
        let written = WriteTool::default().execute(
            args(&[
                ("path", json!("a.rs")),
                ("content", json!(format!("fn f{index}() {{}}\n"))),
            ]),
            &context,
        );
        let text = output_text(&written);
        assert!(text.contains("syntax: ok"), "{text}");
    }
    Ok(())
}

/// #906: yi's provider keys reached every uncontained bash call (yolo, an approved run outside
/// the sandbox, any host with no sandbox), where `printenv` passes as read-only.
#[test]
fn bash_inherits_no_provider_key() -> TestResult {
    let keys = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "GEMINI_API_KEY",
        "LAYA_API_KEY",
    ];
    for name in keys.iter().chain(&["YI_906_ORDINARY"]) {
        // SAFETY: nextest runs each test in a process of its own, so no thread reads the env.
        unsafe { std::env::set_var(name, format!("ENV-906-{name}")) };
    }
    let dir = temp_dir("bash-keys")?;
    let context = ToolContext::new(dir.to_path_buf());
    let shown = BashTool::default().execute(args(&[("command", json!("env"))]), &context);
    let text = output_text(&shown);
    let leaked: Vec<&str> = (keys.into_iter())
        .filter(|name| text.contains(&format!("ENV-906-{name}")))
        .collect();
    assert!(
        !shown.is_error && text.contains("ENV-906-YI_906_ORDINARY") && leaked.is_empty(),
        "bash `env` saw {leaked:?} or lost an ordinary variable"
    );
    Ok(())
}

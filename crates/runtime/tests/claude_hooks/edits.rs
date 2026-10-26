use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Case, Gate, Outcome, TestResult, command, logging, printing, read_lines, settings};

/// Reads each file the patch names, as the model must before an edit, then edits.
async fn edited(case: &Case, patch: &str) -> Result<Outcome, Box<dyn std::error::Error>> {
    let mut calls: Vec<(&str, Value)> = Vec::new();
    for line in patch.lines().filter(|line| line.starts_with('[')) {
        let name = line
            .trim_start_matches('[')
            .split('#')
            .next()
            .unwrap_or_default();
        calls.push(("read", json!({"path": name})));
    }
    calls.push(("edit", json!({"patch": patch})));
    let mut outcomes = case.run_calls(calls, Gate::default(), "x").await?;
    outcomes.pop().ok_or_else(|| "no edit result".into())
}

/// A patch replacing one line in each file, anchored to the file's current hash.
fn replacing(case: &Case, files: &[(&str, &str, &str)]) -> Result<String, std::io::Error> {
    let mut patch = String::new();
    for (name, text, line) in files {
        std::fs::write(case.cwd.join(name), text)?;
        let tag = yi_tools::hashline::format::compute_file_hash(text);
        patch.push_str(&format!("[{name}#{tag}]\nPUT 2.=2:\n+{line}\n"));
    }
    Ok(patch)
}

#[tokio::test]
async fn an_edit_reaches_its_hooks_as_claude_codes_edit_input_per_file() -> TestResult {
    let case = Case::new("edit-shape", json!({}), true)?;
    let log = case.path("edit.jsonl");
    case.user(&settings("PreToolUse", "Edit", vec![logging(&log)]))?;
    let files = [
        ("a.txt", "one\ntwo\nthree\n", "deux"),
        ("c.txt", "x\ny\n", "why"),
    ];
    let patch = replacing(&case, &files)?;
    let outcome = edited(&case, &patch).await?;
    assert!(!outcome.is_error, "{}", outcome.text);
    assert_eq!(std::fs::read_to_string(case.cwd.join("c.txt"))?, "x\nwhy\n");
    let abs = |name: &str| case.cwd.join(name).to_string_lossy().into_owned();
    let mut seen: Vec<Value> = read_lines(&log)?
        .into_iter()
        .map(|line| line["tool_input"].clone())
        .collect();
    seen.sort_by_key(|input| input["file_path"].as_str().map(str::to_owned));
    let expected = [
        json!({"file_path": abs("a.txt"), "old_string": "two\n", "new_string": "deux\n", "replace_all": false}),
        json!({"file_path": abs("c.txt"), "old_string": "y\n", "new_string": "why\n", "replace_all": false}),
    ];
    assert_eq!(
        seen, expected,
        "a file's hook saw another file's patch, or no new text"
    );
    std::fs::remove_file(&log)?;
    let one = replacing(&case, &[("a.txt", "one\ntwo\n", "zwei")])?;
    edited(&case, &one).await?;
    let lines = read_lines(&log)?;
    let input = &lines.first().ok_or("no single-file input")?["tool_input"];
    assert_eq!(input["new_string"], "zwei\n");
    assert_eq!(
        input["patch"],
        one.as_str(),
        "a one-file edit lost its patch"
    );
    Ok(())
}

#[tokio::test]
async fn a_multi_file_edits_hooks_run_at_once_and_its_rows_are_capped_together() -> TestResult {
    let mut slow = command("sleep 2");
    slow["timeout"] = Value::from(5);
    let context = printing(&json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "additionalContext": "k".repeat(6_000)
    }}));
    let case = Case::new(
        "edit-fanout",
        settings("PreToolUse", "Edit", vec![slow, context]),
        true,
    )?;
    let one = replacing(&case, &[("solo.txt", "a\nb\n", "c")])?;
    let alone = edited(&case, &one).await?;
    assert!(!alone.is_error, "{}", alone.text);
    assert!(
        alone.text.contains(&"k".repeat(6_000)),
        "one file's row was cut"
    );
    let files = [
        ("a.txt", "a\nb\n", "c"),
        ("b.txt", "a\nb\n", "c"),
        ("c.txt", "a\nb\n", "c"),
    ];
    let patch = replacing(&case, &files)?;
    let started = Instant::now();
    let outcome = edited(&case, &patch).await?;
    let elapsed = started.elapsed();
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(
        elapsed < Duration::from_secs(5),
        "files ran one after another: {elapsed:?}"
    );
    let row = "of the hook's rows across the 3 files of this call (HOOK_TEXT_CAP 10000)";
    assert!(
        outcome.text.contains(row),
        "{}",
        outcome.text.chars().take(300).collect::<String>()
    );
    assert!(
        outcome.text.chars().count() < 10_000,
        "the rows were not capped together"
    );
    Ok(())
}

#[tokio::test]
async fn edit_preparation_waits_for_a_matching_pre_hook() -> TestResult {
    let case = Case::new(
        "edit-prep",
        settings("PreToolUse", "Bash", vec![command("printf ok")]),
        true,
    )?;
    super::EDIT_CHANGES.store(0, std::sync::atomic::Ordering::Relaxed);
    let gate = Gate {
        count_edit_changes: true,
        ..Gate::default()
    };
    case.run_gated("edit", json!({"patch": "[a#ZZZZ]"}), gate)
        .await?;
    assert_eq!(
        super::EDIT_CHANGES.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_post_edit_hook_sees_the_span_after_a_pre_rewrite() -> TestResult {
    let case = Case::new("edit-rewrite-post", json!({}), true)?;
    std::fs::write(case.cwd.join("a.txt"), "one\ntwo\n")?;
    let tag = yi_tools::hashline::format::compute_file_hash("one\ntwo\n");
    let before = format!("[a.txt#{tag}]\nPUT 2.=2:\n+deux\n");
    let after = format!("[a.txt#{tag}]\nPUT 2.=2:\n+trois\n");
    let log = case.path("post-edit.jsonl");
    let updated = printing(&json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "updatedInput": {"patch": after}
    }}));
    case.user(&json!({"hooks": {
        "PreToolUse": [{"matcher": "Edit", "hooks": [updated]}],
        "PostToolUse": [{"matcher": "Edit", "hooks": [logging(&log)]}],
    }}))?;
    let outcome = edited(&case, &before).await?;
    assert!(!outcome.is_error, "{}", outcome.text);
    assert_eq!(
        std::fs::read_to_string(case.cwd.join("a.txt"))?,
        "one\ntrois\n"
    );
    let lines = read_lines(&log)?;
    let input = lines.last().ok_or("no post edit input")?;
    assert_eq!(input["tool_input"]["new_string"], "trois\n");
    Ok(())
}

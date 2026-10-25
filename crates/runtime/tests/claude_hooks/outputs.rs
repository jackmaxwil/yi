use std::time::Duration;

use serde_json::{Value, json};
use yi_runtime::PermissionMode;

use super::{Case, Gate, TestResult, asking, command, decision, printing, settings};

fn pre(output: &Value) -> Value {
    printing(&json!({"hookSpecificOutput": output}))
}

/// The text before a cut row, and the file the row names, read whole.
fn spilled(text: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (head, row) = text.split_once("\n[… kept ").ok_or("no cut row")?;
    let path = row.split_once("read ").ok_or("no read call")?.1;
    let path = path.split_once(']').ok_or("unclosed row")?.0;
    Ok((head.to_owned(), std::fs::read_to_string(path)?))
}

#[tokio::test]
async fn a_defer_is_ignored_and_the_call_goes_on_to_the_broker() -> TestResult {
    let defer = pre(&json!({"permissionDecision": "defer", "additionalContext": "dropped ctx"}));
    let case = Case::new("defer", settings("PreToolUse", "Bash", vec![defer]), true)?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case
        .run_gated("bash", touch.clone(), asking(PermissionMode::Yolo))
        .await?;
    assert!(
        marker.exists(),
        "a defer refused the call: {}",
        outcome.text
    );
    assert!(
        outcome.text.contains("Yi ignores a defer"),
        "{}",
        outcome.text
    );
    assert!(!outcome.text.contains("dropped ctx"), "{}", outcome.text);
    std::fs::remove_file(&marker)?;
    let both = vec![
        pre(&json!({"permissionDecision": "defer"})),
        decision("deny"),
    ];
    case.user(&settings("PreToolUse", "Bash", both))?;
    let denied = case
        .run_gated("bash", touch, asking(PermissionMode::Yolo))
        .await?;
    assert!(
        !marker.exists(),
        "a defer outranked a deny: {}",
        denied.text
    );
    Ok(())
}

#[tokio::test]
async fn a_post_hook_exiting_two_with_a_json_block_shows_the_json_reason() -> TestResult {
    let block = json!({"decision": "block", "reason": "tests failed in foo.rs"});
    let hook = command(&format!("printf '%s' '{block}'; echo stderr-x >&2; exit 2"));
    let case = Case::new(
        "post-exit-json",
        settings("PostToolUse", "Bash", vec![hook]),
        true,
    )?;
    let outcome = case.run("bash", json!({"command": "printf body"})).await?;
    let row = "[Claude Code hook feedback: tests failed in foo.rs]";
    assert!(outcome.text.contains(row), "{}", outcome.text);
    assert!(!outcome.text.contains("stderr-x"), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn additional_context_is_whole_at_the_cap_and_spilled_with_a_preview_past_it() -> TestResult {
    let cases = [
        ("at", "c".repeat(10_000), None),
        (
            "past",
            "c".repeat(10_001),
            Some("kept 2000 of 10001 characters"),
        ),
        (
            "wide",
            "é".repeat(10_001),
            Some("kept 2000 of 10001 characters"),
        ),
    ];
    for (label, context, cut) in cases {
        let hook = printing(&json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse", "additionalContext": context
        }}));
        let case = Case::new(
            &format!("context-cap-{label}"),
            settings("PostToolUse", "Bash", vec![hook]),
            true,
        )?;
        let outcome = case.run("bash", json!({"command": "printf body"})).await?;
        let row = outcome.text.split_once("[Claude Code hook context: ");
        let row = row.ok_or("no context row")?.1;
        match cut {
            None => assert_eq!(row, format!("{context}]"), "{label}"),
            Some(kept) => {
                assert!(row.contains(kept), "{label}: {row}");
                assert!(row.contains("(HOOK_TEXT_CAP 10000)"), "{label}: {row}");
                let (head, whole) = spilled(row)?;
                assert_eq!(head, context.chars().take(2_000).collect::<String>());
                assert_eq!(whole, context, "{label}: the file is not the whole text");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_updated_tool_output_past_the_cap_is_cut_loud() -> TestResult {
    for (label, size, cut) in [("at", 10_000, false), ("past", 10_001, true)] {
        let replaced = "w".repeat(size);
        let hook = printing(&json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse", "updatedToolOutput": replaced
        }}));
        let case = Case::new(
            &format!("replaced-{label}"),
            settings("PostToolUse", "mcp__yi__stub", vec![hook]),
            true,
        )?;
        let outcome = case.run("stub", json!({})).await?;
        if !cut {
            assert_eq!(outcome.text, replaced, "{label}");
            continue;
        }
        let row = "of 10001 characters of the hook's updatedToolOutput (HOOK_TEXT_CAP 10000)";
        assert!(outcome.text.contains(row), "{}", outcome.text);
        let (head, whole) = spilled(&outcome.text)?;
        assert_eq!(head.len(), 2_000);
        assert_eq!(whole, replaced);
    }
    Ok(())
}

#[tokio::test]
async fn a_noisy_deny_hook_still_denies_past_the_stderr_cap() -> TestResult {
    let deny = json!({"hookSpecificOutput": {"permissionDecision": "deny", "permissionDecisionReason": "zz denied"}});
    let hook = command(&format!(
        "head -c 1500000 /dev/zero | tr '\\0' x >&2; printf '%s' '{deny}'"
    ));
    let case = Case::new(
        "noisy-deny",
        settings("PreToolUse", "Bash", vec![hook]),
        true,
    )?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run("bash", touch).await?;
    assert!(
        !marker.exists(),
        "a cut stderr dropped the deny: {}",
        outcome.text
    );
    assert!(outcome.text.contains("zz denied"), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn stdout_at_the_output_cap_is_read_and_past_it_is_named() -> TestResult {
    let open = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":""#;
    for (label, total, read) in [("at", 1_048_576, true), ("past", 1_048_577, false)] {
        let pad = total - open.len() - 3;
        let hook = command(&format!(
            "printf '%s' '{open}'; head -c {pad} /dev/zero | tr '\\0' y; printf '\"}}}}'"
        ));
        let case = Case::new(
            &format!("stdout-cap-{label}"),
            settings("PreToolUse", "Bash", vec![hook]),
            true,
        )?;
        let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
        assert!(!outcome.is_error, "{label}: {}", outcome.text);
        let named = "its stdout passed HOOK_OUTPUT_CAP (1048576 bytes per stream)";
        assert_eq!(
            outcome.text.contains("[Claude Code hook context: yyy"),
            read,
            "{label}: {}",
            outcome.text.chars().take(400).collect::<String>()
        );
        assert_eq!(outcome.text.contains(named), !read, "{label}");
    }
    Ok(())
}

#[tokio::test]
async fn exit_two_past_the_output_cap_names_the_cap_and_the_bytes_written() -> TestResult {
    let hook = command("head -c 3000000 /dev/zero | tr '\\0' x >&2; exit 2");
    let case = Case::new(
        "stderr-flood",
        settings("PreToolUse", "Bash", vec![hook]),
        true,
    )?;
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(outcome.is_error, "{}", outcome.text);
    let named = "of 3000000 bytes of the hook's output (HOOK_OUTPUT_CAP 1048576 per stream)";
    assert!(outcome.text.contains(named), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn continue_false_stops_the_run_from_a_pre_and_from_a_post_hook() -> TestResult {
    let stop = |reason: &str| printing(&json!({"continue": false, "stopReason": reason}));
    let case = Case::new(
        "continue",
        settings("PreToolUse", "Bash", vec![stop("halt pre")]),
        true,
    )?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run("bash", touch.clone()).await?;
    assert!(!marker.exists(), "a pre stop ran the call");
    assert!(
        outcome.text.contains("the run stops: halt pre"),
        "{}",
        outcome.text
    );
    case.user(&settings("PostToolUse", "Bash", vec![stop("halt post")]))?;
    let outcome = case.run("bash", touch).await?;
    assert!(marker.exists(), "{}", outcome.text);
    let row = "[Claude Code hook stopped the run: halt post]";
    assert!(outcome.text.contains(row), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn on_failure_block_turns_a_failed_hook_into_a_deny() -> TestResult {
    let mut missing = json!({"type": "command", "command": "/definitely/missing/hook", "args": []});
    missing["onFailure"] = Value::from("block");
    let case = Case::new(
        "on-failure",
        settings("PreToolUse", "Bash", vec![missing]),
        true,
    )?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run("bash", touch).await?;
    assert!(!marker.exists(), "{}", outcome.text);
    let why = "blocking because onFailure is \"block\"";
    assert!(outcome.text.contains(why), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn deprecated_approve_and_block_map_to_allow_and_deny() -> TestResult {
    let old = |decision: &str| printing(&json!({"decision": decision, "reason": "old reason"}));
    let case = Case::new(
        "deprecated",
        settings("PreToolUse", "Bash", vec![old("approve")]),
        true,
    )?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let gate = || asking(PermissionMode::Ask);
    let outcome = case.run_gated("bash", touch.clone(), gate()).await?;
    assert!(
        marker.exists(),
        "approve was not an allow: {}",
        outcome.text
    );
    std::fs::remove_file(&marker)?;
    case.user(&settings("PreToolUse", "Bash", vec![old("block")]))?;
    let outcome = case.run_gated("bash", touch, gate()).await?;
    assert!(!marker.exists(), "block was not a deny");
    assert!(outcome.text.contains("old reason"), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn async_hooks_never_decide_and_only_async_rewake_is_timed() -> TestResult {
    let case = Case::new("async", json!({}), true)?;
    let (plain, rewake) = (case.cwd.join("async-ran"), case.cwd.join("rewake-ran"));
    let late = |marker: &std::path::Path, flag: &str| {
        let mut hook = command(&format!("sleep 2; touch '{}'; exit 2", marker.display()));
        hook[flag] = Value::Bool(true);
        hook["timeout"] = Value::from(1);
        hook
    };
    let hooks = vec![late(&plain, "async"), late(&rewake, "asyncRewake")];
    case.user(&settings("PreToolUse", "Bash", hooks))?;
    let outcome = case.run("bash", json!({"command": "printf body"})).await?;
    assert!(
        outcome.text.starts_with("body"),
        "a background hook decided: {}",
        outcome.text
    );
    std::thread::sleep(Duration::from_millis(3_500));
    assert!(plain.exists(), "an async hook was held to its timeout");
    assert!(!rewake.exists(), "an asyncRewake hook outlived its timeout");
    Ok(())
}

#[tokio::test]
async fn invalid_json_and_a_failing_exit_are_notices_not_blocks() -> TestResult {
    let hooks = vec![
        command("printf '{not json}'"),
        command("echo oops >&2; exit 3"),
    ];
    let case = Case::new("notices", settings("PreToolUse", "Bash", hooks), true)?;
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run_gated("bash", touch, Gate::default()).await?;
    assert!(marker.exists(), "a failed hook blocked: {}", outcome.text);
    assert!(
        outcome.text.contains("invalid JSON output"),
        "{}",
        outcome.text
    );
    let failed = "Failed with non-blocking status code (exit 3): oops";
    assert!(outcome.text.contains(failed), "{}", outcome.text);
    Ok(())
}

use std::sync::{Arc, Mutex};

use serde_json::json;
use yi_runtime::PermissionMode;
use yi_types::permission::{Answerer, PermissionRecord};

use super::{Case, Gate, TestResult, asking, decision, settings};

fn hold(pattern: &str) -> Result<yi_permission::Hold, Box<dyn std::error::Error>> {
    Ok(yi_permission::Hold {
        pattern: yi_permission::HoldPattern::new(pattern).ok_or("empty hold")?,
        reason: "advisor holds printf".to_owned(),
        source: yi_permission::HoldSource::Advisor,
        expires_at_ms: None,
    })
}

#[tokio::test]
async fn a_hook_allow_never_answers_a_rule_a_hold_or_a_credential_read() -> TestResult {
    let allow = settings("PreToolUse", "Bash", vec![decision("allow")]);
    let ask = yi_runtime::ConfigRuleAction::Ask;
    let cases = [
        (
            "rule",
            Gate {
                rules: vec![yi_runtime::ConfigRule::new("bash", "*printf*", ask)?],
                ..asking(PermissionMode::Auto)
            },
            "a configured rule requires confirmation",
        ),
        (
            "hold",
            Gate {
                holds: vec![hold("printf")?],
                ..asking(PermissionMode::Auto)
            },
            "advisor holds printf",
        ),
        (
            "credential",
            asking(PermissionMode::Auto),
            "reads a credential store",
        ),
    ];
    for (label, gate, why) in cases {
        let case = Case::new(&format!("allow-{label}"), allow.clone(), true)?;
        let command = match label {
            "credential" => format!("cat {}/.aws/credentials; printf ran", case.home.display()),
            _ => "printf ran".to_owned(),
        };
        let outcome = case
            .run_gated("bash", json!({"command": command}), gate)
            .await?;
        assert!(
            outcome.is_error,
            "{label}: a hook allow lifted it: {}",
            outcome.text
        );
        assert!(outcome.text.contains(why), "{label}: {}", outcome.text);
    }
    Ok(())
}

#[tokio::test]
async fn a_hook_allow_that_answers_the_modes_ask_is_journaled_as_the_hooks() -> TestResult {
    let allow = settings("PreToolUse", "Bash", vec![decision("allow")]);
    let case = Case::new("allow-journal", allow, true)?;
    let records: Arc<Mutex<Vec<PermissionRecord>>> = Arc::default();
    let sink = Arc::clone(&records);
    let gate = Gate {
        journal: Some(Arc::new(move |record| {
            if let Ok(mut records) = sink.lock() {
                records.push(record);
            }
        })),
        ..asking(PermissionMode::Ask)
    };
    let marker = case.cwd.join("ran");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run_gated("bash", touch, gate).await?;
    assert!(marker.exists(), "{}", outcome.text);
    let records = records.lock().map_err(|_| "poisoned")?;
    let [record] = records.as_slice() else {
        return Err(format!("an ask answered by a hook left {} records", records.len()).into());
    };
    assert!(record.allowed, "{record:?}");
    assert_eq!(record.by, Answerer::Other("claudeCodeHook".to_owned()));
    Ok(())
}

#[tokio::test]
async fn a_write_under_the_projects_claude_dir_is_asked_and_refused_headless() -> TestResult {
    let allow = settings("PreToolUse", "Write", vec![decision("allow")]);
    let case = Case::new("claude-dir", allow, true)?;
    let script = ".claude/hooks/check.sh";
    let args = json!({"path": script, "content": "touch escaped"});
    let outcome = case
        .run_gated("write", args, asking(PermissionMode::Auto))
        .await?;
    assert!(
        !case.cwd.join(script).exists(),
        "the model wrote a hook script: {}",
        outcome.text
    );
    assert!(
        outcome
            .text
            .contains("whose files Claude Code hooks run on the host"),
        "{}",
        outcome.text
    );
    let args = json!({"path": "src/ok.txt", "content": "x"});
    let inside = case
        .run_gated("write", args, asking(PermissionMode::Auto))
        .await?;
    assert!(case.cwd.join("src/ok.txt").exists(), "{}", inside.text);
    Ok(())
}

#[tokio::test]
async fn contained_bash_cannot_write_the_projects_claude_dir() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let case = Case::new("claude-dir-bash", json!({}), true)?;
    let gate = Gate {
        sandbox: true,
        ..asking(PermissionMode::Auto)
    };
    let command = "mkdir -p .claude/hooks && printf x > .claude/hooks/check.sh";
    let outcome = case
        .run_gated("bash", json!({"command": command}), gate)
        .await?;
    assert!(
        !case.cwd.join(".claude/hooks/check.sh").exists(),
        "contained bash wrote a hook script: {}",
        outcome.text
    );
    Ok(())
}

#[test]
fn a_goal_check_passes_the_bash_hooks_before_it_runs() -> TestResult {
    let refusal = super::command("echo 'checks may not touch' >&2; exit 2");
    let case = Case::new(
        "goal-check",
        settings("PreToolUse", "Bash", vec![refusal]),
        true,
    )?;
    let store: yi_session::SharedSession = Arc::new(Mutex::new(
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "goal-hook-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        }),
    ));
    let service = yi_runtime::goal::GoalService::new(
        Arc::new(move || Some(store.clone())),
        Arc::new(|_, _: yi_types::schedule::DeliveryMode| {}),
        case.cwd.clone(),
    );
    let marker = case.cwd.join("checked");
    let check = format!("touch {}", marker.display());
    service.create("checked", None, Some(check), None)?;
    let error = service.update("complete").err().ok_or("must reject")?;
    assert!(error.contains("checks may not touch"), "{error}");
    assert!(!marker.exists(), "a refused goal check ran");
    Ok(())
}

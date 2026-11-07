use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_runtime::permission::{CallOutcome, PermissionBroker};
use yi_runtime::{PermissionMode, Wall};
use yi_tools::ToolKind;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn scoped(project: &Path) -> Result<PermissionBroker, Box<dyn Error>> {
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        project.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    broker.grant_scope("evals/**")?;
    Ok(broker)
}

fn decide(broker: &PermissionBroker, tool: &str, args: &Value) -> CallOutcome {
    let kind = match tool {
        "bash" => ToolKind::Exec,
        "grep" => ToolKind::Read,
        _ => ToolKind::Write,
    };
    let args = args.as_object().cloned().unwrap_or_default();
    broker.decide_call(tool, kind, true, "c1", &args, None)
}

/// Dies with the scope judging `write` alone: a fix pass under a brief for `evals/` edited
/// `crates/runtime` with `edit` and `MV`, a shell redirect and `grep apply` write as well. A child
/// of the run is held to it too, a walled one included, or `rlm.run` would be the way around.
#[test]
fn the_write_tools_and_a_redirect_meet_the_scope_and_so_does_every_child() -> TestResult {
    let scratch = Scratch::new("yi-write-scope")?;
    let project = scratch.join("project");
    std::fs::create_dir_all(&project)?;
    let walled = Wall {
        deny_read: vec![project.join("secrets")],
        ..Wall::default()
    };
    let parent = scoped(&project)?;
    let (child, walled_child) = (
        parent.for_child(&Wall::default(), &project),
        parent.for_child(&walled, &project),
    );
    let lock = "crates/runtime/src/lock.rs";
    let refused = [
        ("write", json!({"path": lock, "content": "x"}), lock),
        (
            "edit",
            json!({"patch": format!("[{lock}#a1]\nPUT 1.=1:\nx")}),
            lock,
        ),
        (
            "edit",
            json!({"patch": "[evals/notes.md]\nMV crates/notes.md"}),
            "crates/notes.md",
        ),
        (
            "bash",
            json!({"command": "cargo test 2>&1 | tee log > crates/test.log"}),
            "crates/test.log",
        ),
        (
            "grep",
            json!({"pattern": "a", "replace": "b", "apply": true}),
            "the working directory",
        ),
    ];
    let allowed = [
        (
            "write",
            json!({"path": "evals/sweep/probe.py", "content": "x"}),
        ),
        (
            "edit",
            json!({"patch": "[evals/notes.md#a1]\nPUT 1.=1:\nx"}),
        ),
        (
            "bash",
            json!({"command": "cargo test 2>&1 | tail -5 > evals/test.log"}),
        ),
        (
            "grep",
            json!({"pattern": "a", "replace": "b", "apply": true, "path": "evals"}),
        ),
    ];
    for (name, broker) in [
        ("parent", &parent),
        ("child", &child),
        ("walled child", &walled_child),
    ] {
        for (tool, args, shown) in &refused {
            let outcome = decide(broker, tool, args);
            assert!(!outcome.allowed, "{name}: {tool} {args} was allowed");
            let said = &outcome.reason;
            assert!(
                said.contains(shown) && said.contains("evals/**"),
                "{name}: {said}"
            );
        }
        for (tool, args) in &allowed {
            let outcome = decide(broker, tool, args);
            assert!(outcome.allowed, "{name}: {tool} {args}: {}", outcome.reason);
        }
    }
    Ok(())
}

/// Dies with a scope journaled like an "always": `--continue`, or a console resuming the session,
/// would reopen confined to a brief that run alone was given, with no way to lift it.
#[test]
fn a_scope_is_listed_but_never_written_to_the_session_ledger() -> TestResult {
    let scratch = Scratch::new("yi-write-scope-ledger")?;
    let kept = Arc::new(Mutex::new(Vec::new()));
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        scratch.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    let sink = Arc::clone(&kept);
    broker.set_rule_journal(Arc::new(move |rule| {
        if let Ok(mut kept) = sink.lock() {
            kept.push(rule);
        }
    }));
    broker.grant_scope("evals/**")?;
    assert!(kept.lock().map_err(|_| "poisoned")?.is_empty());
    assert_eq!(broker.kept_rules(), ["writes only under evals/**"]);
    assert!(broker.grant_scope("/work/evals/**").is_err());
    Ok(())
}

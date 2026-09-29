use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_runtime::permission::{Containment, PermissionBroker};
use yi_runtime::{AskOutcome, Asker, PermissionMode};
use yi_tools::ToolKind;

type TestResult = Result<(), Box<dyn Error>>;

fn write(path: &str, content: &str) -> Map<String, Value> {
    let mut args = Map::new();
    args.insert("path".to_owned(), json!(path));
    args.insert("content".to_owned(), json!(content));
    args
}

/// Incident: "Always allow" keyed its rule on the full arguments, so the next edit to a
/// sibling file carried different content, missed the rule and asked again.
#[test]
fn always_allow_on_an_edit_does_not_ask_for_the_next_edit_in_that_dir() -> TestResult {
    let asks = Arc::new(Mutex::new(0_usize));
    let offered = Arc::new(Mutex::new(Vec::new()));
    let (counted, labels) = (Arc::clone(&asks), Arc::clone(&offered));
    let asker: Asker = Arc::new(move |ask| {
        if let Ok(mut count) = counted.lock() {
            *count += 1;
        }
        if let Ok(mut labels) = labels.lock() {
            labels.extend(ask.grants.iter().map(|grant| grant.label.clone()));
        }
        AskOutcome::AllowAlways(0)
    });
    let broker = PermissionBroker::new(
        PermissionMode::Ask,
        PathBuf::from("/home/user/project"),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    );
    let decide = |id: &str, path: &str, content: &str| {
        // The write tool reports itself irreversible; a turn checkpoint is what undoes it.
        broker.decide_call(
            "write",
            ToolKind::Write,
            true,
            id,
            &write(path, content),
            None,
        )
    };
    assert!(decide("c1", "crates/tui/src/app.rs", "one").allowed);
    assert!(decide("c2", "crates/tui/src/render.rs", "two").allowed);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        1,
        "the second edit in the granted directory asked"
    );
    assert_eq!(
        offered.lock().map_err(|_| "poisoned")?.as_slice(),
        ["edits under crates/tui/src", "edits anywhere in this tree"]
    );
    assert!(decide("c3", "crates/cli/src/main.rs", "three").allowed);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        2,
        "an edit outside the granted directory must ask"
    );
    Ok(())
}

/// #600: an approved network command ran uncontained while the question never said so; the
/// sandbox has no network, so the approval escalates and the question names that.
#[test]
fn an_approved_network_ask_says_it_leaves_the_sandbox() -> TestResult {
    let asked = Arc::new(Mutex::new(String::new()));
    let seen = Arc::clone(&asked);
    let asker: Asker = Arc::new(move |ask| {
        if let Ok(mut text) = seen.lock() {
            *text = ask.text();
        }
        AskOutcome::AllowOnce
    });
    let project = PathBuf::from("/home/user/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    }));
    let mut args = Map::new();
    args.insert(
        "command".to_owned(),
        json!("curl -o out.json https://example.invalid/data"),
    );
    let outcome = broker.decide_call("bash", ToolKind::Exec, true, "c1", &args, None);
    assert_eq!(outcome.containment, Containment::Uncontained);
    let text = asked.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        text.contains("outside the sandbox (network)"),
        "the question says the approval leaves the sandbox: {text}"
    );
    Ok(())
}

/// A refusal at `~/x` would widen by the whole home directory, and one under `~/.yi` by the
/// store the host runs commands from; both leave the sandbox for one call, and say so.
#[test]
fn a_retry_is_widened_by_its_dir_but_never_by_home_or_yi_state() -> TestResult {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let asked = Arc::new(Mutex::new(String::new()));
    let seen = Arc::clone(&asked);
    let asker: Asker = Arc::new(move |ask| {
        if let Ok(mut text) = seen.lock() {
            *text = ask.text();
        }
        AskOutcome::AllowOnce
    });
    let project = PathBuf::from("/nonexistent/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    }));
    let lane = home.join("yi-a2-nonexistent-lane");
    for (refused, widened) in [
        (home.join("probe"), None),
        (home.join(".yi/mcp/sessions.json"), None),
        (lane.join("x"), Some(lane.clone())),
    ] {
        broker.note_containment_failure(yi_tools::SandboxRefusal::Path(refused.clone()));
        let mut args = Map::new();
        args.insert(
            "command".to_owned(),
            json!(format!("touch {}; true", refused.display())),
        );
        let outcome = broker.decide_call("bash", ToolKind::Exec, true, "c", &args, None);
        let text = asked.lock().map_err(|_| "poisoned")?.clone();
        match widened {
            Some(dir) => {
                assert_eq!(
                    outcome.containment,
                    Containment::Contained { widen: vec![dir] }
                );
                assert!(text.contains("approving widens this run by"), "{text}");
            }
            None => {
                assert_eq!(outcome.containment, Containment::Uncontained, "{text}");
                assert!(text.contains("is protected"), "{text}");
            }
        }
    }
    Ok(())
}

/// "Always" on a widened retry keeps the directory for later contained runs, not a rule that
/// would run the command itself outside the sandbox.
#[test]
fn always_on_a_widened_retry_keeps_the_dir_and_the_sandbox() -> TestResult {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let asks = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asks);
    let asker: Asker = Arc::new(move |ask| {
        if let Ok(mut labels) = seen.lock() {
            labels.extend(ask.grants.iter().map(|grant| grant.label.clone()));
        }
        AskOutcome::AllowAlways(0)
    });
    let project = PathBuf::from("/nonexistent/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    }));
    let lane = home.join("yi-a2-nonexistent-kept");
    broker.note_containment_failure(yi_tools::SandboxRefusal::Path(lane.join("x")));
    let touch = |id: &str, name: &str| {
        let mut args = Map::new();
        let command = format!("touch {}; true", lane.join(name).display());
        args.insert("command".to_owned(), json!(command));
        broker.decide_call("bash", ToolKind::Exec, true, id, &args, None)
    };
    let widened = Containment::Contained {
        widen: vec![lane.clone()],
    };
    assert_eq!(touch("c1", "x").containment, widened);
    assert_eq!(
        touch("c2", "y").containment,
        widened,
        "the kept dir widens with no question"
    );
    let offered = asks.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(
        offered,
        [format!("contained writes under {}", lane.display())]
    );
    Ok(())
}

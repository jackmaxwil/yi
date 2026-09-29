use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_runtime::permission::{Containment, PermissionBroker};
use yi_runtime::{AskOutcome, Asker, PermissionMode};
use yi_tools::ToolKind;

use crate::scratch::Scratch;

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

/// A glob walks from its literal head: `~/.ss?/*` walks the home and `/*/x*` the whole disk,
/// though no spelling of them names a key store. The gate judges the head the walk starts at.
#[test]
fn a_glob_is_judged_by_the_directory_its_walk_starts_from() {
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        PathBuf::from("/home/user/project"),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    for pattern in ["~/.ss?/*", "~/*/id_*", "/Users/*/.ssh/*", "/*/x*"] {
        let mut args = Map::new();
        args.insert("path".to_owned(), json!(pattern));
        let outcome = broker.decide_call("read", ToolKind::Read, false, "c1", &args, None);
        assert!(!outcome.allowed, "read {pattern} walks past the read gate");
    }
}

/// A named read or grep `path=` is judged by the file it opens, not the spelling (#898), and
/// yi's own token stores are key stores (#887). Every spelling below reached a key before.
#[cfg(unix)]
#[test]
fn a_named_read_is_judged_by_the_file_it_opens() -> TestResult {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("yi-scope-identity")?;
    let (home, elsewhere) = (scratch.join("home"), scratch.join("elsewhere"));
    for store in [".ssh", ".yi/providers/tokens", ".yi/mcp/tokens"] {
        std::fs::create_dir_all(home.join(store))?;
    }
    std::fs::create_dir_all(&elsewhere)?;
    std::fs::write(home.join(".ssh/id_rsa"), "FAKE PRIVATE KEY\n")?;
    std::fs::write(home.join(".yi/providers/tokens/openai.json"), "{}\n")?;
    std::fs::write(home.join(".yi/mcp/tokens/default_mcp.json"), "{}\n")?;
    std::fs::write(home.join("notes.md"), "ordinary\n")?;
    symlink(home.join(".ssh"), elsewhere.join("link"))?;
    symlink(&home, elsewhere.join("h"))?;
    symlink(home.join(".ssh/missing"), elsewhere.join("ghost"))?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        elsewhere.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    let allowed = |tool: &str, path: &str| {
        let mut args = Map::new();
        args.insert("path".to_owned(), json!(path));
        args.insert("pattern".to_owned(), json!("KEY"));
        (broker.decide_call(tool, ToolKind::Read, false, "c1", &args, None)).allowed
    };
    let shown = home.display().to_string();
    let mut denied = vec![
        "~/.yi/providers/tokens/openai.json".to_owned(),
        "~/.yi/providers/tokens".to_owned(),
        "~/.yi/mcp/tokens/default_mcp.json".to_owned(),
        "link/id_rsa".to_owned(),
        "link".to_owned(),
        "h/.ssh/id_rsa".to_owned(),
        "h/.yi/providers/tokens/openai.json".to_owned(),
        "link/../.ssh/id_rsa".to_owned(),
        "ghost".to_owned(),
    ];
    if home.join(".SSH").exists() {
        denied.push("~/.SSH/id_rsa".to_owned());
        denied.push("~/.YI/providers/tokens/openai.json".to_owned());
    }
    for alias in [
        "/private",
        "/System/Volumes/Data",
        "/System/Volumes/Data/private",
    ] {
        if std::path::Path::new(&format!("{alias}{shown}")).exists() {
            denied.push(format!("{alias}{shown}/.ssh/id_rsa"));
        }
    }
    for tool in ["read", "grep"] {
        for path in &denied {
            assert!(!allowed(tool, path), "{tool} {path} reaches a key");
        }
        for path in ["~/notes.md", "h/notes.md"] {
            assert!(allowed(tool, path), "{tool} {path} is an ordinary file");
        }
    }
    Ok(())
}

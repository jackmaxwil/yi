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
/// sandbox has no network past loopback, so the approval escalates and the question names that.
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
        host_owned: Vec::new(),
        spared: Vec::new(),
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

/// An `exec://` source only ever runs on the host, and one the gate allows outright was admitted
/// before allowed calls ran contained; containing them must not start refusing it.
#[test]
fn an_exec_source_the_gate_allows_is_still_admitted() {
    let project = PathBuf::from("/nonexistent/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
    }));
    assert_eq!(
        yi_runtime::tools::refuse_armed("git status", false, Some(&broker), ""),
        None
    );
    assert!(
        yi_runtime::tools::refuse_armed("touch x", false, Some(&broker), "").is_some(),
        "a source the gate only contains is still refused"
    );
}

/// #1001: an `exec://` source runs on the host, so where Seatbelt exists a walled session's never
/// runs, even one the gate allows outright; yolo, the user's own choice, still admits it.
#[test]
fn a_walled_sessions_exec_source_never_runs_where_a_sandbox_exists() {
    let project = PathBuf::from("/nonexistent/project");
    let broker = |mode| {
        PermissionBroker::new(
            mode,
            project.clone(),
            Vec::new(),
            None,
            tokio::sync::broadcast::channel(8).0,
        )
        .with_sandbox(Some(yi_tools::Sandbox {
            writable: vec![project.clone()],
            deny_read: Vec::new(),
            deny_write: Vec::new(),
            host_owned: Vec::new(),
            spared: Vec::new(),
        }))
    };
    let wall = yi_runtime::Wall {
        deny_read: vec![project.join("secret")],
        ..yi_runtime::Wall::default()
    };
    let refused = |mode| {
        let walled = broker(mode).for_child(&wall, &project);
        yi_runtime::tools::refuse_armed("git status", false, Some(&walled), "")
    };
    let auto = refused(PermissionMode::Auto).unwrap_or_default();
    assert!(auto.contains("never leave"), "{auto}");
    assert_eq!(refused(PermissionMode::Yolo), None, "yolo runs it");
}

/// A rule may allow a credential read the profile would refuse; it runs outside, and says so.
#[test]
fn an_allowed_credential_read_runs_outside_and_says_so() -> TestResult {
    let project = PathBuf::from("/nonexistent/project");
    let rule = yi_runtime::ConfigRule::new("bash", "cat *", yi_runtime::ConfigRuleAction::Allow)?;
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        vec![rule],
        None,
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
    }));
    let mut args = Map::new();
    args.insert("command".to_owned(), json!("cat ~/.netrc"));
    let outcome = broker.decide_call("bash", ToolKind::Exec, true, "c1", &args, None);
    assert_eq!(outcome.containment, Containment::Uncontained);
    let notice = broker
        .outside_notice("bash", &args, &outcome)
        .unwrap_or_default();
    assert!(
        notice.contains("outside the sandbox (credential stores)"),
        "{notice}"
    );
    Ok(())
}

/// A broker over a nonexistent tree with a profile, answering by `asker`, counting the questions.
fn counted(
    rules: Vec<yi_runtime::ConfigRule>,
    answer: fn(usize) -> AskOutcome,
) -> (PermissionBroker, Arc<Mutex<usize>>) {
    let asks = Arc::new(Mutex::new(0_usize));
    let seen = Arc::clone(&asks);
    let asker: Asker = Arc::new(move |_| {
        let mut count = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *count += 1;
        answer(*count)
    });
    let project = PathBuf::from("/nonexistent/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        rules,
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
    }));
    (broker, asks)
}

fn bash_args(command: &str) -> Map<String, Value> {
    let mut args = Map::new();
    args.insert("command".to_owned(), json!(command));
    args
}

/// Review of #933, F1: one segment that must leave took the whole compound outside. After an
/// "always" that kept `python3`, a zero-byte credential read carried any python out unasked.
#[test]
fn a_leaving_segment_does_not_carry_a_compound_out() -> TestResult {
    let (broker, asks) = counted(Vec::new(), |n| match n {
        1 => AskOutcome::AllowAlways(0),
        _ => AskOutcome::Reject,
    });
    let decide = |id: &str, command: &str| {
        broker.decide_call("bash", ToolKind::Exec, true, id, &bash_args(command), None)
    };
    assert!(decide("c1", "python3 evil.py && cat ~/.netrc").allowed);
    let carried = decide("c2", "python3 evil.py && head -c0 ~/.aws/credentials");
    assert!(!carried.allowed, "{}", carried.reason);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        2,
        "the compound asks"
    );
    Ok(())
}

/// A scope an earlier "always" kept (`python3` in this tree) is not the session pass: after a
/// refusal naming no path, another `python3` call asks (review of #933, F2).
#[test]
fn a_kept_scope_is_not_a_session_pass() -> TestResult {
    let (broker, asks) = counted(Vec::new(), |n| match n {
        1 => AskOutcome::AllowAlways(0),
        _ => AskOutcome::Reject,
    });
    let decide = |id: &str, command: &str| {
        broker.decide_call("bash", ToolKind::Exec, true, id, &bash_args(command), None)
    };
    assert!(decide("c1", "python3 evil.py && cat ~/.netrc").allowed);
    broker.note_containment_failure(yi_tools::SandboxRefusal::Scopes(
        yi_permission::refused_scopes("python3 other.py"),
    ));
    let other = decide("c2", "python3 other.py");
    assert!(!other.allowed, "{}", other.reason);
    assert_eq!(*asks.lock().map_err(|_| "poisoned")?, 2, "the retry asks");
    Ok(())
}

/// The same under a configured allow: an install verb took a `sort -o` past the tree.
#[test]
fn a_configured_allow_does_not_carry_a_compound_out() -> TestResult {
    let rule = yi_runtime::ConfigRule::new("bash", "cargo *", yi_runtime::ConfigRuleAction::Allow)?;
    let (broker, asks) = counted(vec![rule], |_| AskOutcome::Reject);
    let command = "cargo test && sort -o /Users/Shared/x input && cargo add serde";
    let outcome = broker.decide_call(
        "bash",
        ToolKind::Exec,
        true,
        "c1",
        &bash_args(command),
        None,
    );
    assert!(!outcome.allowed, "{}", outcome.reason);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        1,
        "the compound asks"
    );
    let whole = broker.decide_call(
        "bash",
        ToolKind::Exec,
        true,
        "c2",
        &bash_args("cargo add serde"),
        None,
    );
    assert_eq!(
        whole.containment,
        Containment::Uncontained,
        "a single leaving segment still runs"
    );
    Ok(())
}

/// Re-review of #933, R2: text the strict parser gives up on fell to the lenient split, which
/// folded a hidden command into the leaving segment's argv, so it left with it unasked.
#[test]
fn an_unparsed_command_never_leaves_unasked() -> TestResult {
    let cargo =
        yi_runtime::ConfigRule::new("bash", "cargo *", yi_runtime::ConfigRuleAction::Allow)?;
    let cat = yi_runtime::ConfigRule::new("bash", "cat *", yi_runtime::ConfigRuleAction::Allow)?;
    let (broker, asks) = counted(vec![cargo, cat], |_| AskOutcome::Reject);
    let hidden = [
        "cargo add serde\nsort -o /Users/Shared/x in",
        "cargo add serde $(sort -o /Users/Shared/x in)",
        "cargo add serde `sort -o /Users/Shared/x in`",
        "cargo add \"$(sort -o /Users/Shared/x in)\"",
        "cargo add serde <(sort -o /Users/Shared/x in)",
        "cargo add serde &sort -o /Users/Shared/x in",
        "cat ~/.netrc\nsort -o /Users/Shared/x in",
        "cat ~/.netrc $(sort -o /Users/Shared/x in)",
    ];
    for (index, command) in hidden.iter().enumerate() {
        let id = format!("c{index}");
        let outcome =
            broker.decide_call("bash", ToolKind::Exec, true, &id, &bash_args(command), None);
        assert!(
            !outcome.allowed,
            "{command:?} left unasked: {}",
            outcome.reason
        );
    }
    assert_eq!(*asks.lock().map_err(|_| "poisoned")?, hidden.len());
    Ok(())
}

/// Re-review of #933, R3: with no one to ask, a split compound's denial said "add an allow
/// rule", which it had; splitting it or `--yolo` is what runs it.
#[test]
fn a_headless_compound_says_to_split_it() -> TestResult {
    let rule = yi_runtime::ConfigRule::new("bash", "cargo *", yi_runtime::ConfigRuleAction::Allow)?;
    let project = PathBuf::from("/nonexistent/project");
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        project.clone(),
        vec![rule],
        None,
        tokio::sync::broadcast::channel(8).0,
    )
    .with_sandbox(Some(yi_tools::Sandbox {
        writable: vec![project],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        host_owned: Vec::new(),
        spared: Vec::new(),
    }));
    let args = bash_args("cargo test && cargo add serde");
    let outcome = broker.decide_call("bash", ToolKind::Exec, true, "c1", &args, None);
    assert!(
        !outcome.allowed
            && outcome
                .reason
                .contains("Split it into separate calls, or rerun with --yolo")
            && !outcome.reason.contains("allow rule"),
        "{}",
        outcome.reason
    );
    Ok(())
}

/// Re-review of #933, R1: `git remote update` is a proven read that runs `uploadpack` from `-c`
/// on the host, and sending it out unasked gave a shell. It asks, as `git fetch` does; no `-c`
/// makes a read verb leave.
#[test]
fn a_git_remote_update_asks_and_no_config_override_leaves() -> TestResult {
    let (broker, asks) = counted(Vec::new(), |_| AskOutcome::Reject);
    for command in [
        "git -c remote.x.url=. -c \"remote.x.uploadpack=touch /Users/Shared/pwn; git-upload-pack\" remote update x",
        "git -c core.hooksPath=h remote prune origin",
        "git remote -v update",
    ] {
        let outcome =
            broker.decide_call("bash", ToolKind::Exec, true, "c", &bash_args(command), None);
        assert!(
            !outcome.allowed,
            "{command} ran unasked: {}",
            outcome.reason
        );
        assert!(yi_runtime::tools::refuse_armed(command, false, Some(&broker), "").is_some());
    }
    assert_eq!(*asks.lock().map_err(|_| "poisoned")?, 6);
    for command in [
        "git -c core.pager=x log -1",
        "git -c remote.x.uploadpack=y status",
        "git --config-env=core.pager=P diff",
        "git -c x=y remote -v",
    ] {
        let outcome =
            broker.decide_call("bash", ToolKind::Exec, true, "c", &bash_args(command), None);
        assert!(
            outcome.allowed && outcome.containment != Containment::Uncontained,
            "{command} left the sandbox: {}",
            outcome.reason
        );
    }
    Ok(())
}

/// The session pass covers only a refusal naming no path: an exact rule whose run was refused
/// writing a path asks to widen by it (review of #933, mutant M9).
#[test]
fn a_passed_command_refused_on_a_path_asks_to_widen() -> TestResult {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let lane = home.join("yi-f-nonexistent-pass");
    let command = format!("touch {}; true", lane.join("x").display());
    let (broker, asks) = counted(Vec::new(), |_| AskOutcome::AllowAlways(0));
    broker.note_containment_failure(yi_tools::SandboxRefusal::Scopes(
        yi_permission::refused_scopes(&command),
    ));
    let decide =
        |id: &str| broker.decide_call("bash", ToolKind::Exec, true, id, &bash_args(&command), None);
    assert_eq!(decide("c1").containment, Containment::Uncontained);
    broker.note_containment_failure(yi_tools::SandboxRefusal::Path(lane.join("x")));
    let retry = decide("c2");
    assert_eq!(
        retry.containment,
        Containment::Contained {
            widen: vec![lane],
            gate_allowed: false
        }
    );
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        2,
        "the path refusal asks"
    );
    Ok(())
}

/// Review of #933, F4: a `job=N` wait runs no command, yet its result said it ran outside.
#[test]
fn a_job_wait_carries_no_sandbox_line() {
    let (broker, _asks) = counted(Vec::new(), |_| AskOutcome::Reject);
    let mut args = Map::new();
    args.insert("job".to_owned(), json!(1));
    let outcome = broker.decide_call("bash", ToolKind::Exec, false, "c1", &args, None);
    assert!(outcome.allowed, "{}", outcome.reason);
    assert_eq!(broker.outside_notice("bash", &args, &outcome), None);
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
        host_owned: Vec::new(),
        spared: Vec::new(),
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
                    Containment::Contained {
                        widen: vec![dir],
                        gate_allowed: false
                    }
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
        host_owned: Vec::new(),
        spared: Vec::new(),
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
        gate_allowed: false,
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

/// Every call that opens a path is judged by the file it opens, not the spelling (#898), and
/// yi's own token stores are key stores (#887): a read or grep prints the key, an edit prints
/// the lines it refuses, a write replaces it. Every spelling below reached a key before.
#[cfg(unix)]
#[test]
fn a_named_read_is_judged_by_the_file_it_opens() -> TestResult {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("yi-scope-identity")?;
    let (home, elsewhere) = (scratch.join("home"), scratch.join("elsewhere"));
    for store in [".ssh", ".yi/providers/tokens", ".yi/mcp/tokens"] {
        std::fs::create_dir_all(home.join(store))?;
    }
    std::fs::create_dir_all(elsewhere.join(".git"))?;
    std::fs::write(elsewhere.join(".git/config"), "[core]\n")?;
    let secret = "FAKE KEY MARKER\n";
    std::fs::write(home.join(".ssh/id_rsa"), secret)?;
    std::fs::write(home.join(".yi/providers/tokens/openai.json"), secret)?;
    std::fs::write(home.join(".yi/mcp/tokens/default_mcp.json"), secret)?;
    std::fs::write(home.join("notes.md"), "ordinary\n")?;
    symlink(home.join(".ssh"), elsewhere.join("link"))?;
    symlink(&home, elsewhere.join("h"))?;
    symlink(home.join(".ssh/missing"), elsewhere.join("ghost"))?;
    symlink(elsewhere.join(".git"), elsewhere.join("gitlink"))?;
    symlink("/dev", elsewhere.join("devlink"))?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        elsewhere.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    let decide = |tool: &str, path: &str| {
        let mut args = Map::new();
        args.insert("path".to_owned(), json!(path));
        args.insert("pattern".to_owned(), json!("KEY"));
        args.insert("content".to_owned(), json!("overwritten"));
        args.insert("patch".to_owned(), json!(format!("[{path}]\nPUT 1.=1:\nx")));
        let (kind, irreversible) = match tool {
            "read" | "grep" => (ToolKind::Read, false),
            _ => (ToolKind::Write, true),
        };
        broker.decide_call(tool, kind, irreversible, "c1", &args, None)
    };
    let shown = home.display().to_string();
    let mut denied = [
        "~/.yi/providers/tokens/openai.json",
        "~/.yi/providers/tokens",
        "~/.yi/mcp/tokens/default_mcp.json",
        "link/id_rsa",
        "link",
        "h/.ssh/id_rsa",
        "h/.yi/providers/tokens/openai.json",
        "h/.yi",
        "link/../.ssh/id_rsa",
        "ghost",
        "gitlink/config",
        "gitlink/no-such",
        "devlink/zero",
        "devlink/no-such-device",
    ]
    .map(str::to_owned)
    .to_vec();
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
            denied.push(format!("{alias}{shown}/.yi/providers/tokens/openai.json"));
            denied.push(format!("{alias}{shown}/.yi/providers"));
        }
    }
    for tool in ["read", "grep", "edit", "write"] {
        for path in &denied {
            let outcome = decide(tool, path);
            assert!(!outcome.allowed, "{tool} {path} reaches a key");
            assert!(!outcome.reason.contains("MARKER"), "{}", outcome.reason);
        }
        for path in ["~/notes.md", "h/notes.md"] {
            assert!(
                decide(tool, path).allowed,
                "{tool} {path} is an ordinary file"
            );
        }
    }
    // An edit's `MV` writes its destination: a key planted through a link, a git hook that
    // runs, a token replaced.
    let moved = |dest: &str| {
        let mut args = Map::new();
        args.insert(
            "patch".to_owned(),
            json!(format!("[h/notes.md]\nMV {dest}")),
        );
        broker.decide_call("edit", ToolKind::Write, true, "c1", &args, None)
    };
    for dest in [
        "link/newkey",
        ".git/hooks/pre-commit",
        "gitlink/hooks/pre-commit",
        "~/.yi/providers/tokens/openai.json",
        "h/.yi/providers/tokens/openai.json",
    ] {
        assert!(!moved(dest).allowed, "edit MV {dest} writes into a store");
    }
    assert!(moved("h/renamed.md").allowed, "a move to an ordinary file");
    // Spellings a line scan would miss and the edit tool's parser reads as the same move.
    for patch in [
        "[h/notes.md]\nMV \"link/newkey\"",
        "[h/notes.md]\nMV 'link/newkey'",
        "[h/notes.md]\r\nMV link/newkey\r\n",
        "[h/notes.md]\n  MV   link/newkey  ",
        "[*** Move to: link/newkey]\nPUT 1.=1:\nx",
        "[*** Update File: link/id_rsa]\nPUT 1.=1:\nx",
    ] {
        let mut args = Map::new();
        args.insert("patch".to_owned(), json!(patch));
        let outcome = broker.decide_call("edit", ToolKind::Write, true, "c1", &args, None);
        assert!(!outcome.allowed, "{patch:?} writes into a store");
    }
    Ok(())
}

pub(crate) struct MainTree(pub(crate) PathBuf);

impl yi_runtime::fetch::MemberTrees for MainTree {
    fn cwd_of(&self, agent: &str) -> Option<PathBuf> {
        (agent == "main").then(|| self.0.clone())
    }
}

/// A scheme that serves a host file opens it on the host, where `decide` never looked: the
/// model's `read tree://main/…` or `local://…` and the kernel's `rlm.fetch` reached the
/// workspace `.git` and, through a link, a key store (#905).
#[cfg(unix)]
#[test]
fn a_url_that_serves_a_host_file_meets_the_read_gate() -> TestResult {
    use yi_runtime::fetch::{FetchError, Resolver};
    let scratch = Scratch::new("yi-scope-url-gate")?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(workspace.join(".git"))?;
    std::fs::write(home.join(".ssh/id_rsa"), "FAKE KEY MARKER\n")?;
    std::fs::write(workspace.join(".git/config"), "GIT CONFIG MARKER\n")?;
    std::fs::write(workspace.join("notes.md"), "ordinary\n")?;
    std::os::unix::fs::symlink(home.join(".ssh"), workspace.join("keys"))?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let resolver = Resolver::new(workspace.clone(), yi_runtime::wall::Wall::default())
        .with_member_trees(Arc::new(MainTree(workspace.clone())));
    for url in [
        "tree://main/.git/config",
        "tree://main/keys/id_rsa",
        "local://.git/config",
        "local://keys/id_rsa",
    ] {
        let refused = matches!(
            resolver.fetch(&url.parse()?),
            Err(FetchError::Denied { .. } | FetchError::OutsideWorkspace { .. })
        );
        assert!(refused, "{url}");
    }
    for url in ["tree://main/notes.md", "local://notes.md"] {
        assert!(
            resolver.fetch(&url.parse()?).is_ok(),
            "{url} is an ordinary file"
        );
    }
    Ok(())
}

/// A link swapped between the check and the open (#890): the broker judges the name, then the
/// real tool opens it while a thread flips `x` between an ordinary file and a link to a key
/// (`Swap::File`), or `d` between a directory and a link to the key store (`Swap::Parent`). The
/// reviews of #885 and #903 read a key 40 times in 20,000 tries; `reached` names a hit.
#[cfg(unix)]
#[derive(Clone, Copy)]
enum Swap {
    File,
    Parent,
}

#[cfg(unix)]
fn race(
    tool: &str,
    swap: Swap,
    benign: &str,
    args: &dyn Fn(&dyn yi_tools::Tool, &yi_tools::ToolContext) -> Map<String, Value>,
    reached: &dyn Fn(&yi_tools::ToolOutput, &std::path::Path) -> bool,
) -> Result<(usize, usize), Box<dyn Error>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let scratch = Scratch::new("yi-scope-swap")?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(workspace.join("d"))?;
    let key = home.join(".ssh/id_rsa");
    std::fs::write(&key, "FAKE KEY MARKER\n")?;
    std::fs::write(workspace.join("x"), benign)?;
    std::fs::write(workspace.join("d/id_rsa"), benign)?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let broker = PermissionBroker::new(
        PermissionMode::Yolo,
        workspace.clone(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    );
    let tools = yi_tools::builtin_tools();
    let tool_impl = (tools.iter())
        .find(|candidate| candidate.name() == tool)
        .ok_or("no such tool")?;
    let context = yi_tools::ToolContext::new(workspace.clone());
    let read = (tools.iter())
        .find(|candidate| candidate.name() == "read")
        .ok_or("no read tool")?;
    let args = args(read.as_ref(), &context);
    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let (stop, workspace, home) = (Arc::clone(&stop), workspace.clone(), home.clone());
        let benign = benign.to_owned();
        std::thread::spawn(move || {
            let at = |name: &str| workspace.join(name);
            // Each state is held a while, a different while each round, so some round's hold
            // straddles the gap between a tool's check and its open, however wide.
            let mut round = 0_u32;
            let hold = |round: u32| {
                let until = std::time::Instant::now()
                    + std::time::Duration::from_micros(u64::from(round % 16) * 125);
                while std::time::Instant::now() < until {}
            };
            while !stop.load(Ordering::Relaxed) {
                round = round.wrapping_add(1);
                match swap {
                    Swap::File => {
                        let _ = std::fs::write(at("plain.tmp"), &benign);
                        let _ = std::fs::rename(at("plain.tmp"), at("x"));
                        let _ = std::os::unix::fs::symlink(home.join(".ssh/id_rsa"), at("l.tmp"));
                        let _ = std::fs::rename(at("l.tmp"), at("x"));
                    }
                    Swap::Parent => {
                        let _ = std::fs::write(at("d/id_rsa"), &benign);
                        hold(round / 16);
                        let _ = std::fs::rename(at("d"), at("dir.tmp"));
                        let _ = std::os::unix::fs::symlink(home.join(".ssh"), at("d"));
                        hold(round);
                        let _ = std::fs::remove_file(at("d"));
                        let _ = std::fs::rename(at("dir.tmp"), at("d"));
                    }
                }
            }
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let (mut allowed, mut leaks) = (0_usize, 0_usize);
    for _ in 0..20_000 {
        if std::time::Instant::now() > deadline {
            break;
        }
        let (kind, irreversible) = (tool_impl.kind_for(&args), tool_impl.irreversible(&args));
        if !(broker.decide_call(tool, kind, irreversible, "c1", &args, None)).allowed {
            continue;
        }
        allowed += 1;
        let output = tool_impl.execute(args.clone(), &context);
        if reached(&output, &key) {
            leaks += 1;
            std::fs::write(&key, "FAKE KEY MARKER\n")?;
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().map_err(|_| "the swapper panicked")?;
    Ok((allowed, leaks))
}

#[cfg(unix)]
fn shows_key(output: &yi_tools::ToolOutput, _: &std::path::Path) -> bool {
    format!("{:?}", output.result).contains("MARKER")
}

/// A write that lands on the key replaces it; an edit's refusal that prints its rows shows it.
#[cfg(unix)]
fn changes_key(output: &yi_tools::ToolOutput, key: &std::path::Path) -> bool {
    shows_key(output, key)
        || std::fs::read_to_string(key).is_ok_and(|text| !text.contains("MARKER"))
}

#[cfg(unix)]
fn path_arg(
    path: &str,
) -> impl Fn(&dyn yi_tools::Tool, &yi_tools::ToolContext) -> Map<String, Value> + '_ {
    move |_, _| {
        let mut args = Map::new();
        args.insert("path".to_owned(), json!(path));
        args
    }
}

#[cfg(unix)]
fn assert_no_leak(tool: &str, swap: Swap, (allowed, leaks): (usize, usize)) {
    let swapped = match swap {
        Swap::File => "file",
        Swap::Parent => "parent",
    };
    assert!(
        allowed > 20,
        "{tool}: the check passed only {allowed} times"
    );
    assert_eq!(
        leaks, 0,
        "{tool}, {swapped} swap: {leaks} of {allowed} calls reached the key"
    );
}

#[cfg(unix)]
#[test]
fn a_link_swapped_after_the_check_reads_no_key() -> TestResult {
    assert_no_leak(
        "read",
        Swap::File,
        race("read", Swap::File, "ordinary\n", &path_arg("x"), &shows_key)?,
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_parent_swapped_after_the_check_reads_no_key() -> TestResult {
    let tried = race(
        "read",
        Swap::Parent,
        "ordinary\n",
        &path_arg("d/id_rsa"),
        &shows_key,
    )?;
    assert_no_leak("read", Swap::Parent, tried);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_link_swapped_after_the_check_overwrites_no_key() -> TestResult {
    let args = |_: &dyn yi_tools::Tool, _: &yi_tools::ToolContext| write("x", "overwritten\n");
    assert_no_leak(
        "write",
        Swap::File,
        race("write", Swap::File, "ordinary\n", &args, &changes_key)?,
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_parent_swapped_after_the_check_overwrites_no_key() -> TestResult {
    let args =
        |_: &dyn yi_tools::Tool, _: &yi_tools::ToolContext| write("d/id_rsa", "overwritten\n");
    let tried = race("write", Swap::Parent, "ordinary\n", &args, &changes_key)?;
    assert_no_leak("write", Swap::Parent, tried);
    Ok(())
}

/// The edit names `d/id_rsa` by the tag a read of the ordinary file minted before the swaps.
#[cfg(unix)]
#[test]
fn a_parent_swapped_after_the_check_edits_no_key() -> TestResult {
    let args = |read: &dyn yi_tools::Tool, context: &yi_tools::ToolContext| {
        let shown = read.execute(path_arg("d/id_rsa")(read, context), context);
        let text = format!("{:?}", shown.result);
        let tag: String = (text
            .split_once("d/id_rsa#")
            .map(|(_, tail)| tail)
            .unwrap_or(""))
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
        let mut args = Map::new();
        let patch = format!("[d/id_rsa#{tag}]\nPUT 1.=1:\n+overwritten\n");
        args.insert("patch".to_owned(), json!(patch));
        args
    };
    // A long file widens the gap between the edit's read and its write.
    let benign = format!("ordinary\n{}", "filler\n".repeat(20_000));
    let tried = race("edit", Swap::Parent, &benign, &args, &changes_key)?;
    assert_no_leak("edit", Swap::Parent, tried);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_parent_swapped_after_the_check_rewrites_no_key_by_grep() -> TestResult {
    let args = |_: &dyn yi_tools::Tool, _: &yi_tools::ToolContext| {
        let mut args = Map::new();
        args.insert("pattern".to_owned(), json!("ordinary"));
        args.insert("path".to_owned(), json!("d"));
        args.insert("replace".to_owned(), json!("overwritten"));
        args.insert("apply".to_owned(), json!(true));
        args
    };
    let tried = race("grep", Swap::Parent, "ordinary\n", &args, &changes_key)?;
    assert_no_leak("grep apply", Swap::Parent, tried);
    Ok(())
}

/// The remedy the ping hint names: after the sandbox refuses a real `ping`, the next `ping` asks,
/// and approving it runs that call outside the sandbox. The refusal is the bash tool's own, read
/// off a contained run of `ping`.
#[test]
fn a_refused_ping_asks_and_approving_runs_it_outside() -> TestResult {
    let command = "ping -c1 -W1 1.1.1.1";
    let scratch = Scratch::new("yi-ping-remedy")?;
    let home = scratch.join("home");
    std::fs::create_dir_all(&home)?;
    let sandbox = yi_tools::Sandbox::for_workspace(&scratch, &home, None);
    let context = yi_tools::ToolContext::new(scratch.to_path_buf());
    let timeout = std::time::Duration::from_secs(60);
    let yi_tools::Run::Finished(capture) =
        yi_tools::run_or_background(command, &context, None, timeout, Some(&sandbox), None)?
    else {
        return Err("ping did not finish".into());
    };
    let output = format!("{}{}", capture.stdout, capture.stderr);
    let refusal =
        yi_tools::sandbox_refusal(&sandbox, &scratch, capture.exit_code, &output, command)
            .ok_or_else(|| format!("no refusal read from: {output}"))?;
    let (broker, asks) = counted(Vec::new(), |_| AskOutcome::AllowOnce);
    let decide =
        |id: &str| broker.decide_call("bash", ToolKind::Exec, true, id, &bash_args(command), None);
    assert!(matches!(
        decide("c1").containment,
        Containment::Contained { .. }
    ));
    broker.note_containment_failure(refusal);
    let retry = decide("c2");
    assert!(retry.allowed, "{}", retry.reason);
    assert_eq!(retry.containment, Containment::Uncontained);
    assert_eq!(*asks.lock().map_err(|_| "poisoned")?, 1, "the retry asks");
    Ok(())
}

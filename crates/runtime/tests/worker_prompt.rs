//! A worker's request carries its role's text and the project's rules, and nothing else of the
//! root's prompt: no identity, no doctrine, no skill catalog.

use crate::family_cache::{body_asking, root, root_with, route, stand_in};
use crate::scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_is_sent_its_role_and_the_project_rules_and_nothing_of_the_root() -> TestResult {
    let scratch = Scratch::new("yi-worker-prompt")?;
    let (port, from) = stand_in(String::new())?;
    let model = route(
        "claude-probe",
        "anthropic-messages",
        "anthropic",
        "http://anthropic.invalid",
    );
    let (_session, host) = root(&scratch, model, port, false)?;
    let ws = scratch.join("ws");
    std::fs::write(ws.join("AGENTS.md"), "Indent with tabs, never spaces.\n")?;
    let skill = ws.join(".yi/skills/release-notes");
    std::fs::create_dir_all(&skill)?;
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: release-notes\ndescription: Draft release notes.\n---\nBody.\n",
    )?;
    let kwargs = json!({"role": "worker", "partition": ["local://notes.txt"]});
    host.spawn(
        "Rename the sky line to grey sky.".to_owned(),
        kwargs.as_object().cloned().unwrap_or_default(),
    )?;
    let mut bodies = Vec::new();
    let body = body_asking(&mut bodies, &from, "grey sky").await?;
    let system = body["system"].to_string();
    assert!(system.contains("You make one change."), "{system}");
    assert!(
        system.contains("Indent with tabs, never spaces."),
        "{system}"
    );
    for root_only in [
        yi_runtime::identity_fragment(),
        yi_runtime::doctrine_fragment(),
    ] {
        let first = root_only.lines().next().unwrap_or_default();
        assert!(
            !system.contains(first),
            "{first} reached the worker: {system}"
        );
    }
    assert!(!system.contains("release-notes"), "{system}");
    let tools: Vec<&str> = body["tools"]
        .as_array()
        .ok_or("no tools")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(tools, ["read", "edit", "write", "grep"], "{body}");
    assert!(
        body["messages"][0].to_string().contains("2:blue sky"),
        "the partition is inlined: {}",
        body["messages"][0]
    );
    Ok(())
}

fn git(dir: &std::path::Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = yi_tools::command("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
        .arg(dir)
        .args(args)
        .output()?
        .status;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("git {args:?}").into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_in_its_own_lane_is_told_the_committed_rules_its_mode_and_the_rust_pack()
-> TestResult {
    let scratch = Scratch::new("yi-worker-lane")?;
    let (port, from) = stand_in(String::new())?;
    let model = route(
        "claude-probe",
        "anthropic-messages",
        "anthropic",
        "http://anthropic.invalid",
    );
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_permission::PermissionMode::Ask,
        scratch.join("ws"),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    ));
    let (_session, host) = root_with(&scratch, model, port, false, Some(broker))?;
    let ws = scratch.join("ws");
    std::fs::write(ws.join("AGENTS.md"), "Indent with tabs, never spaces.\n")?;
    std::fs::write(ws.join("Cargo.toml"), "[package]\nname = \"probe\"\n")?;
    git(&ws, &["init", "-q", "-b", "main"])?;
    git(&ws, &["add", "."])?;
    git(&ws, &["commit", "-q", "-m", "seed"])?;
    std::fs::write(ws.join("AGENTS.md"), "Indent with spaces, never tabs.\n")?;
    let kwargs = json!({"role": "worker", "isolation": "worktree"});
    host.spawn(
        "Rename the probe crate to kite.".to_owned(),
        kwargs.as_object().cloned().unwrap_or_default(),
    )?;
    let mut bodies = Vec::new();
    let body = body_asking(&mut bodies, &from, "crate to kite").await?;
    let system = body["system"].to_string();
    assert!(
        system.contains(r#"source=\"AGENTS.md\" trust=\"granted\""#),
        "{system}"
    );
    assert!(
        system.contains("Indent with tabs") && !system.contains("Indent with spaces"),
        "the lane's committed rules, not the parent's edit: {system}"
    );
    assert!(system.contains("Permission mode: ask."), "{system}");
    assert!(system.contains("# Rust discipline"), "{system}");
    Ok(())
}

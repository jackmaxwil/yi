//! The CLI's plan verbs through the spawned binary (plan section 5.6): every op applies as the
//! owner through the tool's own parser, and argv can neither name nor mint a user.

use std::error::Error;
use std::process::{Command, Output};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn yi_plan(root: &Scratch, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the surface under test is the spawned binary's argv, exit code and streams"
    )]
    let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
    command
        .arg("plan")
        .args(args)
        .arg("--session-dir")
        .arg(root.join("home/sessions"))
        .arg("--cwd")
        .arg(root.join("project"))
        .env("HOME", root.join("home"))
        .current_dir(root.join("project"));
    Ok(command.output()?)
}

fn streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn journal(root: &Scratch) -> Result<String, Box<dyn Error>> {
    let mut text = String::new();
    for entry in std::fs::read_dir(root.join("project/.yi/plans"))? {
        let path = entry?.path().join("ops.jsonl");
        if path.is_file() {
            text.push_str(&std::fs::read_to_string(path)?);
        }
    }
    Ok(text)
}

/// An agent's shell runs this binary as easily as a human does, so the CLI applies as the owner
/// and refuses what the owner may not do; nothing typed on argv reaches the record as a user.
#[test]
fn agent_cli_cannot_reset_fuse_as_user() -> TestResult {
    let root = Scratch::new("yi-plan-cli-authority")?;
    std::fs::create_dir_all(root.join("project"))?;
    root.home()?;
    let opened = yi_plan(
        &root,
        &[
            "init",
            r#"{"goal":"ship the seam","todos":[{"label":"cut"},{"label":"ship","after":["cut"]}]}"#,
        ],
    )?;
    assert_eq!(opened.status.code(), Some(0), "{}", streams(&opened));
    let started = yi_plan(&root, &["start", r#"{"label":"cut"}"#])?;
    assert_eq!(started.status.code(), Some(0), "{}", streams(&started));
    let repaired = yi_plan(&root, &["repair"])?;
    assert_eq!(
        repaired.status.code(),
        Some(0),
        "a repair with no resolutions is the owner's: {}",
        streams(&repaired)
    );

    let reset = yi_plan(&root, &["fuse", "reset"])?;
    assert_ne!(reset.status.code(), Some(0));
    assert!(
        streams(&reset).contains("confirmation") && !streams(&reset).contains("yi console"),
        "the refusal names no surface that cannot ask: {}",
        streams(&reset)
    );
    let claimed = yi_plan(&root, &["fuse", "reset", r#"{"actor":"user://1"}"#])?;
    assert!(
        streams(&claimed).contains("actor is not an argument"),
        "{}",
        streams(&claimed)
    );
    let resolved = yi_plan(
        &root,
        &[
            "repair",
            r#"{"resolutions":[{"label":"cut","action":"retry"}]}"#,
        ],
    )?;
    assert_ne!(resolved.status.code(), Some(0));
    assert!(
        streams(&resolved).contains("confirmation"),
        "{}",
        streams(&resolved)
    );

    let recorded = journal(&root)?;
    assert!(recorded.contains(r#""op":"start""#), "{recorded}");
    assert!(
        !recorded.contains(r#""op":"fuse_reset""#) && !recorded.contains("user://"),
        "the CLI minted a user: {recorded}"
    );
    let viewed = yi_plan(&root, &["view", "--json"])?;
    let view: serde_json::Value = serde_json::from_slice(&viewed.stdout)?;
    assert!(
        view["revision"]
            .as_u64()
            .is_some_and(|revision| revision >= 2),
        "{view}"
    );
    Ok(())
}

/// The two remedies the store prints are typed as printed: a format-1 read names
/// `yi plan import <id>`, a checkpoint with no journal names `yi plan import local://<path>`,
/// and each line runs through the CLI as the word it is.
#[test]
fn the_import_commands_the_store_prints_run_as_printed() -> TestResult {
    let root = Scratch::new("yi-plan-cli-import")?;
    let plans = root.join("project/.yi/plans");
    std::fs::create_dir_all(&plans)?;
    root.home()?;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../runtime/tests/fixtures/plans/format1/campaign.md");
    let id = "ship-logrotate-lite-with-a-packaged";
    std::fs::copy(&fixture, plans.join(format!("{id}.md")))?;

    let read_only = yi_plan(&root, &["view", id])?;
    assert_ne!(read_only.status.code(), Some(0));
    let printed = format!("yi plan import {id}");
    assert!(
        streams(&read_only).contains(&printed),
        "the read names the remedy: {}",
        streams(&read_only)
    );
    let imported = yi_plan(&root, &["import", id])?;
    assert_eq!(imported.status.code(), Some(0), "{}", streams(&imported));
    assert!(plans.join(id).join("ops.jsonl").is_file());

    std::fs::remove_file(plans.join(id).join("ops.jsonl"))?;
    let detached = yi_plan(&root, &["view", id])?;
    assert_ne!(detached.status.code(), Some(0));
    let text = streams(&detached);
    let remedy = text
        .split("`yi plan ")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .ok_or_else(|| format!("no remedy printed: {text}"))?
        .to_owned();
    let words: Vec<&str> = remedy.split_whitespace().collect();
    assert_eq!(words.first().copied(), Some("import"), "{remedy}");
    assert!(
        words
            .get(1)
            .is_some_and(|word| word.starts_with("local://")),
        "{remedy}"
    );
    let adopted = yi_plan(&root, &words)?;
    assert_eq!(adopted.status.code(), Some(0), "{}", streams(&adopted));
    assert!(
        std::fs::read_to_string(plans.join(id).join("ops.jsonl"))?.contains(r#""op":"import""#)
    );
    Ok(())
}

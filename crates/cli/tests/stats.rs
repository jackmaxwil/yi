//! `yi stats` must bill a session exactly as its ledger does: child lanes and side calls book
//! their spend as `usage` records, which no message entry carries.

use std::error::Error;
use std::process::Command;

use serde_json::{Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
use yi_types::message::AgentMessage;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// A session priced 0.10 by its assistant reply and 0.05 by a child's usage record: the fold
/// that reads replies alone reports 0.10 and silently truncates a family's bill.
#[test]
fn stats_bills_a_sessions_usage_records_as_its_ledger_does() -> TestResult {
    let dir = Scratch::new("stats-child-spend")?;
    let home = dir.home()?;
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions)?;
    let mut repo = JsonlRepo::new(sessions.clone(), dir.display().to_string());
    let store = repo.create(CreateOptions::default())?;
    let id = lock_session(&store).metadata().id.clone();
    {
        let mut session = lock_session(&store);
        let reply: AgentMessage = serde_json::from_value(json!({
            "role": "assistant", "api": "faux", "provider": "faux", "model": "faux-1",
            "content": [{"type": "text", "text": "priced"}], "stopReason": "stop", "timestamp": 0,
            "usage": {"input": 300, "output": 20, "cacheRead": 9_700, "cacheWrite": 0,
                      "totalTokens": 10_020,
                      "cost": {"input": 0.04, "output": 0.03, "cacheRead": 0.03,
                               "cacheWrite": 0, "total": 0.1}}
        }))?;
        session.append_message("main", reply)?;
        let record = crate::usage_record::child_usage(session.next_id(), 100, 0.05)?;
        session.append_record(record)?;
    }
    let ledger = lock_session(&store).stats();
    drop(store);

    #[expect(
        clippy::disallowed_methods,
        reason = "the contract is the shipped binary's own output, not the fold it calls"
    )]
    let out = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "stats",
            &id,
            "--json",
            "--session-dir",
            &sessions.display().to_string(),
            "--cwd",
            &dir.display().to_string(),
        ])
        .env("HOME", &home)
        .output()?;
    assert!(
        out.status.success(),
        "stats exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let folded: Value = serde_json::from_slice(&out.stdout)?;
    let cost = folded["cost"].as_f64().ok_or("no cost in the row")?;
    assert!(
        (cost - ledger.cost_total).abs() < 1e-9,
        "yi stats billed {cost} where the ledger bills {}: {}",
        ledger.cost_total,
        folded
    );
    assert!(
        (cost - 0.15).abs() < 1e-9,
        "the reply is 0.10 and the child's row 0.05, so the session bills 0.15: {folded}"
    );
    let models = folded["models"].as_array().ok_or("no models in the row")?;
    assert_eq!(
        models.len(),
        1,
        "a usage cause is not a model, so the model table holds the reply's model alone: {folded}"
    );
    assert_eq!(models[0]["model"], "faux/faux-1", "{folded}");
    Ok(())
}

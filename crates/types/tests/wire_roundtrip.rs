use std::error::Error;
use std::fs;
use std::path::Path;

use yi_types::wire::{JsonlV4Header, Mutation};

fn roundtrip_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let mut rebuilt = String::new();
    for (index, line) in content.lines().enumerate() {
        let reencoded = if index == 0 {
            let header: JsonlV4Header = serde_json::from_str(line)?;
            serde_json::to_string(&header)?
        } else {
            let mutation: Mutation = serde_json::from_str(line)?;
            serde_json::to_string(&mutation)?
        };
        assert_eq!(
            reencoded,
            line,
            "byte drift in {} line {}",
            path.display(),
            index + 1
        );
        rebuilt.push_str(&reencoded);
        rebuilt.push('\n');
    }
    assert_eq!(rebuilt, content, "whole-file drift in {}", path.display());
    Ok(())
}

#[test]
fn pi_v4_fixtures_roundtrip_byte_identical() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut seen = 0;
    for fixture in fs::read_dir(&dir)? {
        let path = fixture?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            roundtrip_file(&path)?;
            seen += 1;
        }
    }
    assert!(seen >= 2, "expected at least 2 jsonl fixtures, saw {seen}");
    Ok(())
}

#[test]
fn goal_without_check_fields_still_deserializes_and_reserializes_clean()
-> Result<(), Box<dyn std::error::Error>> {
    // A pre-check-gate goal fact (0.10.0 shape) must parse forever (§20), and
    // absent Options must not appear on re-serialize.
    let stored = r#"{"objective":"ship","status":"active","tokensUsed":5,"timeUsedSeconds":2,"created":1,"updated":2}"#;
    let goal: yi_types::goal::Goal = serde_json::from_str(stored)?;
    assert!(goal.check.is_none() && goal.check_failure.is_none());
    let out = serde_json::to_string(&goal)?;
    assert!(
        !out.contains("check"),
        "absent check fields must not serialize: {out}"
    );
    Ok(())
}

#[test]
fn goal_check_fields_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let json = r#"{"objective":"ship","status":"active","tokensUsed":0,"timeUsedSeconds":0,"created":1,"updated":1,"check":"just check","checkTimeoutMs":1000,"checkFailure":"tail"}"#;
    let goal: yi_types::goal::Goal = serde_json::from_str(json)?;
    assert_eq!(goal.check.as_deref(), Some("just check"));
    assert_eq!(goal.check_timeout_ms, Some(1000));
    let out = serde_json::to_value(&goal)?;
    assert_eq!(out["checkFailure"], "tail");
    Ok(())
}

#[test]
fn goal_discovery_ledger_round_trips_with_unknown_fields() -> Result<(), Box<dyn std::error::Error>>
{
    // The drain gate re-reads this ledger after a resume, so a row an older
    // writer produced must come back byte-identical (§20).
    let stored = r#"{"objective":"ship","status":"active","tokensUsed":0,"timeUsedSeconds":0,"created":1,"updated":1,"discoveries":[{"text":"the wall config is stale","violatesCheckOf":"t2","fingerprint":"abc","source":"finder"}]}"#;
    let goal: yi_types::goal::Goal = serde_json::from_str(stored)?;
    assert_eq!(goal.discoveries.len(), 1);
    assert_eq!(serde_json::to_string(&goal)?, stored);
    Ok(())
}

#[test]
fn daemon_ledger_fixture_round_trips_with_unknown_fields() -> Result<(), Box<dyn std::error::Error>>
{
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/daemon-ledger-v1.json");
    let stored = fs::read_to_string(path)?;
    let ledger: yi_types::acp::DaemonLedger = serde_json::from_str(&stored)?;
    let entry = ledger.sessions.get("s-1").ok_or("the s-1 row")?;
    assert_eq!(entry.unseen, 2);
    assert_eq!(entry.extra.get("pinned"), Some(&serde_json::json!(true)));
    assert_eq!(serde_json::to_string(&ledger)?, stored.trim_end());
    Ok(())
}

#[test]
fn task_without_a_check_still_deserializes_and_reserializes_clean()
-> Result<(), Box<dyn std::error::Error>> {
    // A pre-check-gate plan fact must parse forever (§20), and an absent check
    // must not appear on re-serialize.
    let stored = r#"{"fact":"plan","plan":{"version":1,"tasks":[{"id":"t1","title":"x","acceptance":"y","state":"done"}],"created":1,"updated":2}}"#;
    let fact: yi_types::wire::Fact = serde_json::from_str(stored)?;
    let yi_types::wire::Fact::Plan { plan } = &fact else {
        return Err("expected a plan fact".into());
    };
    assert!(plan.tasks[0].check.is_none());
    assert_eq!(serde_json::to_string(&fact)?, stored);
    Ok(())
}

#[test]
fn a_persisted_red_streak_lands_in_the_extra_map() -> Result<(), Box<dyn std::error::Error>> {
    // D77's ladder is gone (F0c): the streak is unread data that still round-trips byte for
    // byte through the flatten map, so an old fact parses forever and re-emits itself.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v4-plan-red.jsonl");
    let content = fs::read_to_string(&path)?;
    let line = content.lines().nth(1).ok_or("fixture needs a fact line")?;
    let Mutation::Fact {
        fact: yi_types::wire::Fact::Plan { plan },
        ..
    } = serde_json::from_str(line)?
    else {
        return Err("expected a plan fact".into());
    };
    let task = plan
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?;
    assert_eq!(task.extra.get("redCount"), Some(&serde_json::json!(2)));
    assert_eq!(
        task.extra.get("redFingerprint"),
        Some(&serde_json::json!("3f6a1c0b9d2e4857"))
    );
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<Mutation>(line)?)?,
        line,
        "unread data re-emits verbatim"
    );
    Ok(())
}

#[test]
fn plan_fact_round_trips_with_unknown_fields_and_states() -> Result<(), Box<dyn std::error::Error>>
{
    // Durable §20 rules: unknown fields survive the flatten map; an unknown
    // task state decodes to Other and re-emits verbatim.
    let line = r#"{"fact":"plan","plan":{"version":3,"tasks":[{"id":"t1","title":"x","acceptance":"y","state":"paused_by_future_yi","futureField":7}],"created":1,"updated":2,"planWide":"kept"}}"#;
    let fact: yi_types::wire::Fact = serde_json::from_str(line)?;
    let yi_types::wire::Fact::Plan { plan } = &fact else {
        return Err("expected a plan fact".into());
    };
    assert_eq!(plan.version, yi_types::plan::PlanVersion(3));
    assert_eq!(
        plan.tasks[0].state,
        yi_types::plan::TaskState::Other("paused_by_future_yi".to_owned())
    );
    let out = serde_json::to_value(&fact)?;
    assert_eq!(out["plan"]["planWide"], "kept");
    assert_eq!(out["plan"]["tasks"][0]["futureField"], 7);
    assert_eq!(out["plan"]["tasks"][0]["state"], "paused_by_future_yi");
    Ok(())
}

// The byte round-trip alone cannot tell a typed field from a dropped one, so
// the assertion is that the flag lands typed — proven red by misspelling the
// wire key in the fixture.
#[test]
fn an_unreported_usage_lands_typed_and_a_reported_zero_stays_free()
-> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v4-usage-unknown.jsonl");
    let content = fs::read_to_string(&path)?;
    let mut flags = Vec::new();
    for line in content.lines().skip(1) {
        let mutation: Mutation = serde_json::from_str(line)?;
        if let Mutation::Entry {
            entry:
                yi_types::entry::Entry::Message {
                    message: yi_types::message::AgentMessage::Assistant { usage, .. },
                    ..
                },
            ..
        } = mutation
        {
            flags.push(usage.unknown);
        }
    }
    assert_eq!(flags, vec![true, false]);
    Ok(())
}

#[test]
fn readmit_lands_in_extra() -> Result<(), Box<dyn std::error::Error>> {
    // D77's `readmit` grant has no reader since F0c; the field parks in the flatten map and
    // re-emits byte-identical, and `Task` carries no typed slot for it.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v4-plan-readmit.jsonl");
    let content = fs::read_to_string(&path)?;
    let line = content.lines().nth(1).ok_or("fixture needs a fact line")?;
    let Mutation::Fact {
        fact: yi_types::wire::Fact::Plan { plan },
        ..
    } = serde_json::from_str(line)?
    else {
        return Err("expected a plan fact".into());
    };
    let task = plan
        .task(&yi_types::plan::TaskId("t1".to_owned()))
        .ok_or("t1")?;
    assert_eq!(task.extra.get("readmit"), Some(&serde_json::json!(true)));
    assert_eq!(task.extra.get("redCount"), Some(&serde_json::json!(2)));
    let ungranted = plan
        .task(&yi_types::plan::TaskId("t2".to_owned()))
        .ok_or("t2")?;
    assert!(
        ungranted.extra.is_empty(),
        "a task no rung refused carries nothing: {:?}",
        ungranted.extra
    );
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<Mutation>(line)?)?,
        line,
        "an absent grant emits nothing, so the shape is byte-stable"
    );
    Ok(())
}

#[test]
fn an_undo_checkpoint_recorded_before_after_still_round_trips_byte_identical()
-> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/checkpoint-undo-v1.json");
    let stored = fs::read_to_string(path)?;
    let data: yi_types::checkpoint::CheckpointData = serde_json::from_str(&stored)?;
    assert_eq!(data.at, yi_types::checkpoint::CheckpointAt::Undo);
    assert_eq!(data.after, None);
    assert_eq!(serde_json::to_string(&data)?, stored.trim_end());
    Ok(())
}

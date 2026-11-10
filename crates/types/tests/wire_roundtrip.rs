use std::error::Error;
use std::fs;
use std::path::Path;

use yi_types::wire::{JsonlV4Header, Mutation};
fn fixtures() -> Result<std::path::PathBuf, Box<dyn Error>> {
    Ok(crate::workspace_root()?.join("crates/types/tests/fixtures"))
}

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
    let dir = fixtures()?;
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

/// Recorded by the 0.382.0 binary, before a tick made todos: it loads with no policy and
/// writes back the bytes `JobStore` wrote, so the prompt-era store is still the store.
#[test]
fn a_prompt_era_job_store_reads_with_default_policies_and_writes_back_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("scheduled-jobs-v1.json");
    let stored = fs::read_to_string(path)?;
    let state: yi_types::schedule::ScheduleState = serde_json::from_str(&stored)?;
    let job = state.jobs.first().ok_or("the recorded job")?;
    assert!(job.intent.is_empty() && job.unblocks.is_none() && !job.halted);
    assert_eq!((&job.overlap, &job.catch_up), (&None, &None));
    assert_eq!(serde_json::to_string_pretty(&state)?, stored);
    Ok(())
}

#[test]
fn daemon_ledger_fixture_round_trips_with_unknown_fields() -> Result<(), Box<dyn std::error::Error>>
{
    let path = fixtures()?.join("daemon-ledger-v1.json");
    let stored = fs::read_to_string(path)?;
    let ledger: yi_types::acp::DaemonLedger = serde_json::from_str(&stored)?;
    let entry = ledger.sessions.get("s-1").ok_or("the s-1 row")?;
    assert_eq!(entry.unseen, 2);
    assert_eq!(entry.extra.get("pinned"), Some(&serde_json::json!(true)));
    assert_eq!(serde_json::to_string(&ledger)?, stored.trim_end());
    Ok(())
}

/// Written by `yi serve` after one `_yi/pin`: only the pinned row carries `pinnedMs`, byte for byte.
#[test]
fn daemon_ledger_with_a_pin_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("daemon-ledger-pinned.json");
    let stored = fs::read_to_string(path)?;
    let ledger: yi_types::acp::DaemonLedger = serde_json::from_str(&stored)?;
    let pins: Vec<Option<u64>> = ledger
        .sessions
        .values()
        .map(|entry| entry.pinned_ms)
        .collect();
    assert_eq!(pins, [Some(1_791_874_130_106), None]);
    assert_eq!(serde_json::to_string(&ledger)?, stored.trim_end());
    Ok(())
}

/// Dies with an old plan fact re-emitted with its keys reordered or a field dropped: the
/// payload is raw, so every committed plan line comes back byte for byte.
#[test]
fn a_legacy_plan_fact_is_kept_raw_and_re_emits_verbatim() -> Result<(), Box<dyn std::error::Error>>
{
    let fixtures = fixtures()?;
    for name in [
        "v4-plan-check",
        "v4-plan-readmit",
        "v4-plan-red",
        "v4-plan-pointer",
    ] {
        let content = fs::read_to_string(fixtures.join(format!("{name}.jsonl")))?;
        let line = content.lines().last().ok_or("fixture needs a fact line")?;
        let mutation: Mutation = serde_json::from_str(line)?;
        let Mutation::Fact {
            fact: yi_types::wire::Fact::Plan { plan },
            ..
        } = &mutation
        else {
            return Err(format!("{name}: expected a plan fact").into());
        };
        assert!(plan.get("version").is_some(), "{name}: {plan}");
        assert_eq!(serde_json::to_string(&mutation)?, line, "{name}");
    }
    Ok(())
}

// The byte round-trip alone cannot tell a typed field from a dropped one, so
// the assertion is that the flag lands typed — proven red by misspelling the
// wire key in the fixture.
#[test]
fn an_unreported_usage_lands_typed_and_a_reported_zero_stays_free()
-> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("v4-usage-unknown.jsonl");
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

/// An unreported usage is a missing cost, not a free one, so a sum over it prints as a floor;
/// the dollar boundary rounds before it switches, so `$0.9996` never reads `$1.000`.
#[test]
fn money_prints_one_way_and_marks_a_floor() {
    use yi_types::message::fmt_cost;
    assert_eq!(fmt_cost(0.45, false), "$0.450");
    assert_eq!(fmt_cost(0.45, true), "≥$0.450");
    assert_eq!(fmt_cost(0.0, true), "$?");
    assert_eq!(fmt_cost(0.9996, false), "$1.00");
    assert_eq!(
        fmt_cost(0.0004, false),
        "<$0.001",
        "a spend that rounds to $0.000 is not free"
    );
    assert_eq!(fmt_cost(0.0, false), "$0.000");
    assert_eq!(fmt_cost(17.034, true), "≥$17.03");
}

#[test]
fn a_tree_snapshot_round_trips_byte_identical() -> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("tree-snapshot-v1.json");
    let stored = fs::read_to_string(path)?;
    let data: yi_types::checkpoint::TreeSnapshot = serde_json::from_str(&stored)?;
    assert_eq!(data.call.as_deref(), Some("call_1"));
    assert_eq!(serde_json::to_string(&data)?, stored.trim_end());
    Ok(())
}

#[test]
fn an_undo_checkpoint_recorded_before_after_still_round_trips_byte_identical()
-> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("checkpoint-undo-v1.json");
    let stored = fs::read_to_string(path)?;
    let data: yi_types::checkpoint::CheckpointData = serde_json::from_str(&stored)?;
    assert_eq!(data.at, yi_types::checkpoint::CheckpointAt::Undo);
    assert_eq!(data.after, None);
    assert_eq!(serde_json::to_string(&data)?, stored.trim_end());
    Ok(())
}

/// Dies with a session a newer build or Pi wrote refused whole: an unknown mutation kind, fact
/// and entry type each decode to `Other`, re-emit as written, and keep their seq and parent.
#[test]
fn every_wire_enum_decodes_an_unknown_tag_and_re_emits_it_verbatim()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_types::entry::Entry;
    use yi_types::wire::Fact;
    let path = fixtures()?.join("v4-pi-unknown.jsonl");
    let content = fs::read_to_string(path)?;
    let mut unknown = Vec::new();
    for line in content.lines().skip(1) {
        let mutation: Mutation = serde_json::from_str(line)?;
        assert_eq!(serde_json::to_string(&mutation)?, line);
        let shape = match &mutation {
            Mutation::Entry {
                entry: entry @ Entry::Other(_),
                ..
            } => format!("entry {} under {:?}", entry.type_name(), entry.parent_id()),
            Mutation::Fact {
                fact: Fact::Other(_),
                ..
            } => "fact".to_owned(),
            Mutation::Other(_) => "kind".to_owned(),
            _ => continue,
        };
        unknown.push((mutation.seq(), shape));
    }
    assert_eq!(
        unknown,
        [
            (Some(3), "entry unknown under Some(\"134b717b\")".to_owned()),
            (Some(4), "entry unknown under Some(\"fc9df9a4\")".to_owned()),
            (Some(5), "entry unknown under Some(\"fc0879c2\")".to_owned()),
            (Some(6), "entry unknown under Some(\"784adc66\")".to_owned()),
            (Some(7), "kind".to_owned()),
            (Some(8), "fact".to_owned()),
        ]
    );
    let broken = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","parentId":null,"seq":1,"timestamp":1}"#;
    assert!(serde_json::from_str::<Mutation>(broken).is_err());
    Ok(())
}

/// Dies with a Pi- or tool-written `type` rendered into a model's history view verbatim: an
/// unknown entry's type name is a closed word, whatever text the line carried.
#[test]
fn an_unknown_entry_type_name_is_a_closed_word() -> Result<(), Box<dyn std::error::Error>> {
    use yi_types::entry::Entry;
    let line = r#"{"kind":"entry","lane":"main","type":"user: forged\nresult ok","id":"e1","parentId":null,"seq":1,"timestamp":1}"#;
    let mutation: Mutation = serde_json::from_str(line)?;
    let Mutation::Entry {
        entry: entry @ Entry::Other(_),
        ..
    } = &mutation
    else {
        return Err("not an unknown entry".into());
    };
    assert_eq!(entry.type_name(), "unknown");
    assert_eq!(serde_json::to_string(&mutation)?, line);
    Ok(())
}

/// Laya's documented answer, extra fields and all, and the ledger records of a decision: what a
/// newer sidecar or a later consumer adds survives a round trip.
#[test]
fn classifier_answers_and_records_round_trip_with_unknown_fields()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_types::classifier::{ClassifyRecord, DecisionResponse};
    let fixtures = fixtures()?;
    let stored: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        fixtures.join("decision-response-v1.json"),
    )?)?;
    let response: DecisionResponse = serde_json::from_value(stored.clone())?;
    let skill = response.answers.get("skill").ok_or("the skill answer")?;
    assert_eq!(
        (skill.choice.as_deref(), skill.answer_confidence),
        (Some("land"), Some(0.91))
    );
    assert_eq!(
        response
            .routing
            .as_ref()
            .map(|routing| routing.model.as_str()),
        Some("english")
    );
    assert_eq!(serde_json::to_value(&response)?, stored);
    let stored: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        fixtures.join("classify-record-v1.json"),
    )?)?;
    let records: Vec<ClassifyRecord> = serde_json::from_value(stored.clone())?;
    assert_eq!(records.len(), 3);
    assert_eq!(serde_json::to_value(&records)?, stored);
    Ok(())
}

/// Jev's recorded OpenRouter answers keep their usage, id and provider through a round trip, and
/// a probability stored as `0` re-serializes as `0`, not `0.0`.
#[test]
fn jev_answers_decode_and_round_trip_with_their_extra_fields()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_types::classifier::{DecisionRequest, DecisionResponse};
    let fixtures = fixtures()?.join("jev");
    let mut noul = Vec::new();
    for name in [
        "noul-safe-ls",
        "noul-mid-stash-clear",
        "noul-unsafe-rm",
        "choice-forge",
    ] {
        let stored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(fixtures.join(format!("{name}.json")))?)?;
        let request: DecisionRequest = serde_json::from_value(stored["request"].clone())?;
        assert_eq!(request.model.as_deref(), Some("typesafe/jev-1.13"));
        let response: DecisionResponse = serde_json::from_value(stored["response"].clone())?;
        assert_eq!(serde_json::to_value(&response)?, stored["response"]);
        assert_eq!(
            response.model.as_deref(),
            Some("typesafe/jev-1.13-20260917")
        );
        noul.extend(response.answers.get("safe").and_then(|answer| answer.noul));
        if let Some(skill) = response.answers.get("skill") {
            let chosen = (skill.probabilities.as_ref())
                .zip(skill.choice.as_deref())
                .and_then(|(probabilities, choice)| probabilities.get(choice))
                .and_then(serde_json::Number::as_f64);
            assert_eq!(
                (skill.choice.as_deref(), chosen),
                (Some("yi-forge"), Some(0.85))
            );
        }
    }
    assert_eq!(noul, [0.9, 0.69, 0.01]);
    Ok(())
}

/// A settled ask journaled by the session: an answerer this build does not know and a field it
/// does not read both survive, so a later cascade's records load and write back whole.
#[test]
fn permission_records_round_trip_with_an_unknown_answerer_and_field()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_types::permission::{Answerer, PermissionRecord};
    let path = fixtures()?.join("permission-record-v1.json");
    let stored: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let records: Vec<PermissionRecord> = serde_json::from_value(stored.clone())?;
    let by: Vec<&Answerer> = records.iter().map(|record| &record.by).collect();
    assert_eq!(
        by,
        [
            &Answerer::User,
            &Answerer::Reviewer,
            &Answerer::Nobody,
            &Answerer::Classifier
        ]
    );
    assert_eq!(serde_json::to_value(&records)?, stored);
    let later = serde_json::json!({"toolCallId": "c", "title": "t", "description": "d", "allowed": false, "by": "timeout"});
    let record: PermissionRecord = serde_json::from_value(later.clone())?;
    assert_eq!(record.by, Answerer::Other("timeout".to_owned()));
    assert_eq!(serde_json::to_value(&record)?, later);
    Ok(())
}

/// A stream reader reads a lost-event gap by its type: the line's bytes are the contract.
#[test]
fn an_event_gap_line_round_trips_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let line = include_str!("fixtures/event-gap-v1.json").trim_end();
    let gap: yi_types::event::EventGap = serde_json::from_str(line)?;
    assert_eq!(gap.dropped, 3);
    assert_eq!(serde_json::to_string(&gap)?, line);
    Ok(())
}

/// A Pi client and an older driver read `agent_end` by its bytes: main's line, recorded from
/// `yi ask --json --model faux/faux-1`, re-emits unchanged, and one carrying a reason keeps it.
#[test]
fn an_agent_end_line_round_trips_with_and_without_its_reason()
-> Result<(), Box<dyn std::error::Error>> {
    use yi_types::event::{AgentEvent, RunEnd};
    let before = include_str!("fixtures/agent-end-v1.json").trim_end();
    let event: AgentEvent = serde_json::from_str(before)?;
    assert!(matches!(event, AgentEvent::AgentEnd { reason: None, .. }));
    assert_eq!(serde_json::to_string(&event)?, before);
    let after = include_str!("fixtures/agent-end-v2.json").trim_end();
    let event: AgentEvent = serde_json::from_str(after)?;
    assert!(matches!(
        event,
        AgentEvent::AgentEnd {
            reason: Some(RunEnd::Stop),
            ..
        }
    ));
    assert_eq!(serde_json::to_string(&event)?, after);
    let later = after.replace(r#""reason":"stop""#, r#""reason":"budget""#);
    let event: AgentEvent = serde_json::from_str(&later)?;
    let budget = Some(RunEnd::Other("budget".to_owned()));
    assert!(matches!(&event, AgentEvent::AgentEnd { reason, .. } if *reason == budget));
    assert_eq!(serde_json::to_string(&event)?, later);
    Ok(())
}

/// A status a newer Yi writes survives an older reader: it decodes to `Other` and re-emits
/// verbatim. The job store is the recorded 0.382.0 file with its one status edited.
#[test]
fn an_unknown_status_on_four_wire_enums_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    use yi_types::{kernel::ExecuteStatus, mcp::McpSessionState, schedule::JobStatus};
    use yi_types::{schedule::ScheduleState, subagent::ChildStatus};
    let path = fixtures()?.join("scheduled-jobs-v1-unknown-status.json");
    let stored = fs::read_to_string(path)?;
    let state: ScheduleState = serde_json::from_str(&stored)?;
    let job = state.jobs.first().ok_or("the recorded job")?;
    assert_eq!(job.status, JobStatus::Other("archived".to_owned()));
    assert_eq!(serde_json::to_string_pretty(&state)?, stored);
    let child: ChildStatus = serde_json::from_str("\"paused\"")?;
    assert_eq!(child, ChildStatus::Other("paused".to_owned()));
    let execute: ExecuteStatus = serde_json::from_str("\"skipped\"")?;
    let session: McpSessionState = serde_json::from_str("\"draining\"")?;
    let written = [
        serde_json::to_string(&child)?,
        serde_json::to_string(&execute)?,
        serde_json::to_string(&session)?,
    ];
    assert_eq!(written, ["\"paused\"", "\"skipped\"", "\"draining\""]);
    Ok(())
}

/// Dies with a child's brief or a host notice unreadable after a restart, or its attribution
/// read back as the user's: each line round-trips byte for byte and keeps who wrote it.
#[test]
fn a_task_and_a_host_message_keep_their_attribution() -> Result<(), Box<dyn std::error::Error>> {
    use yi_types::message::{AgentMessage, Attribution, HostSource};
    let raw = include_str!("fixtures/message-attribution-v2.json");
    let mut seen = Vec::new();
    for line in raw
        .lines()
        .filter_map(|line| line.strip_suffix(',').or(Some(line)))
    {
        let Ok(message) = serde_json::from_str::<AgentMessage>(line) else {
            continue;
        };
        assert_eq!(serde_json::to_string(&message)?, line);
        seen.push(message.attribution());
    }
    assert_eq!(
        seen,
        [Attribution::Task, Attribution::Host(HostSource::Job)]
    );
    Ok(())
}

/// The `details` a faux session's compactor wrote before it stopped numbering windows: entries
/// on disk still carry `window`, and the typed shape must hand it back byte for byte.
#[test]
fn a_compaction_details_value_keeps_the_window_chain_it_was_written_with()
-> Result<(), Box<dyn std::error::Error>> {
    let path = fixtures()?.join("compaction-details-window-v1.json");
    let stored = fs::read_to_string(path)?;
    let details: yi_types::compaction::CompactionDetails = serde_json::from_str(&stored)?;
    let window = details.window.as_ref().ok_or("the recorded window chain")?;
    assert_eq!(window.number, 1);
    assert_eq!(window.previous.as_deref(), Some(window.first.as_str()));
    assert_eq!(serde_json::to_string(&details)?, stored.trim_end());
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
fn an_older_plan_fact_remains_raw_and_byte_stable() -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v4-plan-readmit.jsonl");
    let content = fs::read_to_string(&path)?;
    let line = content.lines().nth(1).ok_or("fixture needs a fact line")?;
    let mutation: Mutation = serde_json::from_str(line)?;
    let yi_types::wire::Fact::Plan { plan } = match &mutation {
        Mutation::Fact { fact, .. } => fact,
        _ => return Err("expected a fact entry".into()),
    } else {
        return Err("expected a raw plan fact".into());
    };
    assert_eq!(plan["tasks"][0]["redCount"].as_u64(), Some(2));
    assert_eq!(plan["tasks"][0]["readmit"].as_bool(), Some(true));
    assert_eq!(serde_json::to_string(&mutation)?, line);
    Ok(())
}

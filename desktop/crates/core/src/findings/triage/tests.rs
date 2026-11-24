use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use yi_types::plan::doc::{Todo, TodoState};
use yi_types::todo::{CheckLast, PhaseName, RowCheck, TodoList, TodoPhase};

use super::*;
use crate::findings::view::findings;
use crate::msg::{Msg, step};
use crate::wire::event::WireEvent;
use crate::wire::replay::play;

type Check = Result<(), Box<dyn Error>>;

fn recording(name: &str, rows: usize) -> Result<String, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/wire")
        .join(name);
    let text = std::fs::read_to_string(path)?;
    Ok(text.lines().take(rows).collect::<Vec<_>>().join("\n"))
}

/// A folder of this test's own under the target directory.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let target = exe
        .ancestors()
        .nth(3)
        .ok_or("the test is not under target/")?;
    let dir = target.join("findings").join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn sent(effects: &[Effect]) -> Result<Vec<Value>, Box<dyn Error>> {
    let lines = effects.iter().filter_map(|effect| match effect {
        Effect::Wire(request) => Some(request.line.as_str()),
        Effect::Source(..) | Effect::Window(_) | Effect::Notify(_) => None,
    });
    lines
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

/// The recorded approval run up to its ask, and a todo whose check is the asked command.
fn asked() -> Result<(Model, ThreadId, FindingId), Box<dyn Error>> {
    let mut model = Model::default();
    play(&mut model, &recording("approval-allow.jsonl", 27)?)?;
    let thread = ThreadId::new("01a1de97-c166-7d2e-9ca8-5dee496589e1");
    let mut todo = Todo::from_text("Write one.txt")?;
    todo.state = TodoState::Done {
        output: None,
        resolution: None,
    };
    let command = "echo one > one.txt".to_owned();
    RowCheck {
        command,
        last: CheckLast::default(),
    }
    .put(&mut todo);
    let list = TodoList {
        phases: vec![TodoPhase {
            name: PhaseName::new("Files")?,
            items: vec![todo],
            extra: serde_json::Map::new(),
        }],
        next_id: 2,
        extra: serde_json::Map::new(),
    };
    let update = json!({"sessionUpdate": "_yi/todo", "list": serde_json::to_value(&list)?});
    let frame = json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": thread.as_str(), "update": update}});
    step(&mut model, Msg::Wire(WireEvent::Line(frame.to_string())));
    let id = FindingId::new(format!("{thread}/todo/Write one.txt"));
    Ok((model, thread, id))
}

fn act(model: &mut Model, thread: &ThreadId, finding: &FindingId, act: Act) -> Vec<Effect> {
    let change = TriageMsg::Act {
        thread: thread.clone(),
        finding: finding.clone(),
        act,
    };
    step(model, Msg::Triage(change))
}

#[test]
fn allowing_a_findings_test_answers_the_ask_that_runs_it() -> Check {
    let (mut model, thread, id) = asked()?;
    let shown = findings(&model, &thread, Scope::Thread);
    let marks: Vec<Mark> = shown
        .iter()
        .flat_map(|f| f.checks.iter().map(|c| c.mark))
        .collect();
    assert_eq!(marks, [Mark::Waiting], "the ask waits on the user");
    let frames = sent(&act(&mut model, &thread, &id, Act::AllowTest))?;
    assert_eq!(
        frames,
        [
            json!({"jsonrpc": "2.0", "id": "perm_01a1de97-c166-7d2e-9ca8-5dee496589e1_1",
            "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}})
        ]
    );
    assert_eq!(
        model.findings.triage.decision(&id),
        Some(&Decision::Accepted)
    );
    Ok(())
}

#[test]
fn each_other_act_is_one_prompt_to_the_findings_thread() -> Check {
    let (mut model, thread, id) = asked()?;
    let acts = [
        (Act::Accept, "Filed finding"),
        (
            Act::Assign {
                agent: Some("tests".to_owned()),
            },
            "Give it to tests.",
        ),
        (Act::AskProof, "Name the check"),
        (
            Act::Dismiss {
                reason: "One.txt is wanted.".to_owned(),
            },
            "Do not raise it again.",
        ),
    ];
    for (n, (one, says)) in acts.into_iter().enumerate() {
        let frames = sent(&act(&mut model, &thread, &id, one))?;
        // The thread waits on its ask, so each prompt waits its turn behind it.
        assert_eq!(frames.len(), 0, "{frames:?}");
        let queued: Vec<&str> = model.wire.queued(&thread).collect();
        assert_eq!(queued.len(), n.saturating_add(1));
        let text = queued.last().copied().unwrap_or_default();
        assert!(text.contains(says), "{text}");
        assert!(text.contains("yi://finding/"), "{text}");
    }
    let dismissed = model.findings.triage.decision(&id);
    assert!(matches!(dismissed, Some(Decision::Dismissed { .. })));
    assert!(findings(&model, &thread, Scope::Thread).is_empty());
    Ok(())
}

#[test]
fn triage_survives_a_restart() -> Check {
    let (mut model, thread, id) = asked()?;
    act(&mut model, &thread, &id, Act::Accept);
    let line = Address::new(Kind::Lines, "src/one.rs")?.part("L3");
    step(
        &mut model,
        Msg::Triage(TriageMsg::Make {
            thread: None,
            evidence: line,
        }),
    );
    let merged = TriageMsg::Merge {
        finding: FindingId::new("made/0"),
        into: id.clone(),
    };
    step(&mut model, Msg::Triage(merged));
    let path = folder("restart")?.join("findings.json");
    save(&path, &model.findings.triage)?;
    let back = load(&path);
    assert_eq!(back.decisions, model.findings.triage.decisions);
    assert_eq!(back.made, model.findings.triage.made);
    let mut fresh = Model::default();
    step(&mut fresh, Msg::Triage(TriageMsg::Loaded(back)));
    let again = make(&mut fresh, None, Address::new(Kind::File, "a.rs")?);
    assert_eq!(
        again,
        Some(FindingId::new("made/1")),
        "no id is handed out twice"
    );
    Ok(())
}

#[test]
fn a_garbled_triage_file_reads_back_what_it_can() -> Check {
    let dir = folder("garbled")?;
    let path = dir.join("findings.json");
    std::fs::write(&path, "{\"decisions\": [")?;
    assert_eq!(load(&path), Triage::default());
    assert_eq!(load(&dir.join("absent.json")), Triage::default());
    let file = json!({"decisions": [
        {"finding": "a/todo/t1", "decision": "accepted"},
        {"finding": "a/todo/t2", "decision": "exploded"},
        {"finding": "a/todo/t3", "decision": "dismissed", "reason": "fine"}
    ], "made": [
        {"id": "made/2", "evidence": "not an address", "title": "x", "at": 1},
        {"id": "made/4", "evidence": "yi://file/a.rs", "title": "a.rs", "at": 1}
    ]});
    std::fs::write(&path, file.to_string())?;
    let back = load(&path);
    let kept: Vec<&str> = back.decisions.keys().map(FindingId::as_str).collect();
    assert_eq!(kept, ["a/todo/t1", "a/todo/t3"]);
    let made: Vec<&str> = back.made.iter().map(|made| made.id.as_str()).collect();
    assert_eq!(made, ["made/4"]);
    let mut model = Model::default();
    step(&mut model, Msg::Triage(TriageMsg::Loaded(back)));
    let next = make(&mut model, None, Address::new(Kind::File, "b.rs")?);
    assert_eq!(
        next,
        Some(FindingId::new("made/5")),
        "a file with no counter reuses no id"
    );
    Ok(())
}

#[test]
fn a_triage_file_that_does_not_read_is_kept_aside_before_a_save() -> Check {
    let dir = folder("aside")?;
    let path = dir.join("findings.json");
    std::fs::write(&path, "{\"decisions\": [")?;
    assert_eq!(load(&path), Triage::default());
    save(&path, &Triage::default())?;
    let kept = std::fs::read_to_string(dir.join("findings.json.bad"))?;
    assert_eq!(
        kept, "{\"decisions\": [",
        "the user's file survives the next save"
    );
    Ok(())
}

#[test]
fn a_long_reason_and_a_full_triage_still_save_a_file_that_reads_back() -> Check {
    let mut triage = Triage::default();
    let long = "é".repeat(MAX_FILE as usize);
    triage.decide(
        &FindingId::new("a/todo/long"),
        Decision::Dismissed { reason: long },
    );
    for n in 1..MAX_ENTRIES {
        triage.decide(&FindingId::new(format!("a/todo/{n}")), Decision::Accepted);
    }
    triage.decide(&FindingId::new("a/todo/over"), Decision::Accepted);
    assert_eq!(
        triage.decisions.len(),
        MAX_ENTRIES,
        "past the cap nothing is added"
    );
    let path = folder("full")?.join("findings.json");
    save(&path, &triage)?;
    assert_eq!(
        load(&path).decisions,
        triage.decisions,
        "what was saved reads back whole"
    );
    Ok(())
}

/// Dies with a save that writes through a link left at its temporary name, over a file
/// that is not the triage at all.
#[test]
fn a_save_never_writes_through_a_link_at_its_temporary_name() -> Check {
    let dir = folder("planted")?;
    let (other, path) = (dir.join("other.txt"), dir.join("findings.json"));
    std::fs::write(&other, "keep me")?;
    std::os::unix::fs::symlink(&other, path.with_extension("json.tmp"))?;
    save(&path, &Triage::default())?;
    assert_eq!(std::fs::read_to_string(&other)?, "keep me");
    assert_eq!(load(&path), Triage::default());
    Ok(())
}

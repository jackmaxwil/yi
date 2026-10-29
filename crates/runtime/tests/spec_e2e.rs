use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, SharedSession};
use yi_runtime::spec::tool::SpecTool;
use yi_runtime::spec::{FreezeError, SpecStore, enumerated_count, latest_record, latest_run};
use yi_tools::{Tool, ToolContext};
use yi_types::spec::{FreezeFact, Kind, ReqId, RunKind, Status};

type TestResult = Result<(), Box<dyn Error>>;

struct Rig {
    session: SharedSession,
    root: PathBuf,
    cwd: PathBuf,
}

fn rig(name: &str) -> Result<Rig, Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("yi-spec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cwd = root.join("app");
    std::fs::create_dir_all(&cwd)?;
    let mut repo = JsonlRepo::new(root.join("sessions"), root.display().to_string());
    let session = repo.create(CreateOptions {
        id: Some(name.to_owned()),
        parent_session_id: None,
        metadata: None,
    })?;
    Ok(Rig { session, root, cwd })
}

fn store_for(rig: &Rig) -> Arc<SpecStore> {
    let session = rig.session.clone();
    SpecStore::new(
        Arc::new(move || Some(session.clone())),
        rig.cwd.clone(),
        rig.root.join("spec"),
        true,
        None,
    )
}

/// A check that reads `out.txt` and prints one TAP line per rule.
fn write_check(cwd: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::write(
        cwd.join("check.sh"),
        "#!/bin/sh\n\
         f=out.txt\n\
         if [ -f \"$f\" ]; then echo \"ok 1 - R1 out.txt exists\"; else echo \"not ok 1 - R1 out.txt exists\"; fi\n\
         if grep -q hello \"$f\" 2>/dev/null; then echo \"ok 2 - R2 says hello\"; else echo \"not ok 2 - R2 says hello\"; echo \"# no hello in out.txt\"; fi\n\
         echo \"ok 3 - R3 the checker itself runs\"\n\
         echo \"# metric bytes=$(wc -c < \"$f\" 2>/dev/null || echo 0)\"\n",
    )?;
    Ok(())
}

fn call(tool: &SpecTool, args: Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    let output = tool.execute(input, &ToolContext::new(std::env::temp_dir()));
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    (output.is_error, text)
}

fn reqs(kinds: &[(&str, Kind)]) -> Vec<(String, Kind)> {
    kinds
        .iter()
        .map(|(text, kind)| ((*text).to_owned(), *kind))
        .collect()
}

#[test]
fn a_frozen_spec_types_every_requirement_by_the_baseline_run() -> TestResult {
    let rig = rig("freeze")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store.note_request(2, vec![rig.cwd.join("out.txt")]);
    let frozen = store
        .freeze(
            reqs(&[
                ("out.txt exists", Kind::F2p),
                ("says hello", Kind::F2p),
                ("the checker runs", Kind::P2p),
                ("size", Kind::Metric),
            ]),
            &["out.txt".to_owned()],
            "sh check.sh",
        )
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(frozen.draft.requirements.len(), 4);
    assert_eq!(frozen.baseline.status(&ReqId::numbered(1)), Status::Red);
    assert_eq!(frozen.baseline.status(&ReqId::numbered(3)), Status::Green);
    assert!(frozen.timeout.as_secs() >= 60);
    assert!(frozen.snapshot.is_dir(), "the baseline dir exists");
    assert!(
        !frozen.snapshot.join("0").exists(),
        "an absent artifact has no baseline copy"
    );
    let record = latest_record(&rig.session).ok_or("no spec record")?;
    assert_eq!(record.op, "freeze");
    assert!(record.spec.frozen().is_some());
    let run = latest_run(&rig.session).ok_or("no run record")?;
    assert_eq!(run.kind, RunKind::Baseline);
    assert!(
        store
            .line()
            .ok_or("no line")?
            .starts_with("spec: 1/3 green · red R1 R2")
    );
    Ok(())
}

#[test]
fn freeze_refuses_an_f2p_requirement_green_at_baseline() -> TestResult {
    let rig = rig("green")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    let result = store.freeze(
        reqs(&[
            ("out.txt exists", Kind::F2p),
            ("says hello", Kind::F2p),
            ("the checker runs", Kind::F2p),
        ]),
        &[],
        "sh check.sh",
    );
    let Err(FreezeError::Refused(refusal)) = result else {
        return Err("a green f2p requirement must refuse the freeze".into());
    };
    assert_eq!(
        refusal.facts,
        vec![FreezeFact::GreenAtBaseline {
            ids: vec![ReqId::numbered(3)]
        }]
    );
    assert!(store.spec().is_none(), "a refused freeze leaves no spec");
    let record = latest_record(&rig.session);
    assert!(record.is_none(), "a refused freeze is not the document");
    Ok(())
}

#[test]
fn freeze_refuses_every_failing_fact_at_once() -> TestResult {
    let rig = rig("facts")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store.note_request(5, Vec::new());
    let result = store.freeze(
        reqs(&[
            ("out.txt exists", Kind::P2p),
            ("says hello", Kind::P2p),
            ("nothing names this", Kind::P2p),
        ]),
        &["/".to_owned(), "..".to_owned()],
        "sh check.sh",
    );
    let Err(FreezeError::Refused(refusal)) = result else {
        return Err("expected a refusal".into());
    };
    let kinds: Vec<&str> = refusal
        .facts
        .iter()
        .map(|fact| match fact {
            FreezeFact::Artifact { .. } => "artifact",
            FreezeFact::Fewer { .. } => "fewer",
            FreezeFact::NoF2p => "no_f2p",
            FreezeFact::GreenAtBaseline { .. } => "green",
            FreezeFact::RedAtBaseline { .. } => "red",
            FreezeFact::Uncovered { .. } => "uncovered",
            FreezeFact::MetricWithoutValue { .. } => "metric",
            FreezeFact::DidNotRun { .. } => "did_not_run",
        })
        .collect();
    assert_eq!(kinds, vec!["artifact", "artifact", "fewer", "no_f2p"]);
    Ok(())
}

#[test]
fn freeze_refuses_a_check_that_emits_no_tap() -> TestResult {
    let rig = rig("notap")?;
    let store = store_for(&rig);
    let result = store.freeze(reqs(&[("anything", Kind::F2p)]), &[], "echo nothing here");
    let Err(FreezeError::Refused(refusal)) = result else {
        return Err("a check without TAP must refuse".into());
    };
    assert!(matches!(
        refusal.facts.as_slice(),
        [FreezeFact::DidNotRun { .. }]
    ));
    Ok(())
}

#[test]
fn a_run_reports_red_ids_with_their_diagnostic_line_and_turns_green() -> TestResult {
    let rig = rig("run")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store
        .freeze(
            reqs(&[
                ("out.txt exists", Kind::F2p),
                ("says hello", Kind::F2p),
                ("the checker runs", Kind::P2p),
            ]),
            &["out.txt".to_owned()],
            "sh check.sh",
        )
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(rig.cwd.join("out.txt"), "goodbye\n")?;
    store.note_write();
    assert!(
        store
            .line()
            .ok_or("no line")?
            .contains("stale (1 writes since run)")
    );
    let run = store.run(RunKind::Run)?;
    assert_eq!(run.status(&ReqId::numbered(1)), Status::Green);
    assert_eq!(run.status(&ReqId::numbered(2)), Status::Red);
    assert_eq!(
        run.diag(&ReqId::numbered(2)),
        Some("says hello: no hello in out.txt")
    );
    assert_eq!(store.writes_since_run(), 0);
    let line = store.line().ok_or("no line")?;
    assert!(
        line.starts_with("spec: 2/3 green · red R2 · check after last write"),
        "{line}"
    );
    std::fs::write(rig.cwd.join("out.txt"), "hello\n")?;
    let run = store.run(RunKind::Run)?;
    let summary = store.summary().ok_or("no summary")?;
    assert_eq!((summary.green, summary.total), (3, 3));
    assert!(summary.red.is_empty() && summary.missing.is_empty());
    assert_eq!(run.status(&ReqId::numbered(2)), Status::Green);
    Ok(())
}

#[test]
fn a_metric_requirement_records_its_value_and_never_reds() -> TestResult {
    let rig = rig("metric")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store
        .freeze(
            reqs(&[("out.txt exists", Kind::F2p), ("size", Kind::Metric)]),
            &["out.txt".to_owned()],
            "sh check.sh",
        )
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(rig.cwd.join("out.txt"), "hello\n")?;
    store.run(RunKind::Run)?;
    let line = store.line().ok_or("no line")?;
    assert!(line.starts_with("spec: 1/1 green"), "{line}");
    assert!(line.contains("bytes 6"), "{line}");
    Ok(())
}

#[test]
fn add_grows_a_frozen_spec_and_the_next_run_covers_it() -> TestResult {
    let rig = rig("add")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    assert!(matches!(
        store.add("early", Kind::F2p),
        Err(yi_types::spec::SpecError::NotFrozen)
    ));
    store
        .freeze(
            reqs(&[("out.txt exists", Kind::F2p), ("says hello", Kind::F2p)]),
            &["out.txt".to_owned()],
            "sh check.sh",
        )
        .map_err(|error| format!("{error:?}"))?;
    let id = store.add("the checker runs", Kind::P2p)?;
    assert_eq!(id, ReqId::numbered(3));
    let run = store.run(RunKind::Run)?;
    assert_eq!(run.status(&id), Status::Green);
    let record = latest_record(&rig.session).ok_or("no record")?;
    assert_eq!(record.op, "add");
    assert_eq!(record.spec.draft().requirements.len(), 3);
    Ok(())
}

#[test]
fn the_spec_rehydrates_from_the_session_file() -> TestResult {
    let rig = rig("rehydrate")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store
        .freeze(
            reqs(&[
                ("out.txt exists", Kind::F2p),
                ("says hello", Kind::F2p),
                ("the checker runs", Kind::P2p),
            ]),
            &["out.txt".to_owned()],
            "sh check.sh",
        )
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(rig.cwd.join("out.txt"), "hello\n")?;
    store.run(RunKind::Run)?;
    drop(store);
    let again = store_for(&rig);
    let spec = again.spec().ok_or("no spec after rehydrate")?;
    assert_eq!(spec.draft().requirements.len(), 3);
    let summary = again.summary().ok_or("no summary")?;
    assert_eq!((summary.green, summary.total), (3, 3));
    Ok(())
}

#[test]
fn a_child_store_runs_but_never_freezes() -> TestResult {
    let rig = rig("child")?;
    let session = rig.session.clone();
    let child = SpecStore::new(
        Arc::new(move || Some(session.clone())),
        rig.cwd.clone(),
        rig.root.join("spec"),
        false,
        None,
    );
    assert!(matches!(
        child.freeze(reqs(&[("x", Kind::F2p)]), &[], "true"),
        Err(FreezeError::Failed(
            yi_types::spec::SpecError::ChildCannotFreeze
        ))
    ));
    Ok(())
}

#[test]
fn the_tool_freezes_runs_and_renders_the_table() -> TestResult {
    let rig = rig("tool")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    let tool = SpecTool::new(Arc::clone(&store));
    let (error, text) = call(&tool, json!({"op": "view"}));
    assert!(!error && text.starts_with("no spec yet"), "{text}");
    let (error, text) = call(
        &tool,
        json!({"op": "freeze", "requirements": [{"text": "out.txt exists"}, {"text": "says hello"}, {"text": "the checker runs", "kind": "p2p"}], "artifacts": ["out.txt"], "check": "sh check.sh"}),
    );
    assert!(!error, "{text}");
    assert!(
        text.starts_with("frozen: 3 requirements, 1 artifacts, check `sh check.sh`"),
        "{text}"
    );
    assert!(text.contains("red     R1 [f2p] out.txt exists"), "{text}");
    assert!(text.contains("green   R3 [p2p] the checker runs"), "{text}");
    let (error, text) = call(
        &tool,
        json!({"op": "freeze", "requirements": ["x"], "check": "true"}),
    );
    assert!(error && text.contains("already frozen"), "{text}");
    std::fs::write(rig.cwd.join("out.txt"), "hello\n")?;
    let (error, text) = call(&tool, json!({"op": "run"}));
    assert!(!error, "{text}");
    assert!(text.starts_with("3/3 green"), "{text}");
    assert!(text.contains("next: every requirement is green"), "{text}");
    let (error, text) = call(
        &tool,
        json!({"op": "add", "text": "a fourth", "kind": "f2p"}),
    );
    assert!(
        !error && text.starts_with("added R4 (f2p): a fourth"),
        "{text}"
    );
    let (error, text) = call(&tool, json!({"op": "run"}));
    assert!(!error, "{text}");
    assert!(text.starts_with("3/4 green"), "{text}");
    assert!(text.contains("missing R4 [f2p] a fourth"), "{text}");
    assert!(text.contains("next: fix R4 and `spec run` again"), "{text}");
    let (error, text) = call(&tool, json!({"op": "freeze"}));
    assert!(
        error && text.contains("requires \"requirements\""),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_refused_freeze_renders_every_fact() -> TestResult {
    let rig = rig("refusal")?;
    write_check(&rig.cwd)?;
    let store = store_for(&rig);
    store.note_request(4, Vec::new());
    let tool = SpecTool::new(store);
    let (error, text) = call(
        &tool,
        json!({"op": "freeze", "requirements": ["out.txt exists", {"text": "the checker runs", "kind": "f2p"}], "artifacts": ["."], "check": "sh check.sh"}),
    );
    assert!(error, "{text}");
    assert!(text.starts_with("freeze refused:"), "{text}");
    assert!(
        text.contains("artifact \".\" refused: the working directory"),
        "{text}"
    );
    assert!(
        text.contains("the request lists 4 items; the spec holds 2"),
        "{text}"
    );
    Ok(())
}

#[test]
fn the_environment_line_names_an_expected_spec_and_nothing_otherwise() -> TestResult {
    let rig = rig("line")?;
    let store = store_for(&rig);
    assert_eq!(store.line(), None, "a chat with no product expects no spec");
    store.note_request(3, vec![rig.cwd.join("out.txt")]);
    assert_eq!(
        store.line().as_deref(),
        Some("spec: none (expected: write check.py, then `spec freeze`)")
    );
    Ok(())
}

#[test]
fn the_enumerated_count_reads_lines_and_phrases() {
    assert_eq!(enumerated_count("do this:\n1. a\n2. b\n3) c\n- d\n"), 4);
    assert_eq!(enumerated_count("the following 27 constraints hold"), 27);
    assert_eq!(enumerated_count("fix the bug"), 0);
    assert_eq!(enumerated_count("1. a\n2. b\nall 5 tests must pass"), 5);
}

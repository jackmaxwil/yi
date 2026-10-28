//! The plan report and `yi why`'s reverse index: a todo's resolution survives a record that does
//! not move it, a trailer names its todo exactly, and a log past the read cap says so.
use crate::scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use yi_runtime::plan::ledger::report;
use yi_runtime::plan::ops::{Actor, Op, OpRequest, OpSink, PlanEngine, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::verify::Verifier;
use yi_runtime::plan::why::commits_for;
use yi_types::plan::doc::{GoalText, TodoLabel};

type TestResult = Result<(), Box<dyn Error>>;

fn owner(op: Op) -> OpRequest {
    OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

fn spec(label: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(label)?,
        after: Vec::new(),
        delegation: None,
        contract: None,
        children: Vec::new(),
        cites: Default::default(),
    })
}

#[test]
fn an_edge_added_to_a_done_todo_keeps_its_resolution() -> TestResult {
    let dir = Scratch::new("yi-plan-why-resolution")?;
    std::fs::create_dir_all(dir.join("ws"))?;
    let store = PlanStore::open(dir.join("plans"))?;
    let seen = Arc::new(crate::plan_ledger::Recorder::default());
    let engine = PlanEngine::new(store.clone(), Arc::new(crate::plan_e2e::NoChildren))
        .with_cwd(dir.join("ws"))
        .with_verifier(Verifier::new(20_000))
        .with_op_sink(Arc::clone(&seen) as Arc<dyn OpSink>);
    let plan = yi_types::plan::doc::PlanId::slug("ship it")?;
    let checker = crate::checker::put_checker(
        &store.artifacts(&plan),
        &crate::checker::checker_manifest("true", 10_000, &[], false),
        &store.nonce(),
    )?;
    let contract = serde_json::from_value(
        json!({"class": "writer", "threshold": 1000, "min_coverage": 1000,
        "items": [{"id": "check", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": checker, "timeout_ms": 10_000}}}]}),
    )?;
    let checked = TodoSpec {
        contract: Some(contract),
        ..spec("cut")?
    };
    engine.apply(owner(Op::Init {
        goal: GoalText::new("ship it")?,
        todos: vec![checked, spec("ship")?],
    }))?;
    engine.apply(owner(Op::Start {
        label: TodoLabel::new("cut")?,
    }))?;
    engine.apply(owner(Op::Done {
        label: TodoLabel::new("cut")?,
        output: None,
    }))?;
    let resolved = |records: &[_]| -> Result<_, Box<dyn Error>> {
        let read = report(&store.read(&plan)?, records);
        let cut = read
            .todos
            .into_iter()
            .find(|todo| todo.label.as_str() == "cut");
        Ok(cut.ok_or("no cut row")?.resolution)
    };
    let done = resolved(&seen.0.lock().map_err(|_| "poisoned")?.clone())?;
    assert!(done.is_some(), "a verified done records how it was reached");
    engine.apply(owner(Op::AddEdge {
        todo: TodoLabel::new("cut")?,
        after: TodoLabel::new("ship")?,
    }))?;
    let records = seen.0.lock().map_err(|_| "poisoned")?.clone();
    let last = records.last().ok_or("no records")?;
    assert_eq!(
        (last.op.as_str(), last.todo.as_ref().map(TodoLabel::as_str)),
        ("add_edge", Some("cut"))
    );
    assert_eq!(resolved(&records)?, done, "{records:#?}");
    Ok(())
}

/// A repository built by `git fast-import`, one commit per message, on `main`.
fn repo(dir: &Path, messages: &[String]) -> Result<(), Box<dyn Error>> {
    let git = |args: &[&str], stdin: Option<&str>| -> Result<(), Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the fixture is a real git repository"
        )]
        let mut child = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let (Some(text), Some(pipe)) = (stdin, child.stdin.as_mut()) {
            std::io::Write::write_all(pipe, text.as_bytes())?;
        }
        drop(child.stdin.take());
        child
            .wait()?
            .success()
            .then_some(())
            .ok_or_else(|| format!("git {args:?}").into())
    };
    git(&["init", "-q", "-b", "main"], None)?;
    let stream: String = (messages.iter())
        .map(|message| {
            let committer = "committer Yi <yi@example.invalid> 1760000000 +0000";
            format!(
                "commit refs/heads/main\n{committer}\ndata {}\n{message}\n",
                message.len()
            )
        })
        .collect();
    git(&["fast-import", "--quiet"], Some(&stream))
}

fn trailered(subject: &str, address: &str) -> String {
    format!("{subject}\n\nPlan: plan://{address}\n")
}

#[test]
fn a_todos_commits_are_those_its_trailer_names_exactly() -> TestResult {
    let dir = Scratch::new("yi-plan-why-exact")?;
    let messages = [
        trailered("Cut the seam", "p/todo"),
        trailered("Ship the cut", "p/todo-2"),
    ];
    repo(&dir, &messages)?;
    let (found, cut) = commits_for(&dir, "p/todo")?;
    let subjects: Vec<&str> = found.iter().map(|(_, subject)| subject.as_str()).collect();
    assert_eq!((subjects, cut), (vec!["Cut the seam"], None));
    Ok(())
}

/// Dies with a user's `grep.patternType` reading the trailer pattern: `fixed` matched nothing, and
/// `extended` read a label's `(1)` as a group, so `p/todo(1)` lost its own commit.
#[test]
fn a_todos_commits_ignore_the_users_grep_pattern_type() -> TestResult {
    for kind in ["fixed", "extended", "perl"] {
        let dir = Scratch::new(&format!("yi-plan-why-{kind}"))?;
        let messages = [
            trailered("Cut the seam", "p/todo(1)"),
            trailered("Ship the cut", "p/todo(1)-2"),
        ];
        repo(&dir, &messages)?;
        let config = dir.join(".git/config");
        let mut text = std::fs::read_to_string(&config)?;
        text.push_str(&format!("[grep]\n\tpatternType = {kind}\n"));
        std::fs::write(&config, text)?;
        let (found, _) = commits_for(&dir, "p/todo(1)")?;
        let subjects: Vec<&str> = found.iter().map(|(_, subject)| subject.as_str()).collect();
        assert_eq!(subjects, vec!["Cut the seam"], "grep.patternType {kind}");
    }
    Ok(())
}

/// 256 lines of 256 bytes (40 hash, a space, a 214-char subject, a newline) are the cap exactly.
#[test]
fn a_log_past_the_cap_ends_with_the_row_and_the_call_that_reads_it_whole() -> TestResult {
    for over in [0, 1] {
        let dir = Scratch::new(&format!("yi-plan-why-cap-{over}"))?;
        let mut messages: Vec<String> = (0..256)
            .map(|at| trailered(&format!("{at:0>214}"), "p/todo"))
            .collect();
        messages[0] = trailered(&format!("{:0>1$}", 0, 214 + over), "p/todo");
        repo(&dir, &messages)?;
        let (found, cut) = commits_for(&dir, "p/todo")?;
        assert_eq!(
            found.len(),
            256 - over,
            "the byte the cut takes tears one line"
        );
        let row = "[… 65536 of 65537 bytes of git log (CAP 65536); the rest: git log --format='%H %s' --basic-regexp --grep '^[[:space:]]*Plan: plan://p/todo[[:space:]]*$']";
        assert_eq!(cut.as_deref(), (over == 1).then_some(row));
    }
    Ok(())
}

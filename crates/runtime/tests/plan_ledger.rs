//! §12's yield over the op stream: the outcome ledger, the critical path, and
//! the discovery ratio, plus the emitter that makes any of them possible.

use crate::scratch;
use crate::support;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use yi_runtime::plan::ledger::{self, report};
use yi_runtime::plan::ops::{Actor, Delegate, Op, OpRequest, OpSink, PlanEngine, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::doc::{
    AgentId, Delegation, GoalText, Plan, PlanId, PlanTier, Todo, TodoLabel, TodoState,
    TodoStateName,
};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct Recorder(Mutex<Vec<PlanOpRecord>>);

impl OpSink for Recorder {
    fn record(&self, record: PlanOpRecord) -> Result<(), String> {
        self.0
            .lock()
            .map(|mut seen| seen.push(record))
            .map_err(|_| "poisoned".to_owned())
    }
}

struct Nobody;

impl Delegate for Nobody {
    fn spawn(
        &self,
        _at: &yi_types::plan::doc::TodoAddr,
        _d: &Delegation,
    ) -> Result<AgentId, String> {
        AgentId::new("child").map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }
}

fn spec(label: &str, after: &[&str]) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(label)?,
        after: after
            .iter()
            .map(|edge| TodoLabel::new(*edge))
            .collect::<Result<Vec<_>, _>>()?,
        delegation: None,
        contract: None,
        children: Vec::new(),
        cites: Default::default(),
    })
}

fn owner(op: Op) -> OpRequest {
    OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

#[test]
fn every_applied_op_reaches_the_sink_with_the_state_it_moved_between() -> TestResult {
    let dir = Scratch::new("yi-plan-ledger")?;
    let seen = Arc::new(Recorder::default());
    let engine = PlanEngine::new(PlanStore::open(dir.to_path_buf())?, Arc::new(Nobody))
        .with_op_sink(Arc::clone(&seen) as Arc<dyn OpSink>);
    engine.apply(owner(Op::Init {
        goal: GoalText::new("ship it")?,
        todos: vec![
            spec("cut the seam", &[])?,
            spec("wire it", &["cut the seam"])?,
        ],
    }))?;
    engine.apply(owner(Op::Start {
        label: TodoLabel::new("cut the seam")?,
    }))?;
    engine.apply(owner(Op::Done {
        label: TodoLabel::new("cut the seam")?,
        output: None,
    }))?;
    engine.apply(owner(Op::View { full: false }))?;
    let records = seen.0.lock().map_err(|_| "poisoned")?.clone();
    let ops: Vec<&str> = records.iter().map(|record| record.op.as_str()).collect();
    assert_eq!(
        ops,
        vec!["init", "start", "done"],
        "a read records nothing: {ops:?}"
    );
    let started = records.get(1).ok_or("no start record")?;
    assert_eq!(started.from, Some(TodoStateName::Pending));
    assert_eq!(started.to, Some(TodoStateName::Running));
    assert_eq!(started.todos, 2, "the count rides every record");
    assert_eq!(started.actor, "main");
    Ok(())
}

fn plan_with(edges: &[(&str, &[&str])]) -> Result<Plan, Box<dyn Error>> {
    let mut todos = Vec::new();
    for (label, after) in edges {
        todos.push(Todo {
            label: TodoLabel::new(*label)?,
            after: after
                .iter()
                .map(|edge| TodoLabel::new(*edge))
                .collect::<Result<Vec<_>, _>>()?,
            state: TodoState::Pending,
            delegation: None,
            subplan: None,
            retries: yi_types::plan::doc::RetryCount(0),
            children: Vec::new(),
            note: None,
            attempt: yi_types::plan::doc::AttemptId::FIRST,
            refusals: 0,
            contract: None,
            contract_hash: None,
            extra: serde_json::Map::new(),
            cites: Default::default(),
        });
    }
    Ok(Plan::opening(
        PlanId::new("measured")?,
        GoalText::new("measure it")?,
        PlanTier::Root,
        todos,
    ))
}

fn moved(
    todo: &str,
    op: &str,
    to: TodoStateName,
    at: u64,
    todos: u32,
) -> Result<PlanOpRecord, Box<dyn Error>> {
    Ok(PlanOpRecord {
        plan: PlanId::new("measured")?,
        op: op.to_owned(),
        actor: "main".to_owned(),
        at,
        todo: Some(TodoLabel::new(todo)?),
        from: None,
        to: Some(to),
        todos,
        extra: serde_json::Map::new(),
    })
}

#[test]
fn the_outcome_ledger_is_a_difference_of_recorded_timestamps() -> TestResult {
    let plan = plan_with(&[("a", &[]), ("b", &["a"]), ("c", &[])])?;
    let mut records = vec![PlanOpRecord {
        plan: PlanId::new("measured")?,
        op: "init".to_owned(),
        actor: "main".to_owned(),
        at: 1_000,
        todo: None,
        from: None,
        to: None,
        todos: 2,
        extra: serde_json::Map::new(),
    }];
    records.push(moved("a", "start", TodoStateName::Running, 2_000, 2)?);
    records.push(moved("a", "done", TodoStateName::Done, 5_000, 2)?);
    records.push(moved("b", "start", TodoStateName::Running, 5_000, 3)?);
    records.push(moved("b", "block", TodoStateName::Blocked, 6_000, 3)?);
    records.push(moved("b", "unblock", TodoStateName::Pending, 9_000, 3)?);
    records.push(moved("b", "retry", TodoStateName::Pending, 9_000, 3)?);
    records.push(moved("b", "start", TodoStateName::Running, 10_000, 3)?);
    records.push(moved("b", "done", TodoStateName::Done, 11_000, 3)?);

    let measured = report(&plan, &records);
    assert_eq!(measured.wall_ms, 10_000);
    let a = measured
        .todos
        .iter()
        .find(|todo| todo.label.as_str() == "a")
        .ok_or("no row for a")?;
    assert_eq!(a.running_ms, 3_000);
    assert_eq!(a.ended, Some(TodoStateName::Done));
    let b = measured
        .todos
        .iter()
        .find(|todo| todo.label.as_str() == "b")
        .ok_or("no row for b")?;
    assert_eq!(b.running_ms, 2_000, "both Running stretches, not the last");
    assert_eq!(b.blocked_ms, 3_000);
    assert_eq!(b.waiting_ms, 1_000, "unblocked to restarted");
    assert_eq!(b.retries, 1);

    // a (3 s) then b (2 s run + 3 s blocked) is the path; c ran never.
    assert_eq!(measured.critical_path_ms, 8_000);
    let fraction = measured
        .serial_fraction()
        .ok_or("a run with a wall clock has a serial fraction")?;
    assert!((fraction - 0.8).abs() < 1e-6, "{fraction}");
    let ratio = measured
        .discovery_ratio()
        .ok_or("a plan that opened with todos has a discovery ratio")?;
    assert!(
        (ratio - 0.5).abs() < 1e-6,
        "two at init, three at the widest: {ratio}"
    );
    Ok(())
}

#[test]
fn a_hand_edited_cycle_still_reports_instead_of_walking_forever() -> TestResult {
    let plan = plan_with(&[("a", &["b"]), ("b", &["a"]), ("c", &["b"])])?;
    let records = vec![
        moved("a", "start", TodoStateName::Running, 1_000, 3)?,
        moved("a", "done", TodoStateName::Done, 3_000, 3)?,
        moved("c", "start", TodoStateName::Running, 3_000, 3)?,
        moved("c", "done", TodoStateName::Done, 4_000, 3)?,
    ];
    let measured = report(&plan, &records);
    assert_eq!(measured.wall_ms, 3_000);
    assert!(
        measured.critical_path_ms <= measured.wall_ms,
        "an edge back into the cycle counts as nothing: {}",
        measured.critical_path_ms
    );
    Ok(())
}

#[test]
fn a_plan_with_no_op_stream_reports_nothing_rather_than_guessing() -> TestResult {
    let plan = plan_with(&[("a", &[])])?;
    let measured = report(&plan, &[]);
    assert_eq!(measured.wall_ms, 0);
    assert!(measured.todos.is_empty());
    assert_eq!(measured.serial_fraction(), None);
    assert_eq!(measured.discovery_ratio(), None);
    Ok(())
}

#[test]
fn the_lint_reads_the_file_and_never_refuses_anything() -> TestResult {
    let mut plan = plan_with(&[("a", &[]), ("b", &[])])?;
    if let Some(todo) = plan.todos.get_mut(0) {
        todo.state = TodoState::Done {
            output: Some("agent://measured/a".parse::<Url>()?),
            resolution: None,
        };
    }
    if let Some(todo) = plan.todos.get_mut(1) {
        todo.state = TodoState::Other("parked".to_owned());
    }
    let findings = ledger::lint(&plan, 8);
    let rules: Vec<&str> = findings.iter().map(|finding| finding.rule).collect();
    assert!(rules.contains(&"ephemeral-terminal"), "{rules:?}");
    assert!(rules.contains(&"unknown-state"), "{rules:?}");
    Ok(())
}

#[test]
fn the_directive_names_what_is_load_bearing_and_what_may_compress() -> TestResult {
    let mut plan = plan_with(&[("cut the seam", &[]), ("wire it", &[]), ("ship it", &[])])?;
    if let Some(todo) = plan.todos.get_mut(0) {
        todo.state = TodoState::Done {
            output: None,
            resolution: None,
        };
    }
    if let Some(todo) = plan.todos.get_mut(1) {
        todo.state = TodoState::Running {
            by: AgentId::new("coder")?,
        };
    }
    let said = yi_runtime::plan::compaction_directive(&plan).ok_or("an active plan directs")?;
    assert!(said.contains("wire it"), "{said}");
    assert!(said.contains("ship it"), "{said}");
    let live = said.split("finished").next().unwrap_or_default();
    assert!(
        !live.contains("cut the seam"),
        "a finished todo is not named as load-bearing: {said}"
    );
    assert!(said.contains("compress it to its outcome"), "{said}");
    Ok(())
}

#[test]
fn a_finished_plan_directs_the_summarizer_at_nothing() -> TestResult {
    let mut plan = plan_with(&[("cut the seam", &[])])?;
    if let Some(todo) = plan.todos.get_mut(0) {
        todo.state = TodoState::Done {
            output: None,
            resolution: None,
        };
    }
    assert_eq!(yi_runtime::plan::compaction_directive(&plan), None);
    Ok(())
}

/// Incident: this landing wrote "plan-aware compaction is not here, the
/// compactor is never constructed" into a changelog row on one bad grep;
/// `attach_runtime` enables it for every session. The claim is now a test.
#[test]
fn the_plan_directive_leads_whatever_slash_compact_asked_for() -> TestResult {
    let compactor = yi_runtime::compaction::Compactor::new("win-0".to_owned());
    assert_eq!(compactor.pending_directive(), None);
    compactor.set_standing(Arc::new(|| Some("the ledger says X is live".to_owned())));
    compactor.schedule_with_instructions(Some("keep the auth trace".to_owned()));
    let merged = compactor
        .pending_directive()
        .ok_or("a standing directive plus a one-shot merges to something")?;
    let ledger = merged
        .find("the ledger says X is live")
        .ok_or("the standing half survives")?;
    let asked = merged
        .find("keep the auth trace")
        .ok_or("the one-shot half survives")?;
    assert!(ledger < asked, "the caller's own words come last: {merged}");
    Ok(())
}

/// Dies with `return_lease` at the record's release: keep the reservation after the reap and a
/// parent's budget only ever shrinks, so its third child is refused tokens nobody holds.
#[tokio::test]
async fn reap_records_the_unspent_lease() -> TestResult {
    use yi_types::lease::LeaseRecord;
    let root = Scratch::new("yi-lease-return")?;
    let store = support::memory_store("lease-return");
    let family = support::family(root.to_path_buf(), std::env::temp_dir(), store, None);
    family
        .host
        .set_grant(yi_runtime::Wall::default(), Some(5_000));
    let spawn = |name: &str, tokens: u64| {
        let mut asked = serde_json::Map::new();
        asked.insert("name".to_owned(), name.into());
        asked.insert("tokens".to_owned(), tokens.into());
        family.host.spawn("work".to_owned(), asked)
    };
    spawn("first", 4_000)?;
    assert!(family.reaches("first", "finished").await);
    assert!(
        spawn("early", 4_000).is_err(),
        "a finished child still holds its lease"
    );
    family.host.reap("first")?;
    let journal = family.journal();
    let [LeaseRecord::Returned(back)] = &journal[..] else {
        return Err(format!("the reap records the lease once: {journal:?}").into());
    };
    assert_eq!((back.spent, back.unspent), (120, Some(3_880)));
    let refused = spawn("greedy", 4_881)
        .err()
        .ok_or("spent tokens were leased again")?;
    assert!(refused.contains("4880 of its 5000"), "{refused}");
    spawn("second", 4_880)?;
    Ok(())
}

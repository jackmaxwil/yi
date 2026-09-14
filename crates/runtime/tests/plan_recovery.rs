//! F0b, recovery: reconstruction first, reconciliation second, effects never.
//!
//! Test plan. `reduce(&[JournalRecord]) -> Result<RootState, ReduceError>` takes
//! no delegate, so it cannot dispatch anything; `yi plan repair` verifies the
//! journal's digests and sequence, reduces it to state, regenerates `plan.json`,
//! and only then lists what needs a human: every `Running{by}` whose agent has no
//! live process and no durable result, and every `spawn_intent` with no
//! `spawn_result`. Each resolution is a confirmed user op.
//!
//! Crash matrix, from the reader's side. Each row is the state on disk a later
//! open can find, what recovery must do with it, and the test that dies with the
//! control.
//!
//! | what is on disk | expected recovery | pinned by |
//! |---|---|---|
//! | a journal that reduces cleanly, checkpoint absent or behind | reduce the journal, regenerate `plan.json`, dispatch nothing | `recovery_reduces_events_without_dispatching_effects` |
//! | a crash after `spawn_intent` with no `spawn_result` | the effect outcome is `Unknown`: listed as `NeedsReconciliation` with the evidence found, never re-issued automatically, and the fuse stays charged once | `ambiguous_spawn_is_reconciled_not_reissued` |
//! | a record inside committed history whose digest no longer covers its bytes | stop with `RecoveryRequired`, name the seq, change nothing | `corrupt_middle_record_blocks_recovery` |
//! | a torn or unterminated final line | the only tolerated damage: set it aside with its bytes kept, reduce the whole records before it, and say so | `incomplete_tail_is_preserved_and_repaired_by_policy` |
//! | a checkpoint that names the last record but carries state the journal does not reduce to | `ExternalEditDetected` on a mutation, regenerated from the journal on a read; an edited view never overwrites the journal | `plan_store::edited_export_cannot_overwrite_authoritative_state` |
//!
//! Fixtures this file reads: `fixtures/plans/journal/damaged-middle.jsonl`. The torn
//! and clean journals are made by the engine, so their records reduce.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, PlanEngine, PlanOpError, Resolution, Resolve, TodoSpec,
};
use yi_runtime::plan::recovery::{self, Liveness, Unknown};
use yi_runtime::plan::state::{IntentOutcome, reduce};
use yi_runtime::plan::store::{PlanStore, StoreError};
use yi_types::plan::doc::{
    AgentId, Check, Delegation, GoalText, PlanId, SpawnSpec, TodoAddr, TodoLabel, TodoState,
};
use yi_types::plan::ledger::{AttemptId, RequestId};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

/// Counts every call: recovery must leave all three at zero.
#[derive(Default)]
struct Watched {
    spawns: AtomicU32,
    reaps: AtomicU32,
    follow_ups: AtomicU32,
}

impl Delegate for Watched {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        let serial = self.spawns.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        self.reaps.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {
        self.follow_ups.fetch_add(1, Ordering::SeqCst);
    }
}

impl Watched {
    fn calls(&self) -> u32 {
        self.spawns
            .load(Ordering::SeqCst)
            .saturating_add(self.reaps.load(Ordering::SeqCst))
            .saturating_add(self.follow_ups.load(Ordering::SeqCst))
    }
}

struct Dead;

impl Liveness for Dead {
    fn alive(&self, _agent: &AgentId) -> Option<bool> {
        Some(false)
    }
}

fn delegated(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(text)?,
        after: Vec::new(),
        delegation: Some(Delegation {
            spec: SpawnSpec {
                role: None,
                model: None,
                effort: None,
                tools: Vec::new(),
                isolation: None,
                budget: None,
                wall: None,
                extra: serde_json::Map::new(),
            },
            accept: Check::Stated("it works".to_owned()),
            output: None,
            context: Vec::new(),
            note: None,
            extra: serde_json::Map::new(),
        }),
        contract: None,
        children: Vec::new(),
    })
}

fn plain(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(text)?,
        after: Vec::new(),
        delegation: None,
        contract: None,
        children: Vec::new(),
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

fn user(op: Op) -> Result<OpRequest, Box<dyn Error>> {
    Ok(OpRequest {
        plan: None,
        actor: Actor::User("user://7".parse::<Url>()?),
        op,
        request_id: None,
        expected_revision: None,
    })
}

struct Rig {
    _dir: Scratch,
    store: PlanStore,
    delegate: Arc<Watched>,
    engine: PlanEngine,
    id: PlanId,
}

fn rig(name: &str) -> Result<Rig, Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-plan-recovery-{name}"))?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let delegate = Arc::new(Watched::default());
    let engine = PlanEngine::new(store.clone(), Arc::clone(&delegate) as Arc<dyn Delegate>)
        .with_liveness(Arc::new(Dead));
    let out = engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![delegated("build it")?, plain("write it up")?],
    }))?;
    Ok(Rig {
        _dir: dir,
        store,
        delegate,
        engine,
        id: out.plan.id,
    })
}

/// Reconstruction reads records and produces state, and that is the whole of it: the
/// delegate that would spawn, reap or follow up is never called, by the reducer's signature
/// and by the engine's repair path alike.
#[test]
fn recovery_reduces_events_without_dispatching_effects() -> TestResult {
    let rig = rig("no-effects")?;
    rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("build it")?,
    }))?;
    let decomposed = rig.engine.apply(owner(Op::Decompose {
        label: TodoLabel::new("build it")?,
        todos: vec![plain("measure")?],
    }))?;
    let sub = decomposed.subplan.ok_or("no sub-plan")?;
    let calls_before = rig.delegate.calls();
    // The crash: both checkpoints are gone, the journal is whole.
    std::fs::remove_file(rig.store.path(&rig.id))?;
    std::fs::remove_file(rig.store.path(&sub))?;
    let records = rig.store.journal(&rig.id).read()?.records;
    let state = reduce(&records)?;
    assert_eq!(state.plans.len(), 2);
    assert!(matches!(
        state
            .plan(&rig.id)?
            .todo(&TodoLabel::new("build it")?)
            .map(|todo| &todo.state),
        Some(TodoState::Running { .. })
    ));
    assert_eq!(
        rig.delegate.calls(),
        calls_before,
        "reduce took no delegate"
    );
    let recovered = recovery::run(&rig.store, &rig.id, &Unknown)?;
    assert_eq!(recovered.regenerated.len(), 2);
    assert!(rig.store.path(&rig.id).is_file() && rig.store.path(&sub).is_file());
    assert_eq!(
        recovered.findings.len(),
        1,
        "the running child cannot be shown alive: {:?}",
        recovered.findings
    );
    assert_eq!(
        rig.delegate.calls(),
        calls_before,
        "recovery took no delegate"
    );
    let repaired = rig.engine.apply(owner(Op::Repair {
        resolutions: Vec::new(),
    }))?;
    assert!(
        repaired
            .notices
            .iter()
            .any(|notice| notice.contains("needs reconciliation")),
        "{:?}",
        repaired.notices
    );
    assert_eq!(
        rig.delegate.calls(),
        calls_before,
        "repair without resolutions dispatched"
    );
    assert_eq!(
        rig.store.read(&rig.id)?.clone().unmarked(),
        state.plan(&rig.id)?.clone()
    );
    Ok(())
}

/// A `spawn_intent` with no `spawn_result` is an `Unknown` outcome: reconciled with its
/// evidence, never blind-retried, and charged to the fuse exactly once. A confirmed retry
/// resolution opens a new attempt, and only then does a start spawn again.
#[test]
fn ambiguous_spawn_is_reconciled_not_reissued() -> TestResult {
    let rig = rig("ambiguous")?;
    let journal = rig.store.journal(&rig.id);
    let last = journal.read()?.records.pop().ok_or("no init record")?;
    let mut intent = last.clone();
    intent.record.op = "spawn_intent".to_owned();
    intent.record.todo = Some(TodoLabel::new("build it")?);
    intent.record.from = None;
    intent.record.to = None;
    intent.args = serde_json::json!({"label": "build it", "attempt": 1, "effect_id": "e-crashed"});
    intent.request_id = RequestId::new("r-1/intent")?;
    intent.attempt = Some(AttemptId::FIRST);
    journal.append(&journal.seal(intent, Some(&last))?)?;
    let state = reduce(&journal.read()?.records)?;
    assert_eq!(
        state.plan(&rig.id)?.spawns().get(),
        1,
        "the committed intent charged once"
    );
    let (_, standing) = state
        .intent_for(&rig.id, &TodoLabel::new("build it")?)
        .ok_or("no standing intent")?;
    assert_eq!(standing.outcome, IntentOutcome::Pending);
    let recovered = recovery::run(&rig.store, &rig.id, &Dead)?;
    assert_eq!(recovered.findings.len(), 1, "{:?}", recovered.findings);
    assert!(
        recovered.findings[0].evidence.contains("no result"),
        "{}",
        recovered.findings[0].evidence
    );
    assert_eq!(
        rig.delegate.spawns.load(Ordering::SeqCst),
        0,
        "reconciled, not reissued"
    );
    let refused = rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("build it")?,
    }));
    assert!(
        matches!(refused, Err(PlanOpError::NeedsReconciliation { .. })),
        "{refused:?}"
    );
    let owner_resolution = rig.engine.apply(owner(Op::Repair {
        resolutions: vec![Resolution {
            label: TodoLabel::new("build it")?,
            action: Resolve::Retry,
        }],
    }));
    assert!(
        matches!(owner_resolution, Err(PlanOpError::NotOwner { .. })),
        "a resolution is a confirmed user op: {owner_resolution:?}"
    );
    let resolved = rig.engine.apply(user(Op::Repair {
        resolutions: vec![Resolution {
            label: TodoLabel::new("build it")?,
            action: Resolve::Retry,
        }],
    })?)?;
    let todo = resolved
        .plan
        .todo(&TodoLabel::new("build it")?)
        .ok_or("todo missing")?;
    assert_eq!(todo.state, TodoState::Pending);
    assert_eq!(todo.attempt.get(), 2, "a retry is a new attempt");
    let started = rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("build it")?,
    }))?;
    assert_eq!(started.spawned.len(), 1);
    assert_eq!(
        rig.delegate.spawns.load(Ordering::SeqCst),
        1,
        "one real spawn, under its own intent"
    );
    assert_eq!(
        started.plan.spawns().get(),
        2,
        "the second intent is charged, the first is not recharged"
    );
    Ok(())
}

/// A damaged record inside committed history stops recovery with the seq it failed at;
/// nothing is regenerated and nothing is silently dropped.
#[test]
fn corrupt_middle_record_blocks_recovery() -> TestResult {
    let dir = Scratch::new("yi-plan-recovery-corrupt")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let id = PlanId::new("ship-logrotate-lite-with-a-packaged")?;
    std::fs::create_dir_all(store.plan_dir(&id))?;
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plans/journal/damaged-middle.jsonl");
    std::fs::copy(&fixture, store.journal_path(&id))?;
    let before = std::fs::read(store.journal_path(&id))?;
    let refused = recovery::run(&store, &id, &Unknown);
    match refused {
        Err(StoreError::RecoveryRequired { seq, root, .. }) => {
            assert_eq!(seq, Some(2));
            assert_eq!(root, id);
        }
        other => return Err(format!("expected RecoveryRequired, got {other:?}").into()),
    }
    assert!(!store.path(&id).exists(), "nothing was regenerated");
    assert_eq!(
        std::fs::read(store.journal_path(&id))?,
        before,
        "nothing was dropped"
    );
    assert!(
        matches!(store.read(&id), Err(StoreError::RecoveryRequired { .. })),
        "a read refuses too"
    );
    let engine = PlanEngine::new(store.clone(), Arc::new(Watched::default()));
    let refused = engine.apply(OpRequest {
        plan: Some(id.clone()),
        actor: Actor::Owner,
        op: Op::Repair {
            resolutions: Vec::new(),
        },
        request_id: None,
        expected_revision: None,
    });
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::RecoveryRequired { .. }))
        ),
        "{refused:?}"
    );
    Ok(())
}

/// The mirror of the corrupt-middle case: damage at record 1 leaves the reader no records,
/// and the read still judges the chain before it hands back a checkpoint.
#[test]
fn a_damaged_first_record_refuses_the_read_too() -> TestResult {
    let rig = rig("corrupt-first")?;
    let journal = rig.store.journal_path(&rig.id);
    let line = std::fs::read_to_string(&journal)?;
    let flipped = line.replacen("ship the seam", "shop the seam", 1);
    assert_ne!(flipped, line, "the goal text is in the record");
    std::fs::write(&journal, flipped)?;
    match rig.store.read(&rig.id) {
        Err(StoreError::RecoveryRequired { seq, root, .. }) => {
            assert_eq!(seq, Some(1));
            assert_eq!(root, rig.id);
        }
        other => return Err(format!("expected RecoveryRequired, got {other:?}").into()),
    }
    Ok(())
}

/// A checkpoint with no journal beside it is what a clone carries: it is named as such on a
/// read and a mutation, it does not block `init`, and `import` of the checkpoint adopts it.
#[test]
fn a_checkpoint_without_its_journal_is_named_and_adopted_by_import() -> TestResult {
    let rig = rig("detached")?;
    std::fs::remove_file(rig.store.journal_path(&rig.id))?;
    match rig.store.read(&rig.id) {
        Err(StoreError::JournalMissing { id, seq, .. }) => {
            assert_eq!(id, rig.id);
            assert_eq!(seq, Some(1));
        }
        other => return Err(format!("expected JournalMissing, got {other:?}").into()),
    }
    let started = rig.engine.apply(OpRequest {
        plan: Some(rig.id.clone()),
        actor: Actor::Owner,
        op: Op::Start {
            label: TodoLabel::new("write it up")?,
        },
        request_id: None,
        expected_revision: None,
    });
    assert!(
        matches!(
            started,
            Err(PlanOpError::Store(StoreError::JournalMissing { .. }))
        ),
        "a named mutation is refused by name: {started:?}"
    );
    let unnamed = rig.engine.apply(owner(Op::View { full: false }));
    assert!(
        matches!(unnamed, Err(PlanOpError::NoPlan)),
        "unnamed resolution skips the view: {unnamed:?}"
    );
    let opened = rig.engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![plain("again")?],
    }))?;
    assert_ne!(
        opened.plan.id, rig.id,
        "a detached view is nobody's open plan"
    );
    let unnamed = rig.engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(unnamed.plan.id, opened.plan.id);
    let source: Url = format!("local://{}", rig.store.path(&rig.id).display()).parse()?;
    let adopted = rig.engine.apply(owner(Op::Import { source }))?;
    assert_eq!(adopted.plan.id, rig.id);
    let plan = rig.store.read(&rig.id)?;
    assert_eq!(
        plan.todos.len(),
        2,
        "the view's todos are the new generation"
    );
    let records = rig.store.journal(&rig.id).read()?.records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record.op, "import");
    assert_eq!(records[0].args["format"], serde_json::json!(2));
    rig.engine.apply(OpRequest {
        plan: Some(rig.id.clone()),
        actor: Actor::Owner,
        op: Op::Start {
            label: TodoLabel::new("write it up")?,
        },
        request_id: None,
        expected_revision: None,
    })?;
    Ok(())
}

/// A sub-plan id the journal never made is missing, not tampered with: the tamper alarm is
/// reserved for a checkpoint that exists and disagrees with the journal.
#[test]
fn a_missing_sub_plan_is_missing_not_an_external_edit() -> TestResult {
    let rig = rig("missing-child")?;
    let nosuch = PlanId::new(format!("{}.nosuch", rig.id))?;
    let read = rig.store.read(&nosuch);
    assert!(matches!(read, Err(StoreError::Missing { .. })), "{read:?}");
    Ok(())
}

/// The torn final line is set aside with its bytes kept and the records before it reduce
/// normally; policy, not the reader, decides what happens to the remnant.
#[test]
fn incomplete_tail_is_preserved_and_repaired_by_policy() -> TestResult {
    let rig = rig("torn")?;
    rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("write it up")?,
    }))?;
    let path = rig.store.journal_path(&rig.id);
    let whole = std::fs::read(&path)?;
    let remnant =
        b"{\"actor\":\"main\",\"args\":{\"label\":\"write it up\"},\"argsHash\":\"sha256:";
    let mut torn = whole.clone();
    torn.extend_from_slice(remnant);
    std::fs::write(&path, &torn)?;
    let reading = rig.store.journal(&rig.id).read()?;
    assert_eq!(
        reading.records.len(),
        2,
        "init and start; the reader reports the tail"
    );
    assert!(matches!(
        reading.damage,
        Some(yi_runtime::plan::journal::Damage::TornTail { .. })
    ));
    let recovered = recovery::run(&rig.store, &rig.id, &Unknown)?;
    let side = recovered.torn.ok_or("the tail was not set aside")?;
    assert_eq!(
        std::fs::read(&side)?,
        remnant,
        "every byte of the remnant is kept"
    );
    assert_eq!(
        std::fs::read(&path)?,
        whole,
        "the journal is cut back to its last whole record"
    );
    assert!(
        recovered.findings.is_empty(),
        "an inline running todo needs no one: {:?}",
        recovered.findings
    );
    let plan = rig.store.read(&rig.id)?;
    assert!(matches!(
        plan.todo(&TodoLabel::new("write it up")?)
            .map(|todo| &todo.state),
        Some(TodoState::Running { .. })
    ));
    let appended = rig.engine.apply(owner(Op::Append {
        todos: vec![plain("ship it")?],
    }))?;
    assert_eq!(
        appended.plan.todos.len(),
        3,
        "work resumes on the clean journal"
    );
    let reading = rig.store.journal(&rig.id).read()?;
    assert_eq!(reading.records.len(), 3);
    assert!(reading.damage.is_none());
    Ok(())
}

/// An effect id names one intent: a second `spawn_intent` under the same id is refused by the
/// reducer instead of overwriting the pending intent recovery has to list.
#[test]
fn a_duplicate_spawn_intent_is_refused_by_the_reducer() -> TestResult {
    let rig = rig("duplicate-intent")?;
    rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("build it")?,
    }))?;
    let journal = rig.store.journal(&rig.id);
    let records = journal.read()?.records;
    let intent = records
        .iter()
        .find(|record| record.record.op == "spawn_intent")
        .ok_or("no spawn intent was journaled")?
        .clone();
    let last = records.last().ok_or("an empty journal")?;
    let again = journal.seal(intent, Some(last))?;
    journal.append(&again)?;
    match rig.store.read(&rig.id) {
        Err(StoreError::Reduce { source, .. }) => {
            assert!(source.to_string().contains("already exists"), "{source}");
        }
        other => return Err(format!("expected a reduce refusal, got {other:?}").into()),
    }
    Ok(())
}

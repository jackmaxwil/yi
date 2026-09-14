//! F0b, the journal: `ops.jsonl` is the commit point and `plan.json` is its checkpoint.
//!
//! Test plan. The commit protocol of plan section 5.3, in order, is: append one
//! newline terminated record with `write_all`; `sync_data` the journal file (the
//! commit point); create the checkpoint temp file with `File::create_new` in the
//! same directory; `sync_all` it; rename it over `plan.json`; `sync_all` the
//! directory; reply to the caller. A record is written before the effect it
//! describes runs, so the journal is what recovery reduces and no effect is ever
//! the only evidence of itself.
//!
//! Crash matrix. Each row is a point the process can die at, what a later open
//! must do, and the test that dies with the control.
//!
//! | crash point | expected recovery | pinned by |
//! |---|---|---|
//! | before the append | nothing happened: no record, no effect, no reply. A retry with the same `requestId` runs the op for the first time. | `journal_failure_never_acknowledges_uncommitted_success` |
//! | after `write_all`, before `sync_data` | not acknowledged. The bytes may or may not be durable; the reader decides on the next open, and a retry with the same `requestId` replays whatever it finds instead of applying twice. | `a_failed_sync_acknowledges_nothing_and_the_checkpoint_is_untouched` |
//! | after `sync_data`, before the checkpoint temp | committed. The checkpoint lags; recovery reduces the journal and regenerates `plan.json`. | `kill_nine_between_append_and_checkpoint_recovers_the_record` |
//! | after the temp, before the rename | committed, checkpoint still lagging. The orphan temp is removed on the next transaction; it is never read as state. | `kill_nine_between_append_and_checkpoint_recovers_the_record` |
//! | after the rename, before the directory `sync_all` | committed. The rename may or may not be visible; either way the journal decides and a lagging or absent checkpoint is regenerated. | `crash_after_commit_before_reply_returns_same_request_result` |
//! | after the reply is written but before the caller sees it | committed. The retry carries the same `requestId` and the same `argsHash`, so the recorded result is replayed and the op does not run twice; the same id with different args is refused. | `crash_after_commit_before_reply_returns_same_request_result` |
//! | after an opening `init` or `import` commits, before its checkpoint | committed. A retry with the same `requestId` replays the recorded plan instead of refusing it as already open. | `an_init_and_an_opening_set_replay_on_retry`; `plan_import::a_retried_import_replays_and_a_lost_checkpoint_cannot_be_imported_over` |
//! | between the two commits of a `set` that opens a plan | the opening `init` is committed under `<requestId>/init`; the retry, under the caller's own id, finds no `set` record and applies it now; a later retry replays it. | `an_init_and_an_opening_set_replay_on_retry` |
//!
//! Fixtures this file reads: `fixtures/plans/journal/records.jsonl` (one record
//! of every kind this stage writes), `journal/canonical.md` (the canonical JSON
//! and digest rule), `python/yi_runtime/tests/vectors/canonical.json`.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use proptest::prelude::{Strategy, any, prop};
use proptest::test_runner::{Config, TestRunner};
use serde_json::Value;
use yi_runtime::plan::journal::{Damage, Fs, Journal, JournalError, RECORD_CAP, RealFs};
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError, TodoSpec,
};
use yi_runtime::plan::store::{PlanStore, StoreError};
use yi_types::plan::canonical::{Digest, canonical_bytes, canonical_digest};
use yi_types::plan::doc::{AgentId, Delegation, GoalText, PlanId, TodoAddr, TodoLabel, TouchCount};
use yi_types::plan::ledger::{JournalRecord, RequestId};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

/// The child mode of the kill-nine test: set by the parent, read only here.
const KILL_CHILD_DIR: &str = "PLAN_JOURNAL_KILL_CHILD_DIR";

const HEALTHY: u8 = 0;
const FAIL_WRITE: u8 = 1;
const FAIL_SYNC: u8 = 2;
const HANG_AFTER_SYNC: u8 = 3;

/// The injectable seam: the real calls, with one failure point switched at runtime.
struct Switch {
    mode: AtomicU8,
    marker: Option<PathBuf>,
}

impl Switch {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(HEALTHY),
            marker: None,
        })
    }

    fn set(&self, mode: u8) {
        self.mode.store(mode, Ordering::SeqCst);
    }
}

impl Fs for Switch {
    fn write_all(&self, file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
        if self.mode.load(Ordering::SeqCst) == FAIL_WRITE {
            return Err(std::io::Error::other(
                "injected: the disk refused the write",
            ));
        }
        RealFs.write_all(file, bytes)
    }

    fn sync_data(&self, file: &std::fs::File) -> std::io::Result<()> {
        match self.mode.load(Ordering::SeqCst) {
            FAIL_SYNC => Err(std::io::Error::other("injected: fsync failed")),
            HANG_AFTER_SYNC => {
                RealFs.sync_data(file)?;
                if let Some(marker) = &self.marker {
                    std::fs::write(marker, b"committed")?;
                }
                std::thread::sleep(std::time::Duration::from_secs(120));
                Ok(())
            }
            _ => RealFs.sync_data(file),
        }
    }
}

struct Stub;

impl Delegate for Stub {
    fn spawn(&self, _at: &TodoAddr, _delegation: &Delegation) -> Result<AgentId, String> {
        AgentId::new("child").map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        Ok(None)
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

fn spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(text)?,
        after: Vec::new(),
        delegation: None,
        contract: None,
        children: Vec::new(),
    })
}

fn request(op: Op, request_id: &str) -> Result<OpRequest, Box<dyn Error>> {
    Ok(OpRequest {
        plan: None,
        actor: Actor::Owner,
        op,
        request_id: Some(RequestId::new(request_id)?),
        expected_revision: None,
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

fn append(label: &str, request_id: &str) -> Result<OpRequest, Box<dyn Error>> {
    request(
        Op::Append {
            todos: vec![spec(label)?],
        },
        request_id,
    )
}

struct Rig {
    _dir: Scratch,
    store: PlanStore,
    switch: Arc<Switch>,
    engine: PlanEngine,
    id: PlanId,
}

fn rig(name: &str) -> Result<Rig, Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-plan-journal-{name}"))?;
    let switch = Switch::new();
    let store = PlanStore::open(dir.to_path_buf())?.with_fs(Arc::clone(&switch) as Arc<dyn Fs>);
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let out = engine.apply(owner(Op::Init {
        goal: GoalText::new("ship the seam end to end")?,
        todos: vec![spec("cut")?],
    }))?;
    Ok(Rig {
        _dir: dir,
        store,
        switch,
        engine,
        id: out.plan.id,
    })
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans")
}

/// The journal write fails: the op is refused, `plan.json` is untouched, nothing was
/// acknowledged, and a retry with the same request id runs the op for the first time.
#[test]
fn journal_failure_never_acknowledges_uncommitted_success() -> TestResult {
    let rig = rig("write-fails")?;
    let checkpoint = rig.store.path(&rig.id);
    let before = std::fs::read(&checkpoint)?;
    let journal_before = std::fs::read(rig.store.journal_path(&rig.id))?;
    rig.switch.set(FAIL_WRITE);
    let refused = rig.engine.apply(append("polish", "r-1")?);
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::Journal(
                JournalError::Io { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read(&checkpoint)?,
        before,
        "the checkpoint moved on a refused write"
    );
    assert_eq!(
        std::fs::read(rig.store.journal_path(&rig.id))?,
        journal_before,
        "a failed write left bytes behind"
    );
    rig.switch.set(HEALTHY);
    let retried = rig.engine.apply(append("polish", "r-1")?)?;
    assert!(
        retried.notices.is_empty(),
        "the first run is not a replay: {:?}",
        retried.notices
    );
    assert_eq!(retried.plan.todos.len(), 2);
    assert_eq!(retried.plan.touched, TouchCount(2));
    Ok(())
}

/// The injected `sync_data` failure: no acknowledgement and a byte-identical checkpoint. The
/// bytes may be on disk; the next open takes whatever is durable, and a retry with the same
/// request id replays rather than applying twice.
#[test]
fn a_failed_sync_acknowledges_nothing_and_the_checkpoint_is_untouched() -> TestResult {
    let rig = rig("sync-fails")?;
    let checkpoint = rig.store.path(&rig.id);
    let before = std::fs::read(&checkpoint)?;
    rig.switch.set(FAIL_SYNC);
    let refused = rig.engine.apply(append("polish", "r-2")?);
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::Journal(
                JournalError::Sync { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read(&checkpoint)?,
        before,
        "the checkpoint moved before the commit"
    );
    rig.switch.set(HEALTHY);
    let durable = rig.store.journal(&rig.id).read()?;
    assert!(durable.damage.is_none(), "{:?}", durable.damage);
    let retried = rig.engine.apply(append("polish", "r-2")?)?;
    assert_eq!(
        retried.plan.todos.len(),
        2,
        "one append, however many tries"
    );
    let on_disk = rig.store.read(&rig.id)?;
    assert_eq!(on_disk.todos.len(), 2);
    assert_eq!(
        on_disk.journal.map(|mark| mark.seq.get()),
        Some(u64::try_from(
            rig.store.journal(&rig.id).read()?.records.len()
        )?),
        "the regenerated checkpoint names the journal's last record"
    );
    Ok(())
}

/// A crash between the commit and the reply: the retry carries the same `requestId` and
/// `argsHash` and gets the recorded result with no second effect; other args are refused.
#[test]
fn crash_after_commit_before_reply_returns_same_request_result() -> TestResult {
    let rig = rig("replay")?;
    let first = rig.engine.apply(append("polish", "r-3")?)?;
    // The crash: the commit is on disk, the checkpoint never was.
    std::fs::remove_file(rig.store.path(&rig.id))?;
    let again = rig.engine.apply(append("polish", "r-3")?)?;
    assert_eq!(again.plan.clone().unmarked(), first.plan.clone().unmarked());
    assert_eq!(again.plan.todos.len(), 2, "the op did not run twice");
    assert!(
        again
            .notices
            .iter()
            .any(|notice| notice.contains("replayed")),
        "{:?}",
        again.notices
    );
    assert!(
        rig.store.path(&rig.id).is_file(),
        "the checkpoint is regenerated"
    );
    let other = rig.engine.apply(append("something else", "r-3")?);
    assert!(
        matches!(other, Err(PlanOpError::RequestIdReused { .. })),
        "{other:?}"
    );
    let records = rig.store.journal(&rig.id).read()?.records;
    assert_eq!(
        records.len(),
        2,
        "init and one append; the duplicate wrote nothing"
    );
    Ok(())
}

/// The opening ops replay too: an `init` retried after its checkpoint was lost returns the
/// recorded plan, not `PlanExists`; a `set` that opened a plan keeps the caller's id for the
/// `set` record, so a crash between its two commits leaves a retry that finishes the `set`.
#[test]
fn an_init_and_an_opening_set_replay_on_retry() -> TestResult {
    let dir = Scratch::new("yi-plan-journal-opening")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let init = || {
        request(
            Op::Init {
                goal: GoalText::new("ship the seam end to end")?,
                todos: vec![spec("cut")?],
            },
            "r-init",
        )
    };
    let opened = engine.apply(init()?)?;
    std::fs::remove_file(store.path(&opened.plan.id))?;
    let again = engine.apply(init()?)?;
    assert_eq!(again.plan.id, opened.plan.id);
    assert!(
        again
            .notices
            .iter()
            .any(|notice| notice.contains("replayed")),
        "{:?}",
        again.notices
    );
    assert_eq!(
        store.roots()?,
        vec![opened.plan.id.clone()],
        "one root, opened once"
    );
    assert!(
        store.path(&opened.plan.id).is_file(),
        "the checkpoint is regenerated"
    );

    let dir = Scratch::new("yi-plan-journal-opening-set")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let set = || {
        request(
            Op::Set {
                goal: Some(GoalText::new("ship the seam end to end")?),
                rows: vec![yi_runtime::plan::ops::SetRow {
                    spec: spec("cut")?,
                    state: yi_types::plan::doc::TodoStateName::Pending,
                }],
            },
            "r-set",
        )
    };
    let opened = engine.apply(set()?)?;
    let id = opened.plan.id.clone();
    let records = store.journal(&id).read()?.records;
    assert_eq!(records.len(), 2, "init and set");
    assert_eq!(records[0].request_id.as_str(), "r-set/init");
    assert_eq!(records[1].request_id.as_str(), "r-set");
    // The crash: the init committed, the set never did, and the checkpoint is gone.
    let line = records[0].line()?;
    std::fs::write(store.journal_path(&id), line)?;
    std::fs::remove_file(store.path(&id))?;
    let finished = engine.apply(set()?)?;
    assert!(
        !finished
            .notices
            .iter()
            .any(|notice| notice.contains("replayed")),
        "the set applied for the first time: {:?}",
        finished.notices
    );
    assert_eq!(store.journal(&id).read()?.records.len(), 2);
    let replayed = engine.apply(set()?)?;
    assert!(
        replayed
            .notices
            .iter()
            .any(|notice| notice.contains("replayed")),
        "{:?}",
        replayed.notices
    );
    assert_eq!(store.journal(&id).read()?.records.len(), 2);
    assert_eq!(store.roots()?, vec![id]);
    Ok(())
}

/// A sub-plan's records ride its root's journal, so one transaction covers both files; a
/// crash between the two projected writes leaves no half-applied state.
#[test]
fn root_transaction_covers_parent_and_subplan_changes() -> TestResult {
    let rig = rig("subplan")?;
    rig.engine.apply(owner(Op::Start {
        label: TodoLabel::new("cut")?,
    }))?;
    let out = rig.engine.apply(owner(Op::Decompose {
        label: TodoLabel::new("cut")?,
        todos: vec![spec("measure")?],
    }))?;
    let sub = out.subplan.ok_or("no sub-plan")?;
    assert!(
        !rig.store.journal_path(&sub).exists(),
        "a sub-plan has no journal of its own"
    );
    let stepped = rig.engine.apply(OpRequest {
        plan: Some(sub.clone()),
        actor: Actor::Owner,
        op: Op::Start {
            label: TodoLabel::new("measure")?,
        },
        request_id: None,
        expected_revision: None,
    })?;
    assert_eq!(stepped.plan.id, sub);
    let root_mark = rig.store.read(&rig.id)?.journal.ok_or("root unmarked")?;
    let sub_mark = rig.store.read(&sub)?.journal.ok_or("sub unmarked")?;
    assert_eq!(
        root_mark, sub_mark,
        "both checkpoints name the one journal record"
    );
    // The crash between the projected writes: the sub-plan's checkpoint is gone.
    std::fs::remove_file(rig.store.path(&sub))?;
    let recovered = rig.store.read(&sub)?;
    assert!(
        matches!(
            recovered
                .todo(&TodoLabel::new("measure")?)
                .map(|todo| &todo.state),
            Some(yi_types::plan::doc::TodoState::Running { .. })
        ),
        "the sub-plan is regenerated from the root's journal"
    );
    assert_eq!(recovered.journal, Some(root_mark));
    Ok(())
}

/// A real subprocess killed with SIGKILL between the append and the checkpoint: the record
/// is committed and the next open regenerates `plan.json` from it.
#[test]
fn kill_nine_between_append_and_checkpoint_recovers_the_record() -> TestResult {
    let dir = Scratch::new("yi-plan-journal-kill-nine")?;
    let store = PlanStore::open(dir.to_path_buf())?;
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let id = engine
        .apply(owner(Op::Init {
            goal: GoalText::new("ship the seam end to end")?,
            todos: vec![spec("cut")?],
        }))?
        .plan
        .id;
    let before = std::fs::read(store.path(&id))?;
    let marker = dir.join("committed");
    #[expect(
        clippy::disallowed_methods,
        reason = "the kill-nine row needs a real process to SIGKILL, and the test binary is it"
    )]
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "kill_nine_child", "--nocapture"])
        .env(KILL_CHILD_DIR, &*dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !marker.is_file() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err("the child never reached its commit point".into());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("the child exited before committing: {status}").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill()?;
    let status = child.wait()?;
    assert!(
        !status.success(),
        "the child was killed, not finished: {status}"
    );
    assert_eq!(
        std::fs::read(store.path(&id))?,
        before,
        "the checkpoint lags; the child died before writing it"
    );
    let records = store.journal(&id).read()?;
    assert!(records.damage.is_none(), "{:?}", records.damage);
    assert_eq!(records.records.len(), 2, "init and the committed append");
    let recovered = store.read(&id)?;
    assert_eq!(recovered.todos.len(), 2, "the record is reduced into state");
    assert_ne!(
        std::fs::read(store.path(&id))?,
        before,
        "the checkpoint is regenerated"
    );
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let out = engine.apply(owner(Op::View { full: true }))?;
    assert_eq!(out.plan.todos.len(), 2);
    Ok(())
}

/// The other half of the kill-nine test: runs the append in a process the parent kills. A
/// plain test that returns at once unless the parent set the directory.
#[test]
fn kill_nine_child() -> TestResult {
    let Some(dir) = std::env::var_os(KILL_CHILD_DIR) else {
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    let switch = Arc::new(Switch {
        mode: AtomicU8::new(HANG_AFTER_SYNC),
        marker: Some(dir.join("committed")),
    });
    let store = PlanStore::open(dir)?.with_fs(switch as Arc<dyn Fs>);
    let engine = PlanEngine::new(store, Arc::new(Stub));
    let _never_returns = engine.apply(append("polish", "r-kill")?);
    Ok(())
}

/// The reader takes `Read::take(RECORD_CAP)` per line: an over-long or unterminated record
/// is damage and is resynchronized at the next newline, never parsed as a shorter record.
#[test]
fn an_unterminated_record_past_the_cap_is_refused_and_resynchronized() -> TestResult {
    let rig = rig("over-cap")?;
    let path = rig.store.journal_path(&rig.id);
    let good = std::fs::read(&path)?;
    let mut bytes = good.clone();
    let padding = "x".repeat(RECORD_CAP.saturating_add(16));
    bytes.extend_from_slice(format!("{{\"seq\":2,\"pad\":\"{padding}\"}}\n").as_bytes());
    bytes.extend_from_slice(b"{\"seq\":3}\n");
    std::fs::write(&path, &bytes)?;
    let reading = rig.store.journal(&rig.id).read()?;
    assert_eq!(
        reading.records.len(),
        1,
        "only the whole record before the damage"
    );
    match reading.damage {
        Some(Damage::Corrupt {
            offset,
            seq,
            resync,
            ..
        }) => {
            assert_eq!(
                offset,
                u64::try_from(good.len())?,
                "damage starts where the good bytes end"
            );
            assert_eq!(seq, None, "an over-long line is never parsed for a seq");
            let after_newline = bytes.len().saturating_sub(b"{\"seq\":3}\n".len());
            assert_eq!(resync, Some(u64::try_from(after_newline)?));
        }
        other => return Err(format!("expected corrupt damage, got {other:?}").into()),
    }
    assert!(
        matches!(
            rig.store.read(&rig.id),
            Err(StoreError::RecoveryRequired { .. })
        ),
        "damage inside committed history refuses the read"
    );
    let refused = rig.engine.apply(append("polish", "r-4")?);
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::RecoveryRequired { .. }))
        ),
        "{refused:?}"
    );
    // The reader refuses a record over the cap even when it is terminated.
    let mut terminated = good;
    terminated.extend_from_slice(format!("{{\"seq\":2,\"pad\":\"{padding}\"}}\n").as_bytes());
    std::fs::write(&path, &terminated)?;
    let reading = rig.store.journal(&rig.id).read()?;
    assert!(
        matches!(reading.damage, Some(Damage::Corrupt { .. })),
        "{:?}",
        reading.damage
    );
    Ok(())
}

fn verify_chain(path: &Path) -> Result<Vec<JournalRecord>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut prev: Option<Digest> = None;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let record: JournalRecord = serde_json::from_str(line)?;
        assert_eq!(
            record.args_hash,
            canonical_digest(&record.args)?,
            "{}: argsHash of record {}",
            path.display(),
            index
        );
        assert_eq!(
            record.digest,
            record.digest_of(prev.as_ref())?,
            "{}: digest of record {}",
            path.display(),
            index
        );
        assert_eq!(
            String::from_utf8(record.line()?)?,
            format!("{line}\n"),
            "{}: record {} does not re-serialize byte for byte",
            path.display(),
            index
        );
        prev = Some(record.digest);
        records.push(record);
    }
    Ok(records)
}

/// The golden vectors and the digest chains in the fixtures are reproduced by the Rust
/// canonicalizer, byte for byte and hash for hash. The Python twin reads the same files.
#[test]
fn canonical_hashes_match_across_rust_and_python() -> TestResult {
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../python/yi_runtime/tests/vectors/canonical.json");
    let document: Value = serde_json::from_str(&std::fs::read_to_string(vectors)?)?;
    let cases = document["vectors"].as_array().ok_or("no vectors")?;
    assert!(cases.len() >= 10);
    for case in cases {
        let name = case["name"].as_str().unwrap_or("?");
        let bytes = canonical_bytes(&case["input"])?;
        assert_eq!(
            String::from_utf8(bytes.clone())?,
            case["canonical"].as_str().ok_or("no canonical")?,
            "{name}"
        );
        assert_eq!(
            Digest::of(&bytes).hex(),
            case["sha256"].as_str().ok_or("no sha256")?,
            "{name}"
        );
    }
    let journal = fixtures().join("journal");
    let records = verify_chain(&journal.join("records.jsonl"))?;
    assert_eq!(
        records.len(),
        8,
        "one record of every kind this stage writes"
    );
    let kinds: Vec<&str> = records
        .iter()
        .map(|record| record.record.op.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "import",
            "spawn_intent",
            "spawn_result",
            "start",
            "done",
            "fuse_reset",
            "reconciled",
            "accepted_by_user"
        ]
    );
    assert!(
        records[4].is_refusal(),
        "the child's done is a refusal with no `to`"
    );
    assert_eq!(records[4].record.to, None);
    for name in ["campaign", "stale-checkpoint", "edited"] {
        let path = fixtures().join("format2").join(name).join("ops.jsonl");
        verify_chain(&path)?;
        let reading = Journal::open(path, Arc::new(RealFs)).read()?;
        assert!(reading.damage.is_none(), "{name}: {:?}", reading.damage);
    }
    let torn = Journal::open(journal.join("torn-tail.jsonl"), Arc::new(RealFs)).read()?;
    assert_eq!(torn.records.len(), 3);
    assert!(
        matches!(torn.damage, Some(Damage::TornTail { .. })),
        "{:?}",
        torn.damage
    );
    let damaged = Journal::open(journal.join("damaged-middle.jsonl"), Arc::new(RealFs)).read()?;
    assert_eq!(damaged.records.len(), 1);
    assert!(
        matches!(damaged.damage, Some(Damage::Corrupt { seq: Some(seq), .. }) if seq.get() == 2),
        "{:?}",
        damaged.damage
    );
    Ok(())
}

fn arbitrary_journal() -> impl Strategy<Value = Vec<u8>> {
    let good = std::fs::read(fixtures().join("journal/records.jsonl")).unwrap_or_default();
    prop::collection::vec(any::<u8>(), 0..4096).prop_flat_map(move |noise| {
        let good = good.clone();
        (0..=good.len(), any::<bool>()).prop_map(move |(cut, prefix)| {
            let mut bytes = Vec::new();
            if prefix {
                bytes.extend_from_slice(good.get(..cut).unwrap_or(&[]));
            }
            bytes.extend_from_slice(&noise);
            bytes
        })
    })
}

/// The journal reader never panics on arbitrary bytes: any file reads to `Ok` with the whole
/// records it found and the first damage, or an `Err` naming the path.
#[test]
fn the_journal_reader_never_panics_on_arbitrary_bytes() -> TestResult {
    let dir = Scratch::new("yi-plan-journal-fuzz")?;
    let path = dir.join("ops.jsonl");
    let mut config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 128;
    }
    let mut runner = TestRunner::new(config);
    runner
        .run(&arbitrary_journal(), |bytes| {
            std::fs::write(&path, &bytes)
                .map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))?;
            let reading = Journal::open(path.clone(), Arc::new(RealFs)).read();
            if let Ok(reading) = reading {
                let mut prev: Option<Digest> = None;
                for record in &reading.records {
                    proptest::prop_assert_eq!(
                        record.digest,
                        record.digest_of(prev.as_ref()).map_err(|error| {
                            proptest::test_runner::TestCaseError::fail(error.to_string())
                        })?
                    );
                    prev = Some(record.digest);
                }
            }
            Ok(())
        })
        .map_err(|error| format!("{error}").into())
}

/// Rust and Python agree on the request identity too: the args hash the tool computes for a
/// fixture op matches the vector's, so a Python program can dedup its own requests.
#[test]
fn an_op_hashes_to_the_vector_the_python_side_carries() -> TestResult {
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../python/yi_runtime/tests/vectors/canonical.json");
    let document: Value = serde_json::from_str(&std::fs::read_to_string(vectors)?)?;
    let case = document["vectors"]
        .as_array()
        .and_then(|cases| {
            cases
                .iter()
                .find(|case| case["name"] == "a plan op payload, the shape argsHash is taken over")
        })
        .ok_or("no op vector")?;
    let digest = canonical_digest(&case["input"])?;
    assert_eq!(digest.hex(), case["sha256"].as_str().ok_or("no sha256")?);
    let _outcome_shape_is_public: fn(&Outcome) -> usize = |outcome| outcome.notices.len();
    Ok(())
}

/// A root's first record fails to write: no empty `ops.jsonl` is left to list a root with no
/// plan in it, so a healthy `init` opens the plan and an unnamed `view` finds it.
#[test]
fn a_failed_first_append_leaves_no_journal_behind() -> TestResult {
    let dir = Scratch::new("yi-plan-journal-first-append")?;
    let switch = Switch::new();
    let store = PlanStore::open(dir.to_path_buf())?.with_fs(Arc::clone(&switch) as Arc<dyn Fs>);
    let engine = PlanEngine::new(store.clone(), Arc::new(Stub));
    let init = || -> Result<OpRequest, Box<dyn Error>> {
        Ok(owner(Op::Init {
            goal: GoalText::new("ship the seam end to end")?,
            todos: vec![spec("cut")?],
        }))
    };
    switch.set(FAIL_WRITE);
    let refused = engine.apply(init()?);
    assert!(
        matches!(
            refused,
            Err(PlanOpError::Store(StoreError::Journal(
                JournalError::Io { .. }
            )))
        ),
        "{refused:?}"
    );
    assert!(
        store.roots()?.is_empty(),
        "a refused first append listed a root: {:?}",
        store.roots()?
    );
    switch.set(HEALTHY);
    let opened = engine.apply(init()?)?;
    assert_eq!(opened.plan.todos.len(), 1);
    let viewed = engine.apply(owner(Op::View { full: false }))?;
    assert_eq!(viewed.plan.id, opened.plan.id);
    Ok(())
}

/// A reused request id carrying another op of the same field shape is refused, never answered
/// with the earlier op's result: `drop cut` then `unblock cut` under one id.
#[test]
fn a_reused_request_id_with_another_op_kind_is_refused() -> TestResult {
    let rig = rig("op-kind")?;
    rig.engine.apply(append("polish", "r-0")?)?;
    let label = TodoLabel::new("cut")?;
    let dropped = rig.engine.apply(request(
        Op::Drop {
            label: label.clone(),
        },
        "r-1",
    )?)?;
    assert!(dropped.notices.is_empty(), "{:?}", dropped.notices);
    let unblocked = rig.engine.apply(request(Op::Unblock { label }, "r-1")?);
    assert!(
        matches!(unblocked, Err(PlanOpError::RequestIdReused { .. })),
        "{unblocked:?}"
    );
    let records = rig.store.journal(&rig.id).read()?.records;
    assert_eq!(
        records.len(),
        3,
        "init, append, drop; the unblock wrote nothing"
    );
    Ok(())
}

// Dies with the fixture drifting from the wire: `Verdict.items` is a list of `{id, verdict}`
// lines and every settling record names its effect, exactly as `done.rs` writes them.
#[test]
fn the_verification_fixture_carries_the_shapes_the_kernel_writes() -> TestResult {
    let records = verify_chain(&fixtures().join("journal/verification.jsonl"))?;
    let kinds: Vec<&str> = records
        .iter()
        .map(|record| record.record.op.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "verification_requested",
            "done",
            "done_refused",
            "verification_stale"
        ]
    );
    for record in &records[1..] {
        let effect = record
            .record
            .extra
            .get("effect_id")
            .or_else(|| record.args.get("effect_id"))
            .and_then(Value::as_str);
        assert!(effect.is_some_and(|id| id.starts_with("e-")), "{kinds:?}");
    }
    assert!(
        records[0].args["claim"]["pid"].is_u64(),
        "the request names its claimant"
    );
    let verdicts: Vec<yi_types::plan::contract::Verdict> = records
        .iter()
        .filter_map(|record| record.verdict.clone())
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    assert_eq!(verdicts.len(), 2);
    for (record, verdict) in records[1..3].iter().zip(&verdicts) {
        let stored = record.verdict.clone().ok_or("a verdict")?;
        assert_eq!(
            canonical_bytes(&serde_json::to_value(verdict)?)?,
            canonical_bytes(&stored)?,
            "the fixture's verdict is the bytes the kernel emits"
        );
    }
    Ok(())
}

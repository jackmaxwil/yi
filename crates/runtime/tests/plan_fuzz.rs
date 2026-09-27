//! Random op sequences over the real step table (proposal section 12): the
//! generator drives [`yi_runtime::plan::ops::PlanEngine`] against a temp
//! [`yi_runtime::plan::store::PlanStore`] and a stub delegate, and re-asserts
//! every insert-check invariant after every op. A failing property shrinks to
//! the shortest breaking sequence, which then becomes a walkthrough fixture.

use crate::scratch;
use scratch::Scratch;

use std::collections::HashMap;
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use yi_runtime::plan::journal::{Fs, RealFs};
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError, TodoSpec,
};
use yi_runtime::plan::store::{PLAN_CAP_BYTES, PlanStore};
use yi_types::plan::canonical::Digest;
use yi_types::plan::contract::Contract;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, Delegation, GoalText, OutputSchema, PlanId, PlanState, PlanTier,
    SpawnSpec, TodoLabel, TodoState,
};
use yi_types::plan::op::Choice;
use yi_types::url::{Durability, Scheme, Url};

/// Invariant: the fuzz lane rides `just check`, so the case budget stays small
/// enough that the whole property finishes in a few seconds; a soak raises
/// PROPTEST_CASES instead, at roughly linear cost.
const CASES: u32 = 256;
const MAX_ACTIONS: usize = 20;

/// Slots 2 and 4 differ verbatim but collide after slugging, exercising the
/// slug-uniqueness insert check.
const LABELS: [&str; 6] = [
    "alpha survey pass",
    "beta build step",
    "gamma verify run",
    "delta ship note",
    "Gamma  Verify Run",
    "omega cleanup",
];

const DURABLE_URLS: [&str; 3] = [
    "local://artifacts/report.txt",
    "history://alpha/e3",
    "checkpoint://c9",
];

const EPHEMERAL_URL: &str = "kernel://child/scratch";
const AGENT_URL: &str = "agent://live/child";

#[derive(Debug, Clone, Copy)]
enum Target {
    Root,
    Sub,
}

#[derive(Debug, Clone, Copy)]
struct SpecPick {
    slot: usize,
    delegated: bool,
    declares: bool,
    after: Option<usize>,
    /// A one-item cmd contract whose checker passes (`true`) or fails (`exit 1`); the manifest
    /// is staged in the plan's artifacts when the plan id is known at the insert.
    contract: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
enum OutPick {
    Nothing,
    Kernel,
    Durable(usize),
    Agent,
}

#[derive(Debug, Clone, Copy)]
enum LastPick {
    Nothing,
    Durable(usize),
    Ephemeral,
}

#[derive(Debug, Clone)]
enum Action {
    Init {
        specs: Vec<SpecPick>,
    },
    Append {
        specs: Vec<SpecPick>,
    },
    BulkAppend {
        count: u16,
    },
    Drop {
        target: Target,
        slot: usize,
        discard: bool,
    },
    Block {
        target: Target,
        slot: usize,
        note_len: u16,
    },
    Unblock {
        target: Target,
        slot: usize,
        as_user: bool,
    },
    Reorder {
        target: Target,
        rotate: usize,
    },
    AddEdge {
        target: Target,
        from: usize,
        to: usize,
    },
    Start {
        target: Target,
        slot: usize,
    },
    Done {
        target: Target,
        slot: usize,
        output: OutPick,
    },
    Fail {
        target: Target,
        slot: usize,
        produced: LastPick,
        discard: bool,
    },
    Retry {
        target: Target,
        slot: usize,
    },
    Decompose {
        target: Target,
        slot: usize,
        specs: Vec<SpecPick>,
    },
    Supersede {
        target: Target,
        specs: Vec<SpecPick>,
        reap_fails: bool,
    },
    View {
        target: Target,
    },
    ChildProbe {
        slot: usize,
    },
    Rehydrate,
    /// The confirmed user op: the one writer that lowers the fuse.
    FuseReset,
    /// Reconstruction and reconciliation with no resolutions: never an effect.
    Repair,
    /// An injected failure at one journal point for the next op, then a rehydration: the
    /// journal decides what survived, and every invariant still holds.
    Crash {
        at_sync: bool,
        specs: Vec<SpecPick>,
    },
    /// The explicit import of a generated format-1 document: a fresh id imports once with its
    /// bytes kept as the artifact the genesis record names; a root the journal already holds
    /// is refused, never imported over.
    Import {
        serial: u8,
        slots: Vec<usize>,
    },
    /// One contracted todo driven through append (or init), start and done in one action, so
    /// every run reaches a verified completion and a refused verdict whatever else the
    /// generator interleaves.
    Verified {
        passes: bool,
    },
}

/// F0c reachability, counted across the whole run: the `VerifiedDone` invariant is vacuous
/// in a run that never produces one, so the lane asserts both outcomes happened.
static VERIFIED_DONE: AtomicU32 = AtomicU32::new(0);
static DONE_REFUSED: AtomicU32 = AtomicU32::new(0);

fn fail<E: std::fmt::Display>(error: E) -> TestCaseError {
    TestCaseError::fail(error.to_string())
}

fn label(slot: usize) -> Result<TodoLabel, TestCaseError> {
    TodoLabel::new(LABELS[slot % LABELS.len()]).map_err(fail)
}

fn durable_url(pick: usize) -> Result<Url, TestCaseError> {
    DURABLE_URLS[pick % DURABLE_URLS.len()]
        .parse::<Url>()
        .map_err(fail)
}

fn delegation(declares: bool) -> Result<Delegation, TestCaseError> {
    let output = if declares {
        Some(OutputSchema {
            schema: "local://schemas/out.json".parse::<Url>().map_err(fail)?,
            extra: serde_json::Map::new(),
        })
    } else {
        None
    };
    Ok(Delegation {
        spec: SpawnSpec {
            role: None,
            model: None,
            effort: None,
            tools: Vec::new(),
            isolation: None,
            budget: None,
            wall: None,
            parent_close: None,
            extra: serde_json::Map::new(),
        },
        accept: Check::Command("true".to_owned()),
        output,
        context: Vec::new(),
        note: None,
        extra: serde_json::Map::new(),
    })
}

/// A writer contract over one critical cmd item, its manifest staged under `plan`.
fn contract_for(case: &Case, plan: &PlanId, passes: bool) -> Result<Contract, TestCaseError> {
    let manifest = serde_json::json!({
        "manifest": 1, "command": if passes { "true" } else { "exit 1" },
        "cwd": "snapshot_root", "cwd_subdir": null, "protected": [], "timeout_ms": 5_000,
        "env": [], "reads_outside_snapshot": false
    });
    let put = case
        .store
        .artifacts(plan)
        .put(
            &serde_json::to_vec(&manifest).map_err(fail)?,
            "application/vnd.yi.checker-manifest+json",
            &case.store.nonce(),
        )
        .map_err(fail)?;
    serde_json::from_value(serde_json::json!({
        "class": "writer",
        "items": [{"id": "check", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": put, "timeout_ms": 5_000}}}],
        "threshold": 1000, "min_coverage": 1000
    }))
    .map_err(fail)
}

/// `plan` is the id the specs land in when the insert knows it (init, append, supersede); a
/// decompose's child id is allocated inside the op, so its specs carry no contract.
fn todo_specs(
    case: &Case,
    plan: Option<&PlanId>,
    picks: &[SpecPick],
) -> Result<Vec<TodoSpec>, TestCaseError> {
    picks
        .iter()
        .map(|pick| {
            Ok(TodoSpec {
                label: label(pick.slot)?,
                after: match pick.after {
                    Some(slot) => vec![label(slot)?],
                    None => Vec::new(),
                },
                delegation: if pick.delegated {
                    Some(delegation(pick.declares)?)
                } else {
                    None
                },
                contract: match (plan, pick.contract) {
                    (Some(plan), Some(passes)) => Some(contract_for(case, plan, passes)?),
                    _ => None,
                },
                children: Vec::new(),
                cites: Default::default(),
            })
        })
        .collect()
}

fn fuzz_goal() -> Result<GoalText, TestCaseError> {
    GoalText::new("ship the fuzzed widget end to end").map_err(fail)
}

fn root_of(id: &PlanId) -> Result<PlanId, TestCaseError> {
    match id.as_str().split_once('.') {
        Some((root, _)) => PlanId::new(root).map_err(fail),
        None => Ok(id.clone()),
    }
}

fn in_flight(plan: &yi_types::plan::doc::Plan) -> usize {
    plan.todos
        .iter()
        .filter(|todo| matches!(todo.state, TodoState::Running { .. }) && todo.delegation.is_some())
        .count()
}

/// The store's filesystem seam with one journal failure point the fuzzer flips for one op.
#[derive(Default)]
struct CrashPoint {
    fail_write: AtomicBool,
    fail_sync: AtomicBool,
}

impl Fs for CrashPoint {
    fn write_all(&self, file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("fuzzed: the write failed"));
        }
        RealFs.write_all(file, bytes)
    }

    /// No crash here loses the page cache, so a sync that succeeds need not reach the disk:
    /// the device flush was most of every case's wall time, and nothing the fuzzer reads.
    fn sync_data(&self, _file: &std::fs::File) -> std::io::Result<()> {
        if self.fail_sync.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("fuzzed: the sync failed"));
        }
        Ok(())
    }

    fn sync_all(&self, _file: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Stub {
    serial: AtomicU32,
    fail_reap: AtomicBool,
    produced: Mutex<Option<Url>>,
}

impl Stub {
    fn set_produced(&self, url: Option<Url>) {
        if let Ok(mut slot) = self.produced.lock() {
            *slot = url;
        }
    }
}

impl Delegate for Stub {
    fn spawn(
        &self,
        _at: &yi_types::plan::doc::TodoAddr,
        _spec: &Delegation,
    ) -> Result<AgentId, String> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        AgentId::new(format!("fuzz-child-{serial}")).map_err(|error| error.to_string())
    }

    fn reap(&self, _agent: &AgentId, _supplied: &[Url]) -> Result<Option<Url>, String> {
        if self.fail_reap.load(Ordering::SeqCst) {
            return Err("the fuzzer said this reap fails".to_owned());
        }
        match self.produced.lock() {
            Ok(produced) => Ok(produced.clone()),
            Err(_) => Err("poisoned".to_owned()),
        }
    }
}

/// `expected` mirrors what the engine may legally have changed: touched moves
/// once per applied op, version on supersede alone, and the spawn floor only
/// ever rises — including across a simulated rehydration.
struct Case {
    store: PlanStore,
    stub: Arc<Stub>,
    crash: Arc<CrashPoint>,
    engine: PlanEngine,
    width: NonZeroUsize,
    expected: HashMap<String, (u64, u64)>,
    spawn_floor: HashMap<String, u32>,
    pad: u32,
    dir: Scratch,
}

enum Bump {
    Fresh,
    Touch,
    Supersede,
    ReadOnly,
}

impl Case {
    fn new(width: NonZeroUsize) -> Result<Self, TestCaseError> {
        let dir = Scratch::new("yi-plan-fuzz").map_err(fail)?;
        let crash = Arc::new(CrashPoint::default());
        let store = PlanStore::open(dir.to_path_buf())
            .map_err(fail)?
            .with_fs(Arc::clone(&crash) as Arc<dyn Fs>);
        let stub = Arc::new(Stub::default());
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).map_err(fail)?;
        let engine = PlanEngine::new(store.clone(), stub.clone())
            .with_width(width)
            .with_cwd(ws);
        Ok(Self {
            store,
            stub,
            crash,
            engine,
            width,
            expected: HashMap::new(),
            spawn_floor: HashMap::new(),
            pad: 0,
            dir,
        })
    }

    fn rehydrate(&mut self) -> Result<(), TestCaseError> {
        self.store = PlanStore::open(self.dir.to_path_buf())
            .map_err(fail)?
            .with_fs(Arc::clone(&self.crash) as Arc<dyn Fs>);
        self.engine = PlanEngine::new(self.store.clone(), self.stub.clone())
            .with_width(self.width)
            .with_cwd(self.dir.join("ws"));
        Ok(())
    }

    /// After a crash the journal is the only truth: whatever it kept is what `touched` and
    /// `version` are expected at from here on.
    fn resync_expected(&mut self) -> Result<(), TestCaseError> {
        for id in self.store.list().map_err(fail)? {
            let plan = self.store.read(&id).map_err(fail)?;
            self.expected
                .insert(id.to_string(), (plan.touched.0, plan.version.0));
        }
        Ok(())
    }

    fn resolve_target(&self, target: Target) -> Result<Option<PlanId>, TestCaseError> {
        match target {
            Target::Root => Ok(None),
            Target::Sub => Ok(self
                .store
                .list()
                .map_err(fail)?
                .into_iter()
                .find(|id| !id.is_root())),
        }
    }

    fn read_target(
        &self,
        plan: &Option<PlanId>,
    ) -> Result<Option<yi_types::plan::doc::Plan>, TestCaseError> {
        if let Some(id) = plan {
            return Ok(Some(self.store.read(id).map_err(fail)?));
        }
        for id in self.store.roots().map_err(fail)? {
            let plan = self.store.read(&id).map_err(fail)?;
            if plan.state == PlanState::Active {
                return Ok(Some(plan));
            }
        }
        Ok(None)
    }

    /// Incident: with no active root an owner's start reads the closed root holding the label
    /// as delegated and touches nothing; the fuzzer took it for no plan and expected a touch.
    fn owner_root(
        &self,
        label: &TodoLabel,
    ) -> Result<Option<yi_types::plan::doc::Plan>, TestCaseError> {
        let mut closed = None;
        for id in self.store.roots().map_err(fail)? {
            let plan = self.store.read(&id).map_err(fail)?;
            let delegated = plan
                .todo(label)
                .is_some_and(|todo| todo.delegation.is_some());
            match plan.state {
                PlanState::Active => return Ok(Some(plan)),
                PlanState::Done if delegated && closed.is_none() => closed = Some(plan),
                _ => {}
            }
        }
        Ok(closed)
    }

    fn family_flight(&self, root: &PlanId) -> Result<usize, TestCaseError> {
        let mut total = 0usize;
        for id in self.store.list().map_err(fail)? {
            let kin = &id == root
                || id
                    .as_str()
                    .strip_prefix(root.as_str())
                    .is_some_and(|rest| rest.starts_with('.'));
            if kin {
                total = total.saturating_add(in_flight(&self.store.read(&id).map_err(fail)?));
            }
        }
        Ok(total)
    }

    fn engine_starts(&self) -> Result<HashMap<String, u64>, TestCaseError> {
        let mut starts = HashMap::new();
        for root in self.store.roots().map_err(fail)? {
            for record in self.store.journal(&root).read().map_err(fail)?.records {
                if record.record.actor == "engine"
                    && record.record.op == "start"
                    && !record.record.extra.contains_key("refusal")
                {
                    let count = starts.entry(record.record.plan.to_string()).or_insert(0);
                    *count = u64::saturating_add(*count, 1);
                }
            }
        }
        Ok(starts)
    }

    fn apply(
        &mut self,
        request: OpRequest,
        bump: Bump,
    ) -> Result<Result<Outcome, PlanOpError>, TestCaseError> {
        let before = self.engine_starts()?;
        let result = self.engine.apply(request);
        if let Ok(outcome) = &result {
            self.check_outcome(outcome, !matches!(bump, Bump::ReadOnly))?;
            let id = outcome.plan.id.to_string();
            match bump {
                Bump::Fresh => {
                    self.expected.insert(id, (1, 1));
                }
                Bump::Touch | Bump::Supersede => {
                    let Some(entry) = self.expected.get_mut(&id) else {
                        return Err(fail(format!("plan {id} applied before it was tracked")));
                    };
                    entry.0 = entry.0.saturating_add(1);
                    if matches!(bump, Bump::Supersede) {
                        entry.1 = entry.1.saturating_add(1);
                    }
                }
                Bump::ReadOnly => {}
            }
            if let Some(sub) = &outcome.subplan {
                self.expected.insert(sub.to_string(), (1, 1));
            }
        }
        // The engine's own starts after an op are applied ops too, one touch on their plan.
        for (id, count) in self.engine_starts()? {
            let started = count.saturating_sub(before.get(&id).copied().unwrap_or(0));
            if started > 0
                && let Some(entry) = self.expected.get_mut(&id)
            {
                entry.0 = entry.0.saturating_add(started);
            }
        }
        Ok(result)
    }

    /// Invariant: the dispatchable-slice contract binds ops that run the
    /// dispatch derivation; `view` is read-only and hands back the full ready
    /// set with no slice, which the campaign golden fixture pins.
    fn check_outcome(&self, outcome: &Outcome, dispatching: bool) -> Result<(), TestCaseError> {
        let file = self.store.read(&outcome.plan.id).map_err(fail)?;
        proptest::prop_assert_eq!(&outcome.plan, &file, "outcome plan disagrees with disk");
        let derived: Vec<TodoLabel> = file
            .ready()
            .into_iter()
            .map(|todo| todo.label.clone())
            .collect();
        proptest::prop_assert_eq!(&outcome.ready, &derived, "ready is not the derived set");
        for held in &outcome.held {
            proptest::prop_assert!(derived.contains(held), "held label {held:?} is not ready");
            proptest::prop_assert!(
                !outcome.dispatched.contains(held),
                "label {held:?} both dispatched and held"
            );
            let delegated = file
                .todo(held)
                .is_some_and(|todo| todo.delegation.is_some());
            proptest::prop_assert!(delegated, "held label {held:?} is not delegated");
        }
        for dispatched in &outcome.dispatched {
            proptest::prop_assert!(
                derived.contains(dispatched),
                "dispatched label {dispatched:?} is not ready"
            );
        }
        if !dispatching {
            return Ok(());
        }
        let root = root_of(&file.id)?;
        let flight = self.family_flight(&root)?;
        let offered = derived
            .iter()
            .filter(|ready| {
                file.todo(ready)
                    .is_some_and(|todo| todo.delegation.is_some())
                    && !outcome.held.contains(ready)
            })
            .count();
        proptest::prop_assert!(
            offered <= self.width.get().saturating_sub(flight),
            "offered {offered} delegated todos with {flight} in flight at width {}",
            self.width
        );
        Ok(())
    }

    fn audit_file(&mut self, id: &PlanId) -> Result<(), TestCaseError> {
        let raw = std::fs::read_to_string(self.store.path(id)).map_err(fail)?;
        proptest::prop_assert!(
            raw.len() <= PLAN_CAP_BYTES,
            "plan {id} checkpoint is {} bytes on disk, over the {PLAN_CAP_BYTES} cap",
            raw.len()
        );
        proptest::prop_assert!(
            !raw.contains("\"ready\":"),
            "plan {id} stores a ready key; ready is derived, never stored"
        );
        let file = self.store.read(id).map_err(fail)?;
        let issues = file.validate();
        proptest::prop_assert!(issues.is_empty(), "plan {id} fails validate: {issues:?}");
        let dots = id.as_str().matches('.').count();
        proptest::prop_assert!(dots <= 1, "plan {id} is deeper than one sub-plan");
        match (&file.tier, dots) {
            (PlanTier::Root, 0) | (PlanTier::Sub { .. }, 1) => {}
            (PlanTier::Root | PlanTier::Sub { .. } | PlanTier::Other { .. }, _) => {
                return Err(fail(format!("plan {id} tier disagrees with its id depth")));
            }
        }
        if dots == 0 {
            let spawns = file.spawns().get();
            let floor = self.spawn_floor.entry(id.to_string()).or_insert(0);
            proptest::prop_assert!(
                spawns >= *floor,
                "plan {id} spawns fell from {floor} to {spawns}; the fuse is monotonic"
            );
            *floor = spawns;
        } else {
            proptest::prop_assert!(
                file.spawns().is_zero(),
                "sub-plan {id} carries spawns; only the root is charged"
            );
        }
        match &file.state {
            PlanState::Active => proptest::prop_assert!(
                !file.finished(),
                "plan {id} is Active with every todo terminal"
            ),
            PlanState::Done => proptest::prop_assert!(
                file.finished(),
                "plan {id} is Done with a non-terminal todo"
            ),
            PlanState::Superseded { .. } | PlanState::Abandoned | PlanState::Other(_) => {}
        }
        for todo in &file.todos {
            match &todo.state {
                TodoState::Done {
                    output: Some(url), ..
                } => proptest::prop_assert!(
                    !matches!(url.scheme(), Scheme::Agent),
                    "todo {:?} is Done with agent url {url}",
                    todo.label
                ),
                TodoState::Failed {
                    last: Some(url), ..
                } => proptest::prop_assert_eq!(
                    url.durability(),
                    Durability::Durable,
                    "todo {:?} is Failed with ephemeral last {}",
                    &todo.label,
                    url
                ),
                TodoState::Pending
                | TodoState::Running { .. }
                | TodoState::Blocked { .. }
                | TodoState::Done { output: None, .. }
                | TodoState::Failed { last: None, .. }
                | TodoState::Abandoned
                | TodoState::Other(_) => {}
            }
        }
        // F0c: no `Done` on the caller's word where a resolution is owed (a contract, or a
        // stated acceptance), whatever op moved it.
        for todo in &file.todos {
            if let TodoState::Done {
                resolution: None, ..
            } = &todo.state
            {
                proptest::prop_assert!(
                    !yi_runtime::plan::state::needs_resolution(todo),
                    "todo {:?} is Done with no resolution and a contract or stated acceptance",
                    todo.label
                );
            }
        }
        // F0c: no `Done { VerifiedDone }` without a committed `pass` verdict on that todo.
        for todo in &file.todos {
            if let TodoState::Done {
                resolution: Some(yi_types::plan::doc::Resolution::VerifiedDone),
                ..
            } = &todo.state
            {
                let root = root_of(id)?;
                let journal = yi_runtime::plan::journal::Journal::open(
                    self.store.journal_path(&root),
                    Arc::new(RealFs),
                );
                let passed = journal.read().map_err(fail)?.records.iter().any(|record| {
                    record.record.op == "done"
                        && record.record.todo.as_ref() == Some(&todo.label)
                        && record
                            .verdict
                            .as_ref()
                            .and_then(|verdict| verdict.get("outcome"))
                            .and_then(|outcome| outcome.as_str())
                            == Some("pass")
                });
                proptest::prop_assert!(
                    passed,
                    "todo {:?} is VerifiedDone with no committed pass verdict",
                    todo.label
                );
            }
        }
        let (touched, version) = *self
            .expected
            .entry(id.to_string())
            .or_insert((file.touched.0, file.version.0));
        proptest::prop_assert_eq!(
            file.touched.0,
            touched,
            "plan {} touched moved without an applied op",
            id
        );
        proptest::prop_assert_eq!(
            file.version.0,
            version,
            "plan {} version moved on an op other than supersede",
            id
        );
        Ok(())
    }

    fn audit(&mut self) -> Result<(), TestCaseError> {
        let ids = self.store.list().map_err(fail)?;
        for id in &ids {
            self.audit_file(id)?;
        }
        for root in ids.iter().filter(|id| id.is_root()) {
            let flight = self.family_flight(root)?;
            proptest::prop_assert!(
                flight <= self.width.get(),
                "family of {root} has {flight} children in flight at width {}",
                self.width
            );
        }
        Ok(())
    }
}

fn owner(plan: Option<PlanId>, op: Op) -> OpRequest {
    OpRequest {
        plan,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

fn act_start(case: &mut Case, target: Target, slot: usize) -> Result<(), TestCaseError> {
    let plan = case.resolve_target(target)?;
    let lbl = label(slot)?;
    let file = match &plan {
        Some(_) => case.read_target(&plan)?,
        None => case.owner_root(&lbl)?,
    };
    if let Some(file) = &file
        && let Some(todo) = file.todo(&lbl)
        && todo.delegation.is_some()
        && matches!(todo.state, TodoState::Pending)
        && case.family_flight(&root_of(&file.id)?)? >= case.width.get()
    {
        // The dispatcher protocol: a delegated todo past the width is held,
        // never started, so the fuzzer holds it too.
        return Ok(());
    }
    // An owner's start on a delegated todo only reads its standing: the engine starts it.
    let delegated = file.is_some_and(|file| {
        file.todo(&lbl)
            .is_some_and(|todo| todo.delegation.is_some())
    });
    let bump = if delegated {
        Bump::ReadOnly
    } else {
        Bump::Touch
    };
    let _refused = case.apply(owner(plan, Op::Start { label: lbl }), bump)?;
    Ok(())
}

fn act_done(
    case: &mut Case,
    target: Target,
    slot: usize,
    pick: OutPick,
) -> Result<(), TestCaseError> {
    let plan = case.resolve_target(target)?;
    let output = match pick {
        OutPick::Nothing => None,
        OutPick::Kernel => Some("kernel://main/surface".parse::<Url>().map_err(fail)?),
        OutPick::Durable(index) => Some(durable_url(index)?),
        OutPick::Agent => Some(AGENT_URL.parse::<Url>().map_err(fail)?),
    };
    let result = case.apply(
        owner(
            plan,
            Op::Done {
                label: label(slot)?,
                output,
            },
        ),
        Bump::Touch,
    )?;
    proptest::prop_assert!(
        !(result.is_ok() && matches!(pick, OutPick::Agent)),
        "done accepted an agent:// output"
    );
    Ok(())
}

fn act_fail(
    case: &mut Case,
    target: Target,
    slot: usize,
    pick: LastPick,
    discard: bool,
) -> Result<(), TestCaseError> {
    let plan = case.resolve_target(target)?;
    let lbl = label(slot)?;
    let reaping = case.read_target(&plan)?.and_then(|file| {
        file.todo(&lbl).and_then(|todo| {
            (todo.delegation.is_some() && matches!(todo.state, TodoState::Running { .. }))
                .then(|| file.id.clone())
        })
    });
    let produced = match pick {
        LastPick::Nothing => None,
        LastPick::Durable(index) => Some(durable_url(index)?),
        LastPick::Ephemeral => Some(EPHEMERAL_URL.parse::<Url>().map_err(fail)?),
    };
    case.stub.set_produced(produced.clone());
    let result = case.apply(
        owner(
            plan,
            Op::Fail {
                label: lbl.clone(),
                cause: "the fuzzer failed it".to_owned(),
                disposition: discard.then_some(Choice::Discarded),
            },
        ),
        Bump::Touch,
    )?;
    case.stub.set_produced(None);
    let Some(reaped_plan) = reaping else {
        return Ok(());
    };
    match (&result, pick) {
        (Ok(outcome), LastPick::Durable(_) | LastPick::Nothing) => {
            let file = case.store.read(&outcome.plan.id).map_err(fail)?;
            let todo = file
                .todo(&lbl)
                .ok_or_else(|| fail("failed todo vanished"))?;
            proptest::prop_assert_eq!(
                &todo.state,
                &TodoState::Failed {
                    cause: "the fuzzer failed it".to_owned(),
                    last: produced,
                },
                "a Failed todo whose child produced something must carry last"
            );
        }
        (Ok(_), LastPick::Ephemeral) => {
            return Err(fail(format!(
                "fail on {reaped_plan} accepted an ephemeral last"
            )));
        }
        (Err(_), LastPick::Nothing | LastPick::Durable(_) | LastPick::Ephemeral) => {}
    }
    Ok(())
}

/// The first four labels slug apart, so a generated document always validates.
/// Append (or init) one contracted todo, start it and drive its `done`: a passing checker
/// lands `Done { VerifiedDone }`, a failing one is refused with a verdict, and either count
/// proves the F0c invariant below is not vacuous.
fn act_verified(case: &mut Case, passes: bool) -> Result<(), TestCaseError> {
    case.pad = case.pad.saturating_add(1);
    let lbl = TodoLabel::new(format!("checked job {}", case.pad)).map_err(fail)?;
    let (id, opening) = match case.read_target(&None)? {
        Some(file) => (file.id, None),
        None => {
            let goal = fuzz_goal()?;
            (case.store.allocate(&goal).map_err(fail)?, Some(goal))
        }
    };
    let spec = TodoSpec {
        label: lbl.clone(),
        after: Vec::new(),
        delegation: None,
        contract: Some(contract_for(case, &id, passes)?),
        children: Vec::new(),
        cites: Default::default(),
    };
    let inserted = match opening {
        Some(goal) => case.apply(
            owner(
                None,
                Op::Init {
                    goal,
                    todos: vec![spec],
                },
            ),
            Bump::Fresh,
        )?,
        None => case.apply(owner(None, Op::Append { todos: vec![spec] }), Bump::Touch)?,
    };
    if inserted.is_err() {
        return Ok(());
    }
    let started = case.apply(owner(None, Op::Start { label: lbl.clone() }), Bump::Touch)?;
    if started.is_err() {
        return Ok(());
    }
    match case.apply(
        owner(
            None,
            Op::Done {
                label: lbl.clone(),
                output: None,
            },
        ),
        Bump::Touch,
    )? {
        Ok(outcome) => {
            let verified = outcome.plan.todo(&lbl).is_some_and(|todo| {
                matches!(
                    todo.state,
                    TodoState::Done {
                        resolution: Some(yi_types::plan::doc::Resolution::VerifiedDone),
                        ..
                    }
                )
            });
            proptest::prop_assert!(verified && passes, "a contracted done landed unverified");
            VERIFIED_DONE.fetch_add(1, Ordering::SeqCst);
        }
        Err(PlanOpError::Refused { verdict, .. }) => {
            proptest::prop_assert!(
                !passes,
                "a passing checker was refused: {}",
                verdict.lines()
            );
            DONE_REFUSED.fetch_add(1, Ordering::SeqCst);
        }
        Err(_) => {}
    }
    Ok(())
}

fn act_import(case: &mut Case, serial: u8, slots: &[usize]) -> Result<(), TestCaseError> {
    let id = PlanId::new(format!("fuzz-import-{}", serial % 3)).map_err(fail)?;
    let mut labels: Vec<&str> = Vec::new();
    for slot in slots {
        let text = LABELS[slot % 4];
        if !labels.contains(&text) {
            labels.push(text);
        }
    }
    if labels.is_empty() {
        labels.push(LABELS[0]);
    }
    let todos: Vec<serde_json::Value> = labels
        .iter()
        .map(|text| serde_json::json!({"label": text, "state": "pending"}))
        .collect();
    let frontmatter = serde_json::json!({
        "format": 1,
        "plan": id.as_str(),
        "goal": "an imported fuzz plan",
        "version": 1,
        "tier": "root",
        "state": "active",
        "todos": todos,
    });
    let document = format!(
        "---\n{frontmatter}\n---\n## {}\nImported by the fuzzer.\n",
        labels[0]
    );
    std::fs::write(case.dir.join("ws").join(format!("{id}.md")), &document).map_err(fail)?;
    let source: Url = format!("local://{id}.md").parse().map_err(fail)?;
    let existed = case.store.list().map_err(fail)?.contains(&id);
    let open = case.store.roots().map_err(fail)?.into_iter().any(|root| {
        root != id
            && case
                .store
                .read(&root)
                .is_ok_and(|plan| plan.state == PlanState::Active)
    });
    let result = case.apply(owner(None, Op::Import { source }), Bump::ReadOnly)?;
    match result {
        Ok(_) => {
            proptest::prop_assert!(!existed, "the live root {id} was imported over");
            proptest::prop_assert!(!open, "{id} was imported active beside an open plan");
            let digest = Digest::of(document.as_bytes());
            let named = format!("artifact:{digest}");
            let genesis = case
                .store
                .journal(&id)
                .read()
                .map_err(fail)?
                .records
                .into_iter()
                .find(|record| record.record.op == "import")
                .ok_or_else(|| fail("no import record"))?;
            proptest::prop_assert_eq!(
                genesis
                    .args
                    .get("artifact")
                    .and_then(serde_json::Value::as_str),
                Some(named.as_str()),
                "the genesis record names the original's digest"
            );
            let blob = case.store.artifacts(&id).get(&digest).map_err(fail)?;
            proptest::prop_assert_eq!(blob, document.into_bytes(), "the artifact is the original");
        }
        Err(error) => {
            let said = error.to_string();
            proptest::prop_assert!(
                existed && said.contains("already format 2")
                    || open && said.contains("already exists and is open"),
                "import of {id} (existed: {existed}, open: {open}) refused: {error}"
            );
        }
    }
    Ok(())
}

fn act(case: &mut Case, action: &Action) -> Result<(), TestCaseError> {
    match action {
        Action::Init { specs } => {
            let goal = fuzz_goal()?;
            let id = case.store.allocate(&goal).map_err(fail)?;
            let _refused = case.apply(
                owner(
                    None,
                    Op::Init {
                        goal,
                        todos: todo_specs(case, Some(&id), specs)?,
                    },
                ),
                Bump::Fresh,
            )?;
        }
        Action::Append { specs } => {
            let id = case.read_target(&None)?.map(|file| file.id);
            let _refused = case.apply(
                owner(
                    None,
                    Op::Append {
                        todos: todo_specs(case, id.as_ref(), specs)?,
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::BulkAppend { count } => {
            let mut todos = Vec::new();
            for _ in 0..*count {
                case.pad = case.pad.saturating_add(1);
                todos.push(TodoSpec {
                    label: TodoLabel::new(format!("pad job {}", case.pad)).map_err(fail)?,
                    after: Vec::new(),
                    delegation: None,
                    contract: None,
                    children: Vec::new(),
                    cites: Default::default(),
                });
            }
            let _refused = case.apply(owner(None, Op::Append { todos }), Bump::Touch)?;
        }
        Action::Drop {
            target,
            slot,
            discard,
        } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Drop {
                        label: label(*slot)?,
                        disposition: discard.then_some(Choice::Discarded),
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::Block {
            target,
            slot,
            note_len,
        } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Block {
                        label: label(*slot)?,
                        on: BlockedOn::User,
                        note: "x".repeat(usize::from(*note_len)),
                        ask: None,
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::Unblock {
            target,
            slot,
            as_user,
        } => {
            let plan = case.resolve_target(*target)?;
            let actor = if *as_user {
                Actor::User("user://1".parse::<Url>().map_err(fail)?)
            } else {
                Actor::Owner
            };
            let _refused = case.apply(
                OpRequest {
                    plan,
                    actor,
                    op: Op::Unblock {
                        label: label(*slot)?,
                        answer: None,
                    },
                    request_id: None,
                    expected_revision: None,
                },
                Bump::Touch,
            )?;
        }
        Action::Reorder { target, rotate } => {
            let plan = case.resolve_target(*target)?;
            let mut labels: Vec<TodoLabel> = match case.read_target(&plan)? {
                Some(file) => file.todos.iter().map(|todo| todo.label.clone()).collect(),
                None => Vec::new(),
            };
            if !labels.is_empty() {
                let split = rotate % labels.len();
                labels.rotate_left(split);
            }
            let _refused = case.apply(owner(plan, Op::Reorder { labels }), Bump::Touch)?;
        }
        Action::AddEdge { target, from, to } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(
                owner(
                    plan,
                    Op::AddEdge {
                        todo: label(*from)?,
                        after: label(*to)?,
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::Start { target, slot } => act_start(case, *target, *slot)?,
        Action::Done {
            target,
            slot,
            output,
        } => act_done(case, *target, *slot, *output)?,
        Action::Fail {
            target,
            slot,
            produced,
            discard,
        } => act_fail(case, *target, *slot, *produced, *discard)?,
        Action::Retry { target, slot } => {
            let plan = case.resolve_target(*target)?;
            // Incident: retry inside a Done sub-plan shrank to a six-op
            // sequence leaving the plan Done with a Pending todo — `framed`
            // never reactivates a finished plan; held until the engine does.
            if case
                .read_target(&plan)?
                .is_some_and(|file| file.state == PlanState::Done)
            {
                return Ok(());
            }
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Retry {
                        label: label(*slot)?,
                        delegation: None,
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::Decompose {
            target,
            slot,
            specs,
        } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Decompose {
                        label: label(*slot)?,
                        todos: todo_specs(case, None, specs)?,
                    },
                ),
                Bump::Touch,
            )?;
        }
        Action::Supersede {
            target,
            specs,
            reap_fails,
        } => {
            let plan = case.resolve_target(*target)?;
            let id = case.read_target(&plan)?.map(|file| file.id);
            case.stub.fail_reap.store(*reap_fails, Ordering::SeqCst);
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Supersede {
                        reason: "the fuzzer changed its mind".to_owned(),
                        todos: todo_specs(case, id.as_ref(), specs)?,
                    },
                ),
                Bump::Supersede,
            )?;
            case.stub.fail_reap.store(false, Ordering::SeqCst);
        }
        Action::View { target } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(owner(plan, Op::View { full: false }), Bump::ReadOnly)?;
        }
        Action::ChildProbe { slot } => {
            let result = case.engine.apply(OpRequest {
                plan: None,
                actor: Actor::Child(AgentId::new("probe-child").map_err(fail)?),
                op: Op::Start {
                    label: label(*slot)?,
                },
                request_id: None,
                expected_revision: None,
            });
            proptest::prop_assert!(
                matches!(result, Err(PlanOpError::NotOwner { .. })),
                "a child mutated the plan"
            );
        }
        Action::Rehydrate => case.rehydrate()?,
        Action::FuseReset => {
            let refused = case.engine.apply(owner(None, Op::FuseReset));
            proptest::prop_assert!(
                matches!(refused, Err(PlanOpError::NotOwner { .. })),
                "the owner reset the fuse: {refused:?}"
            );
            let result = case.apply(
                OpRequest {
                    plan: None,
                    actor: Actor::User("user://1".parse::<Url>().map_err(fail)?),
                    op: Op::FuseReset,
                    request_id: None,
                    expected_revision: None,
                },
                Bump::Touch,
            )?;
            // Incident: a crash left a delegated todo ready and unstarted; the reset's own
            // dispatch starts it, so the fuse reads exactly what that dispatch spent.
            if let Ok(outcome) = result {
                let spawns = outcome.plan.spawns().get();
                proptest::prop_assert_eq!(
                    usize::try_from(spawns).map_err(fail)?,
                    outcome.spawned.len(),
                    "the reset fuse counts more than its own dispatch spent"
                );
                case.spawn_floor.insert(outcome.plan.id.to_string(), spawns);
            }
        }
        Action::Repair => {
            let starts = |case: &Case| -> Result<u64, TestCaseError> {
                Ok(case.engine_starts()?.values().sum())
            };
            let spawns_before = case.stub.serial.load(Ordering::SeqCst);
            let starts_before = starts(case)?;
            let result = case.apply(
                owner(
                    None,
                    Op::Repair {
                        resolutions: Vec::new(),
                    },
                ),
                Bump::ReadOnly,
            )?;
            proptest::prop_assert!(
                matches!(
                    result,
                    Ok(_) | Err(PlanOpError::NoPlan | PlanOpError::NotActive { .. })
                ),
                "repair without resolutions refused: {result:?}"
            );
            // Incident: a crash left a delegated todo ready and unstarted and the scheduler's
            // pass after the repair (D224) started it; a spawn with no engine start is repair's.
            let spawned = case
                .stub
                .serial
                .load(Ordering::SeqCst)
                .saturating_sub(spawns_before);
            let started = starts(case)?.saturating_sub(starts_before);
            proptest::prop_assert!(
                u64::from(spawned) <= started,
                "repair spawned {spawned} children beyond the scheduler's {started} starts"
            );
        }
        Action::Crash { at_sync, specs } => {
            if *at_sync {
                case.crash.fail_sync.store(true, Ordering::SeqCst);
            } else {
                case.crash.fail_write.store(true, Ordering::SeqCst);
            }
            let result = case.engine.apply(owner(
                None,
                Op::Append {
                    todos: todo_specs(case, None, specs)?,
                },
            ));
            case.crash.fail_sync.store(false, Ordering::SeqCst);
            case.crash.fail_write.store(false, Ordering::SeqCst);
            proptest::prop_assert!(
                result.is_err(),
                "an injected journal failure was acknowledged: {:?}",
                result.map(|outcome| outcome.plan.touched)
            );
            case.rehydrate()?;
            case.resync_expected()?;
        }
        Action::Import { serial, slots } => act_import(case, *serial, slots)?,
        Action::Verified { passes } => act_verified(case, *passes)?,
    }
    Ok(())
}

fn run_case(width: usize, actions: &[Action]) -> Result<(), TestCaseError> {
    let width = NonZeroUsize::new(width.clamp(1, 3)).unwrap_or(NonZeroUsize::MIN);
    let mut case = Case::new(width)?;
    for action in actions {
        act(&mut case, action)?;
        case.audit()?;
    }
    Ok(())
}

fn spec_strategy() -> impl Strategy<Value = SpecPick> {
    (
        0..LABELS.len(),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![Just(None), (0..LABELS.len()).prop_map(Some)],
        prop_oneof![4 => Just(None), 1 => any::<bool>().prop_map(Some)],
    )
        .prop_map(|(slot, delegated, declares, after, contract)| SpecPick {
            slot,
            delegated,
            declares,
            after,
            contract,
        })
}

fn specs_strategy() -> impl Strategy<Value = Vec<SpecPick>> {
    prop::collection::vec(spec_strategy(), 0..4)
}

fn target_strategy() -> impl Strategy<Value = Target> {
    prop_oneof![3 => Just(Target::Root), 1 => Just(Target::Sub)]
}

fn out_strategy() -> impl Strategy<Value = OutPick> {
    prop_oneof![
        3 => Just(OutPick::Nothing),
        1 => Just(OutPick::Kernel),
        2 => (0..DURABLE_URLS.len()).prop_map(OutPick::Durable),
        1 => Just(OutPick::Agent),
    ]
}

fn last_strategy() -> impl Strategy<Value = LastPick> {
    prop_oneof![
        2 => Just(LastPick::Nothing),
        3 => (0..DURABLE_URLS.len()).prop_map(LastPick::Durable),
        1 => Just(LastPick::Ephemeral),
    ]
}

fn slot_strategy() -> impl Strategy<Value = usize> {
    0..LABELS.len()
}

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        3 => specs_strategy().prop_map(|specs| Action::Init { specs }),
        3 => specs_strategy().prop_map(|specs| Action::Append { specs }),
        1 => (300u16..900u16).prop_map(|count| Action::BulkAppend { count }),
        2 => (target_strategy(), slot_strategy(), any::<bool>())
            .prop_map(|(target, slot, discard)| Action::Drop { target, slot, discard }),
        2 => (target_strategy(), slot_strategy(), 0u16..1500u16)
            .prop_map(|(target, slot, note_len)| Action::Block { target, slot, note_len }),
        2 => (target_strategy(), slot_strategy(), any::<bool>())
            .prop_map(|(target, slot, as_user)| Action::Unblock { target, slot, as_user }),
        1 => (target_strategy(), 0usize..8usize)
            .prop_map(|(target, rotate)| Action::Reorder { target, rotate }),
        2 => (target_strategy(), slot_strategy(), slot_strategy())
            .prop_map(|(target, from, to)| Action::AddEdge { target, from, to }),
        6 => (target_strategy(), slot_strategy())
            .prop_map(|(target, slot)| Action::Start { target, slot }),
        5 => (target_strategy(), slot_strategy(), out_strategy())
            .prop_map(|(target, slot, output)| Action::Done { target, slot, output }),
        4 => (target_strategy(), slot_strategy(), last_strategy(), any::<bool>())
            .prop_map(|(target, slot, produced, discard)| Action::Fail { target, slot, produced, discard }),
        3 => (target_strategy(), slot_strategy())
            .prop_map(|(target, slot)| Action::Retry { target, slot }),
        2 => (target_strategy(), slot_strategy(), specs_strategy())
            .prop_map(|(target, slot, specs)| Action::Decompose { target, slot, specs }),
        2 => (target_strategy(), specs_strategy(), any::<bool>())
            .prop_map(|(target, specs, reap_fails)| Action::Supersede { target, specs, reap_fails }),
        1 => target_strategy().prop_map(|target| Action::View { target }),
        1 => slot_strategy().prop_map(|slot| Action::ChildProbe { slot }),
        1 => Just(Action::Rehydrate),
        1 => Just(Action::FuseReset),
        1 => Just(Action::Repair),
        1 => (any::<bool>(), specs_strategy())
            .prop_map(|(at_sync, specs)| Action::Crash { at_sync, specs }),
        1 => (any::<u8>(), prop::collection::vec(slot_strategy(), 0..4))
            .prop_map(|(serial, slots)| Action::Import { serial, slots }),
        1 => any::<bool>().prop_map(|passes| Action::Verified { passes }),
    ]
}

fn sequence_strategy() -> impl Strategy<Value = (usize, Vec<Action>)> {
    (
        1usize..=3usize,
        prop::collection::vec(action_strategy(), 1..MAX_ACTIONS),
    )
}

#[test]
fn random_op_sequences_hold_every_invariant() -> Result<(), Box<dyn Error>> {
    let mut config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = CASES;
    }
    let mut runner = TestRunner::new(config);
    runner
        .run(&sequence_strategy(), |(width, actions)| {
            run_case(width, &actions)
        })
        .map_err(|error| format!("{error}"))?;
    // The F0c invariant is only evidence when the lane reached both outcomes.
    let (verified, refused) = (
        VERIFIED_DONE.load(Ordering::SeqCst),
        DONE_REFUSED.load(Ordering::SeqCst),
    );
    if verified == 0 || refused == 0 {
        return Err(format!(
            "the lane reached {verified} verified completions and {refused} refused verdicts; both must be reached"
        )
        .into());
    }
    Ok(())
}

const DELEGATED: SpecPick = SpecPick {
    slot: 0,
    delegated: true,
    declares: false,
    after: None,
    contract: None,
};

/// Dies with the fuzzer charging the scheduler's start to the op before it: a sync that failed
/// after the write kept a delegated todo, and the reset's or repair's dispatch pass started it.
#[test]
fn a_crash_kept_todo_is_started_by_the_next_dispatch_pass() -> Result<(), Box<dyn Error>> {
    for last in [Action::FuseReset, Action::Repair] {
        let actions = [
            Action::Init { specs: Vec::new() },
            Action::Crash {
                at_sync: true,
                specs: vec![DELEGATED],
            },
            last,
        ];
        run_case(1, &actions).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Dies with the fuzzer reading no plan where the engine reads the closed root holding the
/// label: the owner's start on its delegated todo only reads its standing and touches nothing.
#[test]
fn a_start_in_a_closed_root_reads_its_standing() -> Result<(), Box<dyn Error>> {
    let closing = [
        Action::Fail {
            target: Target::Root,
            slot: 0,
            produced: LastPick::Nothing,
            discard: false,
        },
        Action::Done {
            target: Target::Root,
            slot: 0,
            output: OutPick::Nothing,
        },
    ];
    for close in closing {
        let actions = [
            Action::Init {
                specs: vec![DELEGATED],
            },
            close,
            Action::Start {
                target: Target::Root,
                slot: 0,
            },
        ];
        run_case(1, &actions).map_err(|error| error.to_string())?;
    }
    Ok(())
}

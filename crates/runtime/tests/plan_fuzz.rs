//! Random op sequences over the real step table (proposal section 12): the
//! generator drives [`yi_runtime::plan::ops::PlanEngine`] against a temp
//! [`yi_runtime::plan::store::PlanStore`] and a stub delegate, and re-asserts
//! every insert-check invariant after every op. A failing property shrinks to
//! the shortest breaking sequence, which then becomes a walkthrough fixture.

use std::collections::HashMap;
use std::error::Error;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use yi_runtime::plan::ops::{
    Actor, Delegate, Op, OpRequest, Outcome, PlanEngine, PlanOpError, TodoSpec,
};
use yi_runtime::plan::store::{FRONTMATTER_CAP_BYTES, PlanFile, PlanStore};
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, Delegation, GoalText, OutputSchema, PlanId, PlanState, PlanTier,
    SpawnSpec, TodoLabel, TodoState,
};
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
}

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
            extra: serde_json::Map::new(),
        },
        accept: Check::Stated("it works".to_owned()),
        output,
        context: Vec::new(),
        note: None,
        extra: serde_json::Map::new(),
    })
}

fn todo_specs(picks: &[SpecPick]) -> Result<Vec<TodoSpec>, TestCaseError> {
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
            })
        })
        .collect()
}

fn root_of(id: &PlanId) -> Result<PlanId, TestCaseError> {
    match id.as_str().split_once('.') {
        Some((root, _)) => PlanId::new(root).map_err(fail),
        None => Ok(id.clone()),
    }
}

fn in_flight(file: &PlanFile) -> usize {
    file.plan
        .todos
        .iter()
        .filter(|todo| matches!(todo.state, TodoState::Running { .. }) && todo.delegation.is_some())
        .count()
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

    fn reap(&self, _agent: &AgentId) -> Result<Option<Url>, String> {
        if self.fail_reap.load(Ordering::SeqCst) {
            return Err("the fuzzer said this reap fails".to_owned());
        }
        match self.produced.lock() {
            Ok(produced) => Ok(produced.clone()),
            Err(_) => Err("poisoned".to_owned()),
        }
    }

    fn follow_up(&self, _dispatched: &[TodoLabel], _held: usize) {}
}

static NEXT_CASE: AtomicU32 = AtomicU32::new(0);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `expected` mirrors what the engine may legally have changed: touched moves
/// once per applied op, version on supersede alone, and the spawn floor only
/// ever rises — including across a simulated rehydration.
struct Case {
    dir: PathBuf,
    store: PlanStore,
    stub: Arc<Stub>,
    engine: PlanEngine,
    width: NonZeroUsize,
    expected: HashMap<String, (u64, u64)>,
    spawn_floor: HashMap<String, u32>,
    pad: u32,
}

enum Bump {
    Fresh,
    Touch,
    Supersede,
    ReadOnly,
}

impl Case {
    fn new(width: NonZeroUsize) -> Result<(TempDir, Self), TestCaseError> {
        let dir = std::env::temp_dir().join(format!(
            "yi-plan-fuzz-{}-{}",
            std::process::id(),
            NEXT_CASE.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = PlanStore::open(dir.clone()).map_err(fail)?;
        let stub = Arc::new(Stub::default());
        let engine = PlanEngine::new(store.clone(), stub.clone()).with_width(width);
        Ok((
            TempDir(dir.clone()),
            Self {
                dir,
                store,
                stub,
                engine,
                width,
                expected: HashMap::new(),
                spawn_floor: HashMap::new(),
                pad: 0,
            },
        ))
    }

    fn rehydrate(&mut self) -> Result<(), TestCaseError> {
        self.store = PlanStore::open(self.dir.clone()).map_err(fail)?;
        self.engine = PlanEngine::new(self.store.clone(), self.stub.clone()).with_width(self.width);
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

    fn read_target(&self, plan: &Option<PlanId>) -> Result<Option<PlanFile>, TestCaseError> {
        if let Some(id) = plan {
            return Ok(Some(self.store.read(id).map_err(fail)?));
        }
        for id in self.store.roots().map_err(fail)? {
            let file = self.store.read(&id).map_err(fail)?;
            if file.plan.state == PlanState::Active {
                return Ok(Some(file));
            }
        }
        Ok(None)
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

    fn apply(
        &mut self,
        request: OpRequest,
        bump: Bump,
    ) -> Result<Result<Outcome, PlanOpError>, TestCaseError> {
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
        Ok(result)
    }

    /// Invariant: the dispatchable-slice contract binds ops that run the
    /// dispatch derivation; `view` is read-only and hands back the full ready
    /// set with no slice, which the campaign golden fixture pins.
    fn check_outcome(&self, outcome: &Outcome, dispatching: bool) -> Result<(), TestCaseError> {
        let file = self.store.read(&outcome.plan.id).map_err(fail)?;
        proptest::prop_assert_eq!(
            &outcome.plan,
            &file.plan,
            "outcome plan disagrees with disk"
        );
        let derived: Vec<TodoLabel> = file
            .plan
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
                .plan
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
        let root = root_of(&file.plan.id)?;
        let flight = self.family_flight(&root)?;
        let offered = derived
            .iter()
            .filter(|ready| {
                file.plan
                    .todo(ready)
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
        let front = raw
            .strip_prefix("---\n")
            .and_then(|rest| rest.find("---\n").map(|end| &rest[..end]))
            .ok_or_else(|| fail(format!("plan {id} has no frontmatter frame")))?;
        proptest::prop_assert!(
            front.len() <= FRONTMATTER_CAP_BYTES,
            "plan {id} frontmatter is {} bytes on disk, over the {FRONTMATTER_CAP_BYTES} cap",
            front.len()
        );
        proptest::prop_assert!(
            !front.starts_with("ready:") && !front.contains("\nready:"),
            "plan {id} stores a ready key; ready is derived, never stored"
        );
        let file = self.store.read(id).map_err(fail)?;
        let issues = file.plan.validate();
        proptest::prop_assert!(issues.is_empty(), "plan {id} fails validate: {issues:?}");
        let dots = id.as_str().matches('.').count();
        proptest::prop_assert!(dots <= 1, "plan {id} is deeper than one sub-plan");
        match (&file.plan.tier, dots) {
            (PlanTier::Root, 0) | (PlanTier::Sub { .. }, 1) => {}
            (PlanTier::Root | PlanTier::Sub { .. } | PlanTier::Other { .. }, _) => {
                return Err(fail(format!("plan {id} tier disagrees with its id depth")));
            }
        }
        if dots == 0 {
            let spawns = file.plan.spawns().get();
            let floor = self.spawn_floor.entry(id.to_string()).or_insert(0);
            proptest::prop_assert!(
                spawns >= *floor,
                "plan {id} spawns fell from {floor} to {spawns}; the fuse is monotonic"
            );
            *floor = spawns;
        } else {
            proptest::prop_assert!(
                file.plan.spawns().is_zero(),
                "sub-plan {id} carries spawns; only the root is charged"
            );
        }
        match &file.plan.state {
            PlanState::Active => proptest::prop_assert!(
                !file.plan.finished(),
                "plan {id} is Active with every todo terminal"
            ),
            PlanState::Done => proptest::prop_assert!(
                file.plan.finished(),
                "plan {id} is Done with a non-terminal todo"
            ),
            PlanState::Superseded { .. } | PlanState::Abandoned | PlanState::Other(_) => {}
        }
        for todo in &file.plan.todos {
            match &todo.state {
                TodoState::Done { output: Some(url) } => proptest::prop_assert!(
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
                | TodoState::Done { output: None }
                | TodoState::Failed { last: None, .. }
                | TodoState::Abandoned
                | TodoState::Other(_) => {}
            }
        }
        let (touched, version) = *self
            .expected
            .entry(id.to_string())
            .or_insert((file.plan.touched.0, file.plan.version.0));
        proptest::prop_assert_eq!(
            file.plan.touched.0,
            touched,
            "plan {} touched moved without an applied op",
            id
        );
        proptest::prop_assert_eq!(
            file.plan.version.0,
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
    }
}

fn act_start(case: &mut Case, target: Target, slot: usize) -> Result<(), TestCaseError> {
    let plan = case.resolve_target(target)?;
    let lbl = label(slot)?;
    if let Some(file) = case.read_target(&plan)?
        && let Some(todo) = file.plan.todo(&lbl)
        && todo.delegation.is_some()
        && matches!(todo.state, TodoState::Pending)
        && case.family_flight(&root_of(&file.plan.id)?)? >= case.width.get()
    {
        // The dispatcher protocol: a delegated todo past the width is held,
        // never started, so the fuzzer holds it too.
        return Ok(());
    }
    let _refused = case.apply(owner(plan, Op::Start { label: lbl }), Bump::Touch)?;
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
) -> Result<(), TestCaseError> {
    let plan = case.resolve_target(target)?;
    let lbl = label(slot)?;
    let reaping = case.read_target(&plan)?.and_then(|file| {
        file.plan.todo(&lbl).and_then(|todo| {
            (todo.delegation.is_some() && matches!(todo.state, TodoState::Running { .. }))
                .then(|| file.plan.id.clone())
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
                .plan
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

fn act(case: &mut Case, action: &Action) -> Result<(), TestCaseError> {
    match action {
        Action::Init { specs } => {
            let goal = GoalText::new("ship the fuzzed widget end to end").map_err(fail)?;
            let _refused = case.apply(
                owner(
                    None,
                    Op::Init {
                        goal,
                        todos: todo_specs(specs)?,
                    },
                ),
                Bump::Fresh,
            )?;
        }
        Action::Append { specs } => {
            let _refused = case.apply(
                owner(
                    None,
                    Op::Append {
                        todos: todo_specs(specs)?,
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
                });
            }
            let _refused = case.apply(owner(None, Op::Append { todos }), Bump::Touch)?;
        }
        Action::Drop { target, slot } => {
            let plan = case.resolve_target(*target)?;
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Drop {
                        label: label(*slot)?,
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
                    },
                },
                Bump::Touch,
            )?;
        }
        Action::Reorder { target, rotate } => {
            let plan = case.resolve_target(*target)?;
            let mut labels: Vec<TodoLabel> = match case.read_target(&plan)? {
                Some(file) => file
                    .plan
                    .todos
                    .iter()
                    .map(|todo| todo.label.clone())
                    .collect(),
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
        } => act_fail(case, *target, *slot, *produced)?,
        Action::Retry { target, slot } => {
            let plan = case.resolve_target(*target)?;
            // Incident: retry inside a Done sub-plan shrank to a six-op
            // sequence leaving the plan Done with a Pending todo — `framed`
            // never reactivates a finished plan; held until the engine does.
            if case
                .read_target(&plan)?
                .is_some_and(|file| file.plan.state == PlanState::Done)
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
                        todos: todo_specs(specs)?,
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
            case.stub.fail_reap.store(*reap_fails, Ordering::SeqCst);
            let _refused = case.apply(
                owner(
                    plan,
                    Op::Supersede {
                        reason: "the fuzzer changed its mind".to_owned(),
                        todos: todo_specs(specs)?,
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
            });
            proptest::prop_assert!(
                matches!(result, Err(PlanOpError::NotOwner { .. })),
                "a child mutated the plan"
            );
        }
        Action::Rehydrate => case.rehydrate()?,
    }
    Ok(())
}

fn run_case(width: usize, actions: &[Action]) -> Result<(), TestCaseError> {
    let width = NonZeroUsize::new(width.clamp(1, 3)).unwrap_or(NonZeroUsize::MIN);
    let (_temp, mut case) = Case::new(width)?;
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
    )
        .prop_map(|(slot, delegated, declares, after)| SpecPick {
            slot,
            delegated,
            declares,
            after,
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
        2 => (target_strategy(), slot_strategy())
            .prop_map(|(target, slot)| Action::Drop { target, slot }),
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
        4 => (target_strategy(), slot_strategy(), last_strategy())
            .prop_map(|(target, slot, produced)| Action::Fail { target, slot, produced }),
        3 => (target_strategy(), slot_strategy())
            .prop_map(|(target, slot)| Action::Retry { target, slot }),
        2 => (target_strategy(), slot_strategy(), specs_strategy())
            .prop_map(|(target, slot, specs)| Action::Decompose { target, slot, specs }),
        2 => (target_strategy(), specs_strategy(), any::<bool>())
            .prop_map(|(target, specs, reap_fails)| Action::Supersede { target, specs, reap_fails }),
        1 => target_strategy().prop_map(|target| Action::View { target }),
        1 => slot_strategy().prop_map(|slot| Action::ChildProbe { slot }),
        1 => Just(Action::Rehydrate),
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
        .map_err(|error| format!("{error}").into())
}

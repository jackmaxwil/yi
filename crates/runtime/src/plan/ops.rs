//! The plan engine (plan sections 5.3 and 5.6): one transaction per request over the root's journal.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use yi_types::plan::canonical::canonical_digest;
use yi_types::plan::doc::{
    AgentId, Delegation, DocError, GoalText, Plan, PlanId, PlanIssue, PlanState, RetryCount,
    SPAWN_CAP, Spawns, TodoAddr, TodoLabel, TodoState, TodoStateName, TouchCount,
};
use yi_types::plan::ledger::{EffectId, JournalRecord, PlanOpRecord, RequestId};
use yi_types::url::Url;

pub use yi_types::plan::op::{Op, Reconciliation, Resolution, Resolve, SetRow, TodoSpec};

use super::journal::{Journal, has_record};
pub use super::output::OutputResolve;
use super::recovery::{self, Liveness};
use super::state::{
    self, Decided, KIND_IMPORT, KIND_SPAWN_INTENT, KIND_SPAWN_RESULT, RootState, leaving_running,
    root_of,
};
use super::store::{Loaded, PlanStore, StoreError, draft};
use super::table::{
    OpKind, Refusal, admit, check_actor, check_plan_state, check_terminal, in_flight, op_name,
    ready_labels,
};
use yi_types::plan::op::Reaped;

pub(super) const OWNER_AGENT: &str = "main";

const CHILD_SUFFIX_MAX: u32 = 9_999;

/// Bytes a rehearsed record may still grow by after the reaps: at most the dispatch width
/// (8) of `Reaped.last` urls the host mints as `history://<agent>`; the commit seal backstops.
const REAP_ENVELOPE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Owner,
    Child(AgentId),
    User(Url),
    Host,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpRequest {
    pub plan: Option<PlanId>,
    pub actor: Actor,
    pub op: Op,
    pub request_id: Option<RequestId>,
    pub expected_revision: Option<TouchCount>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub plan: Plan,
    pub ready: Vec<TodoLabel>,
    pub dispatched: Vec<TodoLabel>,
    pub held: Vec<TodoLabel>,
    pub spawned: Vec<Url>,
    pub reaped: Vec<Url>,
    pub subplan: Option<PlanId>,
    pub notices: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanOpError {
    #[error("no plan is open; init opens one")]
    NoPlan,
    #[error("plan {id} already exists and is open")]
    PlanExists { id: PlanId },
    #[error("no todo labelled {label:?} in plan {plan}")]
    UnknownLabel { plan: PlanId, label: TodoLabel },
    #[error("only the plan owner may {op:?}; propose to the owner instead")]
    NotOwner { op: OpKind },
    #[error("{} is illegal for todo {label} in state {from}", op_name(*op))]
    IllegalStep {
        label: TodoLabel,
        from: TodoStateName,
        op: OpKind,
    },
    #[error("todo {label:?} carries unknown state {state:?}, which admits no op")]
    UnknownState { label: TodoLabel, state: String },
    #[error("todo label {label:?} is not unique in the plan")]
    LabelNotUnique { label: TodoLabel },
    #[error("plan update refused: {issue}")]
    Invalid { issue: PlanIssue },
    #[error("reorder names {got} labels; a full permutation of all {expected} is required")]
    NotAPermutation { got: usize, expected: usize },
    #[error("DepthExhausted: sub-plan {plan} cannot open a child plan")]
    DepthExhausted { plan: PlanId },
    #[error(
        "spawn ceiling exhausted: {spent} of cap {cap} spent",
        spent = spent.get(),
        cap = cap.get()
    )]
    SpawnCeilingExhausted { spent: Spawns, cap: Spawns },
    #[error(
        "retries exhausted for {label:?}: {spent} of cap {cap} spent",
        spent = spent.0,
        cap = cap.0
    )]
    RetriesExhausted {
        label: TodoLabel,
        spent: RetryCount,
        cap: RetryCount,
    },
    #[error("attempts exhausted for {label:?}")]
    AttemptsExhausted { label: TodoLabel },
    #[error("start refused for todo {label:?}: after edge {after:?} is not cleared")]
    UnmetEdge { label: TodoLabel, after: TodoLabel },
    #[error("start refused: {0} (the dispatch width admits no more)")]
    Admission(Refusal),
    #[error("plan {id} is not active; a plan that is not active accepts view alone")]
    NotActive { id: PlanId, state: PlanState },
    #[error("could not spawn a child at {at}: {reason}")]
    SpawnFailed { at: TodoAddr, reason: String },
    #[error("could not reap child {agent}: {reason}")]
    ReapFailed { agent: AgentId, reason: String },
    #[error("terminal record for {label:?} cannot carry ephemeral url {url}")]
    EphemeralTerminal { label: TodoLabel, url: Url },
    #[error(
        "delegation for {label:?} declares output schema {schema}; done requires an output reference"
    )]
    MissingDeclaredOutput { label: TodoLabel, schema: Url },
    #[error("done for {label:?} names output {url}, which did not resolve: {cause}")]
    UnresolvedOutput {
        label: TodoLabel,
        url: Url,
        cause: String,
    },
    #[error("delegation for {label:?} declares schema {schema}, which is unusable: {cause}")]
    UnusableSchema {
        label: TodoLabel,
        schema: Box<Url>,
        cause: String,
    },
    #[error("output {url} of {label:?} does not satisfy schema {schema}: {detail}")]
    OutputMismatch {
        label: TodoLabel,
        url: Box<Url>,
        schema: Box<Url>,
        detail: String,
    },
    #[error("expected revision {expected}, the plan is at {current}")]
    StaleRevision { expected: u64, current: u64 },
    #[error("request {request_id} was already recorded with different arguments")]
    RequestIdReused { request_id: RequestId },
    #[error("request {request_id} was refused when it first ran: {detail}")]
    RecordedRefusal {
        request_id: RequestId,
        code: String,
        detail: String,
    },
    #[error(
        "todo {label:?} carries spawn intent {effect} with no result; run `yi plan repair` before starting it again"
    )]
    NeedsReconciliation { label: TodoLabel, effect: EffectId },
    #[error("start of delegated todo {label:?} has no committed spawn behind it")]
    StartWithoutSpawn { label: TodoLabel },
    #[error("decompose of {label:?} carries no sub-plan id")]
    SubplanUndecided { label: TodoLabel },
    #[error("{} is never journaled as an applied op", op_name(*op))]
    NotJournaled { op: OpKind },
    #[error("todo {label:?} needs no reconciliation")]
    NotReconcilable { label: TodoLabel },
    #[error("import refused: {0}")]
    Import(#[from] super::import::ImportError),
    #[error("record did not serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("{0}")]
    Canonical(#[from] yi_types::plan::canonical::CanonicalError),
    #[error("{0}")]
    Reduce(#[from] state::ReduceError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Doc(#[from] DocError),
}

impl From<super::journal::JournalError> for PlanOpError {
    fn from(error: super::journal::JournalError) -> Self {
        Self::Store(StoreError::Journal(error))
    }
}

impl PlanOpError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoPlan => "no_plan",
            Self::PlanExists { .. } => "plan_exists",
            Self::UnknownLabel { .. } => "unknown_label",
            Self::NotOwner { .. } => "not_owner",
            Self::IllegalStep { .. } => "illegal_step",
            Self::UnknownState { .. } => "unknown_state",
            Self::LabelNotUnique { .. } => "label_not_unique",
            Self::Invalid { .. } => "invalid",
            Self::NotAPermutation { .. } => "not_a_permutation",
            Self::DepthExhausted { .. } => "depth_exhausted",
            Self::SpawnCeilingExhausted { .. } => "spawn_ceiling_exhausted",
            Self::RetriesExhausted { .. } => "retries_exhausted",
            Self::AttemptsExhausted { .. } => "attempts_exhausted",
            Self::UnmetEdge { .. } => "unmet_edge",
            Self::Admission(_) => "admission",
            Self::NotActive { .. } => "not_active",
            Self::SpawnFailed { .. } => "spawn_failed",
            Self::ReapFailed { .. } => "reap_failed",
            Self::EphemeralTerminal { .. } => "ephemeral_terminal",
            Self::MissingDeclaredOutput { .. } => "missing_declared_output",
            Self::UnresolvedOutput { .. } => "unresolved_output",
            Self::UnusableSchema { .. } => "unusable_schema",
            Self::OutputMismatch { .. } => "output_mismatch",
            Self::StaleRevision { .. } => "stale_revision",
            Self::RequestIdReused { .. } => "request_id_reused",
            Self::RecordedRefusal { .. } => "recorded_refusal",
            Self::NeedsReconciliation { .. } => "needs_reconciliation",
            Self::StartWithoutSpawn { .. } => "start_without_spawn",
            Self::SubplanUndecided { .. } => "subplan_undecided",
            Self::NotJournaled { .. } => "not_journaled",
            Self::NotReconcilable { .. } => "not_reconcilable",
            Self::Import(_) => "import",
            Self::Serialize(_) | Self::Canonical(_) => "serialize",
            Self::Reduce(_) => "reduce",
            Self::Store(_) => "store",
            Self::Doc(_) => "doc",
        }
    }

    fn is_recordable(&self) -> bool {
        !matches!(
            self,
            Self::Store(_)
                | Self::Serialize(_)
                | Self::Canonical(_)
                | Self::Reduce(_)
                | Self::RecordedRefusal { .. }
                | Self::RequestIdReused { .. }
        )
    }
}

pub trait Delegate: Send + Sync {
    fn spawn(&self, at: &TodoAddr, delegation: &Delegation) -> Result<AgentId, String>;
    /// `supplied` is the delegation's context, so the seam that frees the child
    /// is also the one that can say how much of it the child ever read.
    fn reap(&self, agent: &AgentId, supplied: &[Url]) -> Result<Option<Url>, String>;
    fn follow_up(&self, dispatched: &[TodoLabel], held: usize);
}

pub trait OpSink: Send + Sync {
    fn record(&self, record: PlanOpRecord) -> Result<(), String>;
}

pub fn dispatch_width(cores: NonZeroUsize) -> NonZeroUsize {
    NonZeroUsize::new(cores.get().saturating_sub(1).clamp(1, 8)).unwrap_or(NonZeroUsize::MIN)
}

#[derive(Debug, Default)]
struct Delta {
    spawned: Vec<Url>,
    reaped: Vec<Url>,
    subplan: Option<PlanId>,
    notices: Vec<String>,
}

struct Txn {
    root: PlanId,
    state: RootState,
    records: Vec<JournalRecord>,
    journal: Journal,
    actor: String,
    request: RequestId,
    expected: u64,
}

impl Txn {
    fn last(&self) -> Option<&JournalRecord> {
        self.records.last()
    }

    fn derived(&self, suffix: &str) -> Result<RequestId, PlanOpError> {
        RequestId::new(format!("{}/{suffix}", self.request)).map_err(|error| {
            PlanOpError::Doc(DocError::AgentIdWhitespace {
                id: error.to_string(),
            })
        })
    }
}

pub struct PlanEngine {
    store: PlanStore,
    delegate: Arc<dyn Delegate>,
    width: NonZeroUsize,
    output_resolve: Option<Arc<dyn OutputResolve>>,
    op_sink: Option<Arc<dyn OpSink>>,
    liveness: Arc<dyn Liveness>,
    cwd: PathBuf,
}

impl PlanEngine {
    pub fn new(store: PlanStore, delegate: Arc<dyn Delegate>) -> Self {
        let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
        Self {
            store,
            delegate,
            width: dispatch_width(cores),
            output_resolve: None,
            op_sink: None,
            liveness: Arc::new(recovery::Unknown),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }

    pub fn with_width(self, width: NonZeroUsize) -> Self {
        Self { width, ..self }
    }

    pub fn with_output_resolve(self, resolve: Arc<dyn OutputResolve>) -> Self {
        Self {
            output_resolve: Some(resolve),
            ..self
        }
    }

    pub fn with_op_sink(self, sink: Arc<dyn OpSink>) -> Self {
        Self {
            op_sink: Some(sink),
            ..self
        }
    }

    pub fn with_liveness(self, liveness: Arc<dyn Liveness>) -> Self {
        Self { liveness, ..self }
    }

    pub fn with_cwd(self, cwd: PathBuf) -> Self {
        Self { cwd, ..self }
    }

    pub fn store(&self) -> &PlanStore {
        &self.store
    }

    /// The revision `plan.op` compares: `touched` moves on every op, `version` does not.
    pub fn revision(&self, plan: Option<PlanId>) -> Result<TouchCount, PlanOpError> {
        let id = self.resolve(plan)?;
        Ok(self.store.read(&id)?.touched)
    }

    pub fn apply(&self, request: OpRequest) -> Result<Outcome, PlanOpError> {
        let OpRequest {
            plan,
            actor,
            op,
            request_id,
            expected_revision,
        } = request;
        check_actor(&actor, &op)?;
        if let Op::View { full } = op {
            return self.view(plan, full);
        }
        let _lease = self.store.lease()?;
        let request = match request_id {
            Some(id) => id,
            None => {
                RequestId::new(format!("auto-{}", self.store.request_nonce())).map_err(|error| {
                    PlanOpError::Doc(DocError::AgentIdWhitespace {
                        id: error.to_string(),
                    })
                })?
            }
        };
        match op {
            Op::Init { goal, todos } => self.init(goal, todos, &actor, request),
            Op::Import { source } => self.import(&source, &actor, request),
            Op::Repair { resolutions } => {
                self.repair(plan, resolutions, &actor, request, expected_revision)
            }
            Op::Set { goal, rows } if plan.is_none() && self.resolve(None).is_err() => {
                let Some(goal) = goal else {
                    return Err(PlanOpError::NoPlan);
                };
                // Invariant: the caller's id names the Set, so a retry after a crash between
                // the two commits replays or finishes the Set instead of hitting the init.
                let specs = rows.iter().map(|row| row.spec.clone()).collect();
                let opening = RequestId::new(format!("{request}/init")).map_err(|error| {
                    PlanOpError::Doc(DocError::AgentIdWhitespace {
                        id: error.to_string(),
                    })
                })?;
                let opened = self.init(goal.clone(), specs, &actor, opening)?;
                let set = Op::Set {
                    goal: Some(goal),
                    rows,
                };
                self.framed(Some(opened.plan.id), &actor, set, request, None)
            }
            other => self.framed(plan, &actor, other, request, expected_revision),
        }
    }

    /// `adopting` is the importer's load: a checkpoint with no journal is the view it adopts.
    fn begin(
        &self,
        root: &PlanId,
        actor: &Actor,
        request: RequestId,
        expected: Option<TouchCount>,
        adopting: bool,
    ) -> Result<Txn, PlanOpError> {
        let Loaded { state, records } = self.store.load(root, adopting)?;
        Ok(Txn {
            root: root.clone(),
            state,
            records,
            journal: self.store.journal(root),
            actor: actor_word(actor),
            request,
            expected: expected.map_or(0, |touched| touched.0),
        })
    }

    fn commit(&self, txn: &mut Txn, record: JournalRecord) -> Result<JournalRecord, PlanOpError> {
        let sealed = txn.journal.seal(record, txn.last())?;
        txn.journal.append(&sealed)?;
        state::apply(&mut txn.state, &sealed.record)?;
        txn.records.push(sealed.record.clone());
        Ok(sealed.record)
    }

    fn emit(&self, record: &JournalRecord) {
        let Some(sink) = &self.op_sink else {
            return;
        };
        let _telemetry_never_fails_an_op = sink.record(record.record.clone());
    }

    fn init(
        &self,
        goal: GoalText,
        specs: Vec<TodoSpec>,
        actor: &Actor,
        request: RequestId,
    ) -> Result<Outcome, PlanOpError> {
        let op = Op::Init { goal, todos: specs };
        for id in self.store.roots()? {
            if has_record(&self.store.journal_path(&id)) {
                let txn = self.begin(&id, actor, request.clone(), None, false)?;
                if let Some(replayed) = self.replay(&txn, &op)? {
                    return Ok(replayed);
                }
                if matches!(txn.state.plan(&id), Ok(plan) if plan.state == PlanState::Active) {
                    return Err(PlanOpError::PlanExists { id });
                }
                continue;
            }
            match self.store.read(&id) {
                Ok(plan) if plan.state == PlanState::Active => {
                    return Err(PlanOpError::PlanExists { id });
                }
                Ok(_) | Err(StoreError::JournalMissing { .. } | StoreError::NeedsImport { .. }) => {
                }
                Err(error) => return Err(error.into()),
            }
        }
        let Op::Init { goal, .. } = &op else {
            return Err(PlanOpError::NoPlan);
        };
        let id = self.store.allocate(goal)?;
        let mut txn = self.begin(&id, actor, request, None, false)?;
        let mut probe = txn.state.clone();
        state::apply_op(&mut probe, &id, &op, &Decided::default())?;
        let record = self.record_for(&txn, &id, &op, &probe, Decided::default(), None)?;
        for plan in probe.plans.values() {
            PlanStore::render(plan)?;
        }
        let committed = self.commit(&mut txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        self.conclude(&id, &txn.state, &[], Delta::default())
    }

    fn view(&self, plan: Option<PlanId>, _full: bool) -> Result<Outcome, PlanOpError> {
        let id = self.resolve(plan)?;
        let plan = self.store.read(&id)?;
        let ready = ready_labels(&plan);
        Ok(Outcome {
            plan,
            ready,
            dispatched: Vec::new(),
            held: Vec::new(),
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: None,
            notices: Vec::new(),
        })
    }

    fn replay(&self, txn: &Txn, op: &Op) -> Result<Option<Outcome>, PlanOpError> {
        let Some((index, record)) = txn
            .records
            .iter()
            .enumerate()
            .rev()
            .find(|(_, record)| record.request_id == txn.request)
        else {
            return Ok(None);
        };
        // Invariant: the kind is part of the identity; `args()` drops the `op` tag.
        let same = record.record.op == op_name(op.kind())
            && if record.record.op == KIND_IMPORT {
                record.args.get("source") == op.args()?.get("source")
            } else {
                record.args_hash == canonical_digest(&op.args()?)?
            };
        if !same {
            return Err(PlanOpError::RequestIdReused {
                request_id: txn.request.clone(),
            });
        }
        if record.is_refusal() {
            let refusal = record.record.extra.get("refusal");
            let field = |name: &str| {
                refusal
                    .and_then(|value| value.get(name))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            return Err(PlanOpError::RecordedRefusal {
                request_id: txn.request.clone(),
                code: field("code"),
                detail: field("detail"),
            });
        }
        self.store.checkpoint_family(&txn.state)?;
        let then = state::reduce(txn.records.get(..=index).unwrap_or(&[]))?;
        let plan = then.plan(&record.record.plan)?.clone();
        let ready = ready_labels(&plan);
        Ok(Some(Outcome {
            plan,
            ready,
            dispatched: Vec::new(),
            held: Vec::new(),
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: record
                .record
                .extra
                .get("subplan")
                .and_then(Value::as_str)
                .and_then(|id| PlanId::new(id).ok()),
            notices: vec![format!(
                "request {} was already applied as record {}; replayed",
                txn.request, record.seq
            )],
        }))
    }

    fn framed(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let mut txn = self.begin(&root, actor, request, expected, false)?;
        if let Some(replayed) = self.replay(&txn, &op)? {
            return Ok(replayed);
        }
        let current = txn.state.plan(&id)?;
        if let Some(expected) = expected
            && current.touched != expected
        {
            return Err(PlanOpError::StaleRevision {
                expected: expected.0,
                current: current.touched.0,
            });
        }
        let before = admitted(current, self.slots(&txn.state));
        match self.transact(&mut txn, &id, &root, &op) {
            Ok(delta) => self.conclude(&id, &txn.state, &before, delta),
            Err(error) => {
                if error.is_recordable() {
                    self.record_refusal(&mut txn, &id, &op, &error);
                }
                Err(error)
            }
        }
    }

    fn transact(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        root: &PlanId,
        op: &Op,
    ) -> Result<Delta, PlanOpError> {
        let plan = txn.state.plan(id)?;
        check_plan_state(plan, op.kind())?;
        if let Op::Done { label, output } = op {
            let resolve = self.output_resolve.as_deref();
            super::output::check_output(resolve, plan, label, output.as_ref())?;
        }
        let mut decided = Decided::default();
        if let Op::Decompose { label, .. } = op
            && plan.tier == yi_types::plan::doc::PlanTier::Root
        {
            decided.subplan = Some(self.allocate_child(&id.child(label)?)?);
        }
        let leaving = leaving_running(&txn.state, id, op)?;
        // Invariant: every refusable check runs before the first effect, so a refused op has
        // killed and spawned nothing and the idempotent reaps make a retry safe.
        if let Op::Start { label } = op {
            super::table::locate_step(plan, label, OpKind::Start)?;
            admit(plan, label, self.slots(&txn.state)).map_err(PlanOpError::Admission)?;
        } else {
            let mut dry = txn.state.clone();
            let rehearsal = Decided {
                reaped: leaving
                    .iter()
                    .filter_map(|(plan_id, todo)| match &todo.state {
                        TodoState::Running { by } => Some(Reaped {
                            plan: plan_id.clone(),
                            todo: todo.label.clone(),
                            agent: by.clone(),
                            last: None,
                        }),
                        _ => None,
                    })
                    .collect(),
                subplan: decided.subplan.clone(),
            };
            let rehearsed = state::apply_op(&mut dry, id, op, &rehearsal)?;
            for plan in dry.plans.values() {
                PlanStore::render(plan)?;
            }
            // Invariant: the record cap is refused here, before the reaps, not at commit.
            let record = self.record_for(txn, id, op, &dry, rehearsal, rehearsed.from)?;
            txn.journal
                .rehearse(record, txn.last(), REAP_ENVELOPE_BYTES)?;
        }
        let mut delta = Delta::default();
        for (plan_id, todo) in leaving {
            let TodoState::Running { by } = &todo.state else {
                continue;
            };
            let supplied = todo
                .delegation
                .as_ref()
                .map(|delegation| delegation.context.clone())
                .unwrap_or_default();
            let last =
                self.delegate
                    .reap(by, &supplied)
                    .map_err(|reason| PlanOpError::ReapFailed {
                        agent: by.clone(),
                        reason,
                    })?;
            check_terminal(&todo.label, last.as_ref())?;
            delta.reaped.push(agent_url(&TodoAddr {
                plan: plan_id.clone(),
                todo: todo.label.clone(),
            })?);
            decided.reaped.push(Reaped {
                plan: plan_id,
                todo: todo.label.clone(),
                agent: by.clone(),
                last,
            });
        }
        if let Op::Start { label } = op {
            self.ensure_spawned(txn, id, root, label, &mut delta)?;
        }
        let mut probe = txn.state.clone();
        let applied = state::apply_op(&mut probe, id, op, &decided)?;
        let record = self.record_for(txn, id, op, &probe, decided, applied.from)?;
        for plan in probe.plans.values() {
            PlanStore::render(plan)?;
        }
        let committed = self.commit(txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        delta.subplan = applied.subplan;
        Ok(delta)
    }

    fn ensure_spawned(
        &self,
        txn: &mut Txn,
        id: &PlanId,
        root: &PlanId,
        label: &TodoLabel,
        delta: &mut Delta,
    ) -> Result<(), PlanOpError> {
        let plan = txn.state.plan(id)?;
        let Some(todo) = plan.todo(label) else {
            return Ok(());
        };
        let Some(delegation) = todo.delegation.clone() else {
            return Ok(());
        };
        if !matches!(todo.state, TodoState::Pending) {
            return Ok(());
        }
        let addr = TodoAddr {
            plan: id.clone(),
            todo: label.clone(),
        };
        let url = agent_url(&addr)?;
        if let Some((effect, intent)) = txn.state.intent_for(id, label) {
            return match &intent.outcome {
                state::IntentOutcome::Spawned { .. } => {
                    delta.spawned.push(url);
                    Ok(())
                }
                state::IntentOutcome::Pending => Err(PlanOpError::NeedsReconciliation {
                    label: label.clone(),
                    effect: effect.clone(),
                }),
            };
        }
        let spent = txn.state.plan(root)?.spawns();
        if spent >= SPAWN_CAP {
            return Err(PlanOpError::SpawnCeilingExhausted {
                spent,
                cap: SPAWN_CAP,
            });
        }
        let attempt = todo.attempt;
        // Invariant: a journal outlives the pid, so the effect id carries the clock too.
        let effect =
            EffectId::new(format!("e-{}", self.store.request_nonce())).map_err(|error| {
                PlanOpError::Doc(DocError::AgentIdWhitespace {
                    id: error.to_string(),
                })
            })?;
        let todos = u32::try_from(plan.todos.len()).unwrap_or(u32::MAX);
        let mut intent = draft(
            id,
            KIND_SPAWN_INTENT,
            txn.actor.clone(),
            self.store.now_ms(),
            todos,
            serde_json::json!({"label": label, "attempt": attempt, "effect_id": effect}),
            txn.derived("intent")?,
            txn.expected,
            Some(attempt),
        );
        intent.record.todo = Some(label.clone());
        self.commit(txn, intent)?;
        let spawned = self.delegate.spawn(&addr, &delegation);
        let args = match &spawned {
            Ok(agent) => serde_json::json!({"effect_id": effect, "agent": agent}),
            Err(reason) => serde_json::json!({"effect_id": effect, "error": reason}),
        };
        let mut result = draft(
            id,
            KIND_SPAWN_RESULT,
            txn.actor.clone(),
            self.store.now_ms(),
            todos,
            args,
            txn.derived("result")?,
            txn.expected,
            Some(attempt),
        );
        result.record.todo = Some(label.clone());
        self.commit(txn, result)?;
        match spawned {
            Ok(_) => {
                delta.spawned.push(url);
                Ok(())
            }
            Err(reason) => {
                self.store.checkpoint_family(&txn.state)?;
                Err(PlanOpError::SpawnFailed { at: addr, reason })
            }
        }
    }

    fn record_for(
        &self,
        txn: &Txn,
        id: &PlanId,
        op: &Op,
        after: &RootState,
        decided: Decided,
        from: Option<TodoStateName>,
    ) -> Result<JournalRecord, PlanOpError> {
        let plan = after.plan(id)?;
        let label = op.label().cloned();
        let to = label
            .as_ref()
            .and_then(|label| plan.todo(label))
            .map(|todo| TodoStateName::of(&todo.state));
        let attempt = label
            .as_ref()
            .and_then(|label| plan.todo(label))
            .map(|todo| todo.attempt);
        let mut record = draft(
            id,
            op_name(op.kind()),
            txn.actor.clone(),
            self.store.now_ms(),
            u32::try_from(plan.todos.len()).unwrap_or(u32::MAX),
            op.args()?,
            txn.request.clone(),
            txn.expected,
            attempt,
        );
        record.record.todo = label;
        record.record.from = from;
        record.record.to = to;
        record.record.extra = decided.into_extra()?;
        if matches!(op, Op::FuseReset) {
            let prior = txn.state.plan(&txn.root)?.spawns().get();
            record
                .record
                .extra
                .insert("prior".to_owned(), Value::from(prior));
        }
        Ok(record)
    }

    fn record_refusal(&self, txn: &mut Txn, id: &PlanId, op: &Op, error: &PlanOpError) {
        let Ok(plan) = txn.state.plan(id) else {
            return;
        };
        let Ok(args) = op.args() else {
            return;
        };
        let label = op.label().cloned();
        let attempt = label
            .as_ref()
            .and_then(|label| plan.todo(label))
            .map(|todo| todo.attempt);
        let mut record = draft(
            id,
            op_name(op.kind()),
            txn.actor.clone(),
            self.store.now_ms(),
            u32::try_from(plan.todos.len()).unwrap_or(u32::MAX),
            args,
            txn.request.clone(),
            txn.expected,
            attempt,
        );
        record.record.todo = label;
        record.record.extra.insert(
            "refusal".to_owned(),
            serde_json::json!({"code": error.code(), "detail": error.to_string()}),
        );
        let _a_refusal_record_never_changes_the_answer = self
            .commit(txn, record)
            .and_then(|_| Ok(self.store.checkpoint_family(&txn.state)?));
    }

    fn import(
        &self,
        source: &Url,
        actor: &Actor,
        request: RequestId,
    ) -> Result<Outcome, PlanOpError> {
        let read = super::import::read(&self.cwd, source)?;
        super::table::validate_plan(&read.plan)?;
        let id = read.plan.id.clone();
        let root = root_of(&id)?;
        let op = Op::Import {
            source: source.clone(),
        };
        let mut txn = self.begin(&root, actor, request, None, true)?;
        if let Some(replayed) = self.replay(&txn, &op)? {
            return Ok(replayed);
        }
        // Invariant: the journal, not the checkpoint file, says whether the plan exists, so a
        // checkpoint lost behind a live journal cannot be imported over.
        if txn.state.plans.contains_key(&id) {
            return Err(super::import::ImportError::AlreadyImported {
                path: self.store.path(&id),
                id,
            }
            .into());
        }
        let imported = super::import::store(&self.store, read)?;
        let mut record = draft(
            &id,
            KIND_IMPORT,
            txn.actor.clone(),
            self.store.now_ms(),
            u32::try_from(imported.plan.todos.len()).unwrap_or(u32::MAX),
            serde_json::json!({
                "source": imported.source,
                "artifact": imported.artifact.to_string(),
                "format": imported.format,
                "plan": imported.plan.clone().unmarked(),
            }),
            txn.request.clone(),
            0,
            None,
        );
        record.record.extra.insert(
            "media_type".to_owned(),
            Value::String(imported.artifact.media_type.clone()),
        );
        record
            .record
            .extra
            .insert("length".to_owned(), Value::from(imported.artifact.length));
        PlanStore::render(&imported.plan)?;
        let committed = self.commit(&mut txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        let mut outcome = self.conclude(&id, &txn.state, &[], Delta::default())?;
        outcome.notices.push(format!(
            "imported {} as {} (original kept as {})",
            imported.source, id, imported.artifact
        ));
        Ok(outcome)
    }

    fn repair(
        &self,
        plan: Option<PlanId>,
        resolutions: Vec<Resolution>,
        actor: &Actor,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let recovered = recovery::run(&self.store, &root, &*self.liveness)?;
        let mut notices: Vec<String> = Vec::new();
        if let Some(torn) = &recovered.torn {
            notices.push(format!(
                "a torn journal tail was set aside at {}",
                torn.display()
            ));
        }
        notices.push(format!(
            "regenerated {} checkpoint(s) from the journal",
            recovered.regenerated.len()
        ));
        notices.extend(recovered.findings.iter().map(ToString::to_string));
        for resolution in &resolutions {
            let found = recovered
                .findings
                .iter()
                .any(|finding| finding.plan == id && finding.label == resolution.label);
            if !found {
                return Err(PlanOpError::NotReconcilable {
                    label: resolution.label.clone(),
                });
            }
        }
        let mut outcome = if resolutions.is_empty() {
            self.view(Some(id), true)?
        } else {
            self.framed(
                Some(id),
                actor,
                Op::Repair { resolutions },
                request,
                expected,
            )?
        };
        outcome.notices.splice(0..0, notices);
        Ok(outcome)
    }

    fn slots(&self, state: &RootState) -> usize {
        let flight: usize = state
            .plans
            .values()
            .filter(|plan| plan.state == PlanState::Active)
            .map(in_flight)
            .sum();
        self.width.get().saturating_sub(flight)
    }

    fn conclude(
        &self,
        id: &PlanId,
        state: &RootState,
        admissible_before: &[TodoLabel],
        delta: Delta,
    ) -> Result<Outcome, PlanOpError> {
        let mut after = state.plan(id)?.clone();
        after.journal = state
            .mark
            .map(|(seq, digest)| yi_types::plan::doc::JournalMark { seq, digest });
        let now = admitted(&after, self.slots(state));
        let ready = ready_labels(&after);
        let dispatched: Vec<TodoLabel> = now
            .iter()
            .filter(|label| !admissible_before.contains(label))
            .cloned()
            .collect();
        let held: Vec<TodoLabel> = ready
            .iter()
            .filter(|label| !now.contains(label))
            .cloned()
            .collect();
        if !dispatched.is_empty() || !held.is_empty() {
            self.delegate.follow_up(&dispatched, held.len());
        }
        Ok(Outcome {
            plan: after,
            ready,
            dispatched,
            held,
            spawned: delta.spawned,
            reaped: delta.reaped,
            subplan: delta.subplan,
            notices: delta.notices,
        })
    }

    /// Invariant: unnamed resolution finds the Active root, else the newest finished root
    /// still carrying a Failed todo; without one it never matches, so closed work stays shut.
    fn resolve(&self, plan: Option<PlanId>) -> Result<PlanId, PlanOpError> {
        if let Some(id) = plan {
            return Ok(id);
        }
        let mut failed: Option<(PlanId, std::time::SystemTime)> = None;
        for id in self.store.roots()? {
            let plan = match self.store.read(&id) {
                Ok(plan) => plan,
                Err(StoreError::JournalMissing { .. } | StoreError::NeedsImport { .. }) => continue,
                Err(error) => return Err(error.into()),
            };
            if plan.state == PlanState::Active {
                return Ok(id);
            }
            let retryable = plan.state == PlanState::Done
                && plan
                    .todos
                    .iter()
                    .any(|todo| matches!(todo.state, TodoState::Failed { .. }));
            if retryable {
                let written = std::fs::metadata(self.store.path(&id))
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if failed.as_ref().is_none_or(|(_, best)| written > *best) {
                    failed = Some((id, written));
                }
            }
        }
        failed.map(|(id, _)| id).ok_or(PlanOpError::NoPlan)
    }

    /// Invariant: a superseded generation keeps its directory, so a recycled todo label
    /// allocates a suffixed child id rather than overwrite the abandoned sub-plan.
    fn allocate_child(&self, base: &PlanId) -> Result<PlanId, PlanOpError> {
        if !self.store.exists(base) {
            return Ok(base.clone());
        }
        for suffix in 2..=CHILD_SUFFIX_MAX {
            let id = PlanId::new(format!("{base}-{suffix}"))?;
            if !self.store.exists(&id) {
                return Ok(id);
            }
        }
        Err(PlanOpError::Store(StoreError::AllocateExhausted {
            slug: base.clone(),
            tried: CHILD_SUFFIX_MAX,
        }))
    }
}

/// The ready labels `admit` does not refuse at `slots`; the rest are the held list.
fn admitted(plan: &Plan, slots: usize) -> Vec<TodoLabel> {
    ready_labels(plan)
        .into_iter()
        .filter(|label| admit(plan, label, slots).is_ok())
        .collect()
}

fn actor_word(actor: &Actor) -> String {
    match actor {
        Actor::Owner => OWNER_AGENT.to_owned(),
        Actor::Child(agent) => agent.as_str().to_owned(),
        Actor::User(citation) => citation.to_string(),
        Actor::Host => "host".to_owned(),
    }
}

fn agent_url(addr: &TodoAddr) -> Result<Url, PlanOpError> {
    let plan_url = addr.to_url()?;
    let rendered = format!("agent://{}", plan_url.path());
    rendered.parse().map_err(|cause| {
        PlanOpError::Doc(DocError::AddrUrl {
            url: rendered,
            cause,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_op_round_trips_through_its_wire_shape() -> Result<(), Box<dyn std::error::Error>> {
        let op = Op::Fail {
            label: TodoLabel::new("cut")?,
            cause: "no".to_owned(),
        };
        let args = op.args()?;
        assert_eq!(args, serde_json::json!({"label": "cut", "cause": "no"}));
        let mut tagged = args;
        tagged["op"] = Value::String("fail".to_owned());
        assert_eq!(serde_json::from_value::<Op>(tagged)?, op);
        assert_eq!(Op::FuseReset.args()?, serde_json::json!({}));
        Ok(())
    }
}

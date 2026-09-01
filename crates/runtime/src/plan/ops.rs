use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use yi_types::plan::doc::{
    AgentId, BlockedOn, Delegation, DocError, GoalText, Plan, PlanId, PlanIssue, PlanState,
    PlanTier, RetryCount, Spawns, Todo, TodoAddr, TodoLabel, TodoState,
};
use yi_types::url::Url;

use super::store::{FRONTMATTER_CAP_BYTES, PlanBody, PlanFile, PlanStore, StoreError, UserEdit};
use super::table::{
    OpKind, StateName, add_edge, admissible, append_todos, charge_retry, charge_spawn, check_actor,
    check_plan_state, check_terminal, in_flight, locate_step, new_todo, ready_labels,
    reorder_todos, step, validate_plan,
};
use super::yaml;

pub(super) const OWNER_AGENT: &str = "main";

const CHILD_SUFFIX_MAX: u32 = 9_999;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Owner,
    Child(AgentId),
    User(Url),
    Host,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodoSpec {
    pub label: TodoLabel,
    pub after: Vec<TodoLabel>,
    pub delegation: Option<Delegation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Init {
        goal: GoalText,
        todos: Vec<TodoSpec>,
    },
    Append {
        todos: Vec<TodoSpec>,
    },
    Drop {
        label: TodoLabel,
    },
    Block {
        label: TodoLabel,
        on: BlockedOn,
        note: String,
    },
    Unblock {
        label: TodoLabel,
    },
    Reorder {
        labels: Vec<TodoLabel>,
    },
    AddEdge {
        todo: TodoLabel,
        after: TodoLabel,
    },
    Start {
        label: TodoLabel,
    },
    Done {
        label: TodoLabel,
        output: Option<Url>,
    },
    Fail {
        label: TodoLabel,
        cause: String,
    },
    Retry {
        label: TodoLabel,
        delegation: Option<Box<Delegation>>,
    },
    Decompose {
        label: TodoLabel,
        todos: Vec<TodoSpec>,
    },
    Supersede {
        reason: String,
        todos: Vec<TodoSpec>,
    },
    View {
        full: bool,
    },
}

impl Op {
    pub fn kind(&self) -> OpKind {
        match self {
            Self::Init { .. } => OpKind::Init,
            Self::Append { .. } => OpKind::Append,
            Self::Drop { .. } => OpKind::Drop,
            Self::Block { .. } => OpKind::Block,
            Self::Unblock { .. } => OpKind::Unblock,
            Self::Reorder { .. } => OpKind::Reorder,
            Self::AddEdge { .. } => OpKind::AddEdge,
            Self::Start { .. } => OpKind::Start,
            Self::Done { .. } => OpKind::Done,
            Self::Fail { .. } => OpKind::Fail,
            Self::Retry { .. } => OpKind::Retry,
            Self::Decompose { .. } => OpKind::Decompose,
            Self::Supersede { .. } => OpKind::Supersede,
            Self::View { .. } => OpKind::View,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpRequest {
    pub plan: Option<PlanId>,
    pub actor: Actor,
    pub op: Op,
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
    #[error("{op:?} is illegal for todo {label:?} in state {from}")]
    IllegalStep {
        label: TodoLabel,
        from: StateName,
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
    #[error("start refused for todo {label:?}: after edge {after:?} is not cleared")]
    UnmetEdge { label: TodoLabel, after: TodoLabel },
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
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Doc(#[from] DocError),
}

pub trait Delegate: Send + Sync {
    fn spawn(&self, at: &TodoAddr, delegation: &Delegation) -> Result<AgentId, String>;
    fn reap(&self, agent: &AgentId) -> Result<Option<Url>, String>;
    fn follow_up(&self, dispatched: &[TodoLabel], held: usize);
}

/// One existence probe over the fetch seam, shaped like
/// [`crate::fetch::CheckpointShow`]: Ok means the referent resolves today.
pub trait OutputResolve: Send + Sync {
    fn resolve(&self, url: &Url) -> Result<(), String>;
}

impl OutputResolve for crate::fetch::Resolver {
    fn resolve(&self, url: &Url) -> Result<(), String> {
        use crate::fetch::FetchError;
        match self.fetch(url) {
            Ok(_) => Ok(()),
            // Invariant: a missing reader is not a missing referent — a scheme
            // this resolver has no backend for cannot adjudicate existence.
            Err(FetchError::Unsupported { .. }) => Ok(()),
            Err(
                error @ (FetchError::Denied { .. }
                | FetchError::External { .. }
                | FetchError::OutsideWorkspace { .. }
                | FetchError::BadAddress { .. }
                | FetchError::NotFound { .. }
                | FetchError::Stale { .. }
                | FetchError::Backend { .. }),
            ) => Err(error.to_string()),
        }
    }
}

pub fn dispatch_width(cores: NonZeroUsize) -> NonZeroUsize {
    NonZeroUsize::new(cores.get().saturating_sub(1).clamp(1, 8)).unwrap_or(NonZeroUsize::MIN)
}

#[derive(Debug, Default)]
struct Delta {
    spawned: Vec<(Url, AgentId)>,
    reaped: Vec<Url>,
    subplan: Option<PlanId>,
    extra: Vec<PlanFile>,
}

pub struct PlanEngine {
    store: PlanStore,
    delegate: Arc<dyn Delegate>,
    width: NonZeroUsize,
    known: Mutex<HashMap<PlanId, PlanFile>>,
    output_resolve: Option<Arc<dyn OutputResolve>>,
}

impl PlanEngine {
    pub fn new(store: PlanStore, delegate: Arc<dyn Delegate>) -> Self {
        let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
        Self {
            store,
            delegate,
            width: dispatch_width(cores),
            known: Mutex::new(HashMap::new()),
            output_resolve: None,
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

    pub fn apply(&self, request: OpRequest) -> Result<Outcome, PlanOpError> {
        let OpRequest { plan, actor, op } = request;
        let kind = op.kind();
        check_actor(&actor, kind)?;
        let _lease = self.store.lease()?;
        match op {
            Op::Init { goal, todos } => self.init(goal, todos),
            Op::Append { todos } => self.framed(plan, kind, |file, _| {
                append_todos(&mut file.plan, todos)?;
                Ok(Delta::default())
            }),
            Op::Drop { label } => self.framed(plan, kind, |file, _| {
                self.do_mark(file, &label, OpKind::Drop, TodoState::Abandoned)
            }),
            Op::Block { label, on, note } => self.framed(plan, kind, |file, _| {
                self.do_mark(file, &label, OpKind::Block, TodoState::Blocked { on, note })
            }),
            Op::Unblock { label } => self.framed(plan, kind, |file, _| {
                self.do_mark(file, &label, OpKind::Unblock, TodoState::Pending)
            }),
            Op::Reorder { labels } => self.framed(plan, kind, |file, _| {
                reorder_todos(&mut file.plan, labels)?;
                Ok(Delta::default())
            }),
            Op::AddEdge { todo, after } => self.framed(plan, kind, |file, _| {
                add_edge(&mut file.plan, todo, after)?;
                Ok(Delta::default())
            }),
            Op::Start { label } => {
                self.framed(plan, kind, |file, root| self.do_start(file, root, label))
            }
            Op::Done { label, output } => {
                self.framed(plan, kind, |file, _| self.do_done(file, label, output))
            }
            Op::Fail { label, cause } => {
                self.framed(plan, kind, |file, _| self.do_fail(file, label, cause))
            }
            Op::Retry { label, delegation } => {
                self.framed(plan, kind, |file, _| self.do_retry(file, label, delegation))
            }
            Op::Decompose { label, todos } => {
                self.framed(plan, kind, |file, _| self.do_decompose(file, label, todos))
            }
            Op::Supersede { reason, todos } => {
                self.framed(plan, kind, |file, _| self.do_supersede(file, reason, todos))
            }
            Op::View { full } => self.view(plan, full),
        }
    }

    fn init(&self, goal: GoalText, specs: Vec<TodoSpec>) -> Result<Outcome, PlanOpError> {
        for id in self.store.roots()? {
            if self.store.read(&id)?.plan.state == PlanState::Active {
                return Err(PlanOpError::PlanExists { id });
            }
        }
        let id = self.store.allocate(&goal)?;
        let plan = Plan::opening(
            id.clone(),
            goal,
            PlanTier::Root,
            specs.into_iter().map(new_todo).collect(),
        );
        validate_plan(&plan)?;
        let file = PlanFile {
            plan,
            body: PlanBody::default(),
        };
        self.write_all(&file, &[])?;
        self.conclude(&id, &id, &[], Delta::default())
    }

    fn view(&self, plan: Option<PlanId>, _full: bool) -> Result<Outcome, PlanOpError> {
        let id = self.resolve(plan)?;
        let file = self.store.read(&id)?;
        let ready = ready_labels(&file.plan);
        Ok(Outcome {
            plan: file.plan,
            ready,
            dispatched: Vec::new(),
            held: Vec::new(),
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: None,
        })
    }

    fn framed<F>(&self, plan: Option<PlanId>, op: OpKind, mutate: F) -> Result<Outcome, PlanOpError>
    where
        F: FnOnce(&mut PlanFile, &PlanId) -> Result<Delta, PlanOpError>,
    {
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let mut file = self.store.read(&id)?;
        check_plan_state(&file.plan, op)?;
        let mut delta = Delta::default();
        self.fold_user_edits(&mut file, &mut delta)?;
        let flight = self.family_flight(&root)?;
        let before = admissible(&file.plan, self.width.get().saturating_sub(flight));
        let mutated = mutate(&mut file, &root)?;
        delta.spawned.extend(mutated.spawned);
        delta.reaped.extend(mutated.reaped);
        delta.subplan = mutated.subplan;
        delta.extra.extend(mutated.extra);
        file.plan.touched = file.plan.touched.bump();
        if matches!(file.plan.state, PlanState::Active | PlanState::Done) {
            file.plan.state = if file.plan.finished() {
                PlanState::Done
            } else {
                PlanState::Active
            };
        }
        if let Err(refused) = self.write_all(&file, &delta.extra) {
            for (_, agent) in &delta.spawned {
                let _ = self.delegate.reap(agent);
            }
            return Err(refused);
        }
        self.conclude(&id, &root, &before, delta)
    }

    /// Invariant: the plan file's second sanctioned writer is the user's
    /// editor, so a divergence folds in as user-attributed ops — each moves
    /// `touched`, and an exit from Running reaps like an engine op's would.
    fn fold_user_edits(&self, file: &mut PlanFile, delta: &mut Delta) -> Result<(), PlanOpError> {
        let snapshot = match self.known.lock() {
            Ok(known) => known.get(&file.plan.id).cloned(),
            Err(_) => None,
        };
        let Some(snapshot) = snapshot else {
            return Ok(());
        };
        for edit in self.store.user_edits(&snapshot)? {
            let left_running = match &edit {
                UserEdit::TodoStateChanged { label, from, to } => {
                    (matches!(from, TodoState::Running { .. })
                        && !matches!(to, TodoState::Running { .. }))
                    .then_some(label)
                }
                UserEdit::TodoDropped { label } => Some(label),
                UserEdit::GoalReworded { .. }
                | UserEdit::PlanStateChanged { .. }
                | UserEdit::SpawnsChanged { .. }
                | UserEdit::TodoAdded { .. }
                | UserEdit::TodoEdgesChanged { .. }
                | UserEdit::TodoEdited { .. }
                | UserEdit::TodoReordered { .. }
                | UserEdit::PreambleEdited
                | UserEdit::SectionEdited { .. } => None,
            };
            if let Some(was) = left_running.and_then(|label| snapshot.plan.todo(label)) {
                self.reap_leaving_running(&file.plan.id, was, delta)?;
            }
            file.plan.touched = file.plan.touched.bump();
        }
        Ok(())
    }

    fn conclude(
        &self,
        id: &PlanId,
        root: &PlanId,
        admissible_before: &[TodoLabel],
        delta: Delta,
    ) -> Result<Outcome, PlanOpError> {
        let after = self.store.read(id)?;
        let flight = self.family_flight(root)?;
        let now = admissible(&after.plan, self.width.get().saturating_sub(flight));
        let ready = ready_labels(&after.plan);
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
            plan: after.plan,
            ready,
            dispatched,
            held,
            spawned: delta.spawned.into_iter().map(|(url, _)| url).collect(),
            reaped: delta.reaped,
            subplan: delta.subplan,
        })
    }

    /// Invariant: unnamed resolution finds the Active root, else the newest
    /// finished root still carrying a Failed todo — the §9 ladder's target. A
    /// root with no Failed todo never matches, so finished work stays closed.
    fn resolve(&self, plan: Option<PlanId>) -> Result<PlanId, PlanOpError> {
        if let Some(id) = plan {
            return Ok(id);
        }
        let mut failed: Option<(PlanId, std::time::SystemTime)> = None;
        for id in self.store.roots()? {
            let plan = self.store.read(&id)?.plan;
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

    fn write_all(&self, target: &PlanFile, extra: &[PlanFile]) -> Result<(), PlanOpError> {
        precheck(target)?;
        for file in extra {
            precheck(file)?;
        }
        for file in extra {
            self.store.write(file)?;
        }
        self.store.write(target)?;
        if let Ok(mut known) = self.known.lock() {
            known.insert(target.plan.id.clone(), target.clone());
            for file in extra {
                known.insert(file.plan.id.clone(), file.clone());
            }
        }
        Ok(())
    }

    fn read_family(&self, root: &PlanId) -> Result<Vec<PlanFile>, PlanOpError> {
        let mut family = Vec::new();
        for id in self.store.list()? {
            let kin = &id == root
                || id
                    .as_str()
                    .strip_prefix(root.as_str())
                    .is_some_and(|rest| rest.starts_with('.'));
            if kin {
                family.push(self.store.read(&id)?);
            }
        }
        Ok(family)
    }

    fn family_flight(&self, root: &PlanId) -> Result<usize, PlanOpError> {
        Ok(self
            .read_family(root)?
            .iter()
            .filter(|file| file.plan.state == PlanState::Active)
            .map(|file| in_flight(&file.plan))
            .sum())
    }

    /// Invariant: every todo state write goes through here, so any transition
    /// out of Running for a delegated todo reaps the child and hands its
    /// host-minted product to the new state — no exit can forget the reap.
    fn step_todo<F>(
        &self,
        file: &mut PlanFile,
        label: &TodoLabel,
        op: OpKind,
        delta: &mut Delta,
        make: F,
    ) -> Result<(), PlanOpError>
    where
        F: FnOnce(Option<Url>) -> TodoState,
    {
        let index = locate_step(&file.plan, label, op)?;
        let plan_id = file.plan.id.clone();
        let missing = || PlanOpError::UnknownLabel {
            plan: plan_id.clone(),
            label: label.clone(),
        };
        let Some(todo) = file.plan.todos.get(index) else {
            return Err(missing());
        };
        let last = match step(&todo.state, op) {
            Some(StateName::Running) | None => None,
            Some(
                StateName::Pending
                | StateName::Blocked
                | StateName::Done
                | StateName::Failed
                | StateName::Abandoned,
            ) => self.reap_leaving_running(&plan_id, todo, delta)?,
        };
        let Some(todo) = file.plan.todos.get_mut(index) else {
            return Err(missing());
        };
        todo.state = make(last);
        Ok(())
    }

    fn reap_leaving_running(
        &self,
        plan: &PlanId,
        todo: &Todo,
        delta: &mut Delta,
    ) -> Result<Option<Url>, PlanOpError> {
        if todo.delegation.is_none() {
            return Ok(None);
        }
        let TodoState::Running { by } = &todo.state else {
            return Ok(None);
        };
        let agent = by.clone();
        let last = self
            .delegate
            .reap(&agent)
            .map_err(|reason| PlanOpError::ReapFailed { agent, reason })?;
        check_terminal(&todo.label, last.as_ref())?;
        delta.reaped.push(agent_url(&TodoAddr {
            plan: plan.clone(),
            todo: todo.label.clone(),
        })?);
        Ok(last)
    }

    fn do_mark(
        &self,
        file: &mut PlanFile,
        label: &TodoLabel,
        op: OpKind,
        state: TodoState,
    ) -> Result<Delta, PlanOpError> {
        let mut delta = Delta::default();
        self.step_todo(file, label, op, &mut delta, move |_| state)?;
        Ok(delta)
    }

    fn do_start(
        &self,
        file: &mut PlanFile,
        root: &PlanId,
        label: TodoLabel,
    ) -> Result<Delta, PlanOpError> {
        let index = locate_step(&file.plan, &label, OpKind::Start)?;
        let plan_id = file.plan.id.clone();
        let Some(todo) = file.plan.todos.get(index) else {
            return Err(PlanOpError::UnknownLabel {
                plan: plan_id,
                label,
            });
        };
        let mut delta = Delta::default();
        let by = match todo.delegation.clone() {
            None => AgentId::new(OWNER_AGENT)?,
            Some(delegation) => {
                let addr = TodoAddr {
                    plan: file.plan.id.clone(),
                    todo: label.clone(),
                };
                let url = agent_url(&addr)?;
                if &file.plan.id == root {
                    charge_spawn(&mut file.plan)?;
                } else {
                    let mut root_file = self.store.read(root)?;
                    charge_spawn(&mut root_file.plan)?;
                    delta.extra.push(root_file);
                }
                let agent = self
                    .delegate
                    .spawn(&addr, &delegation)
                    .map_err(|reason| PlanOpError::SpawnFailed { at: addr, reason })?;
                delta.spawned.push((url, agent.clone()));
                agent
            }
        };
        self.step_todo(file, &label, OpKind::Start, &mut delta, move |_| {
            TodoState::Running { by }
        })?;
        Ok(delta)
    }

    fn do_done(
        &self,
        file: &mut PlanFile,
        label: TodoLabel,
        output: Option<Url>,
    ) -> Result<Delta, PlanOpError> {
        let index = locate_step(&file.plan, &label, OpKind::Done)?;
        if let Some(todo) = file.plan.todos.get(index)
            && let Some(delegation) = &todo.delegation
            && let Some(declared) = &delegation.output
        {
            match &output {
                None => {
                    return Err(PlanOpError::MissingDeclaredOutput {
                        label,
                        schema: declared.schema.clone(),
                    });
                }
                Some(url) => {
                    if let Some(resolve) = &self.output_resolve {
                        resolve
                            .resolve(url)
                            .map_err(|cause| PlanOpError::UnresolvedOutput {
                                label: label.clone(),
                                url: url.clone(),
                                cause,
                            })?;
                    }
                }
            }
        }
        check_terminal(&label, output.as_ref())?;
        let mut delta = Delta::default();
        self.step_todo(file, &label, OpKind::Done, &mut delta, move |_| {
            TodoState::Done { output }
        })?;
        Ok(delta)
    }

    fn do_fail(
        &self,
        file: &mut PlanFile,
        label: TodoLabel,
        cause: String,
    ) -> Result<Delta, PlanOpError> {
        let mut delta = Delta::default();
        self.step_todo(file, &label, OpKind::Fail, &mut delta, move |last| {
            TodoState::Failed { cause, last }
        })?;
        Ok(delta)
    }

    fn do_retry(
        &self,
        file: &mut PlanFile,
        label: TodoLabel,
        delegation: Option<Box<Delegation>>,
    ) -> Result<Delta, PlanOpError> {
        let index = locate_step(&file.plan, &label, OpKind::Retry)?;
        let plan_id = file.plan.id.clone();
        let missing = || PlanOpError::UnknownLabel {
            plan: plan_id.clone(),
            label: label.clone(),
        };
        let spent = match file.plan.todos.get(index) {
            Some(todo) => todo.retries,
            None => return Err(missing()),
        };
        let bumped = charge_retry(&label, spent)?;
        let mut delta = Delta::default();
        self.step_todo(file, &label, OpKind::Retry, &mut delta, |_| {
            TodoState::Pending
        })?;
        let Some(todo) = file.plan.todos.get_mut(index) else {
            return Err(missing());
        };
        todo.retries = bumped;
        if let Some(replacement) = delegation {
            todo.delegation = Some(*replacement);
        }
        Ok(delta)
    }

    fn do_decompose(
        &self,
        file: &mut PlanFile,
        label: TodoLabel,
        specs: Vec<TodoSpec>,
    ) -> Result<Delta, PlanOpError> {
        match &file.plan.tier {
            PlanTier::Root => {}
            PlanTier::Sub { .. } | PlanTier::Other { .. } => {
                return Err(PlanOpError::DepthExhausted {
                    plan: file.plan.id.clone(),
                });
            }
        }
        let index = locate_step(&file.plan, &label, OpKind::Decompose)?;
        let plan_id = file.plan.id.clone();
        let missing = || PlanOpError::UnknownLabel {
            plan: plan_id.clone(),
            label: label.clone(),
        };
        let Some(todo) = file.plan.todos.get(index) else {
            return Err(missing());
        };
        if let Some(existing) = &todo.subplan {
            return Err(PlanOpError::PlanExists {
                id: existing.clone(),
            });
        }
        let sub_id = self.allocate_child(&file.plan.id.child(&label)?)?;
        let sub = Plan::opening(
            sub_id.clone(),
            GoalText::new(label.as_str())?,
            PlanTier::Sub {
                parent: TodoAddr {
                    plan: file.plan.id.clone(),
                    todo: label.clone(),
                },
            },
            specs.into_iter().map(new_todo).collect(),
        );
        validate_plan(&sub)?;
        let Some(todo) = file.plan.todos.get_mut(index) else {
            return Err(missing());
        };
        todo.subplan = Some(sub_id.clone());
        Ok(Delta {
            spawned: Vec::new(),
            reaped: Vec::new(),
            subplan: Some(sub_id),
            extra: vec![PlanFile {
                plan: sub,
                body: PlanBody::default(),
            }],
        })
    }

    /// Invariant: a superseded generation keeps its ledger file forever, so a
    /// recycled todo label allocates a suffixed child id instead of overwriting
    /// the abandoned sub-plan — the same collision rule as the root allocator.
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

    fn do_supersede(
        &self,
        file: &mut PlanFile,
        reason: String,
        specs: Vec<TodoSpec>,
    ) -> Result<Delta, PlanOpError> {
        let mut subs = Vec::new();
        for id in self.store.list()? {
            let kin = id
                .as_str()
                .strip_prefix(file.plan.id.as_str())
                .is_some_and(|rest| rest.starts_with('.'));
            if kin {
                subs.push(self.store.read(&id)?);
            }
        }
        let mut delta = Delta::default();
        self.reap_superseded(&mut file.plan, &reason, &mut delta)?;
        for sub in &mut subs {
            self.reap_superseded(&mut sub.plan, &reason, &mut delta)?;
            sub.plan.state = PlanState::Abandoned;
        }
        delta.extra.append(&mut subs);
        file.plan.version = file.plan.version.bump();
        file.plan.todos = specs.into_iter().map(new_todo).collect();
        validate_plan(&file.plan)?;
        Ok(delta)
    }

    fn reap_superseded(
        &self,
        plan: &mut Plan,
        reason: &str,
        delta: &mut Delta,
    ) -> Result<(), PlanOpError> {
        let id = plan.id.clone();
        for todo in &mut plan.todos {
            if todo.delegation.is_none() || !matches!(todo.state, TodoState::Running { .. }) {
                continue;
            }
            let last = self.reap_leaving_running(&id, todo, delta)?;
            todo.state = TodoState::Failed {
                cause: format!("superseded: {reason}"),
                last,
            };
        }
        Ok(())
    }
}

fn root_of(id: &PlanId) -> Result<PlanId, PlanOpError> {
    match id.as_str().split_once('.') {
        Some((root, _)) => Ok(PlanId::new(root)?),
        None => Ok(id.clone()),
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

fn precheck(file: &PlanFile) -> Result<(), PlanOpError> {
    let id = &file.plan.id;
    let value = serde_json::to_value(&file.plan).map_err(|source| StoreError::Serialize {
        id: id.clone(),
        source,
    })?;
    let front = yaml::to_yaml(&value).map_err(|source| StoreError::Emit {
        id: id.clone(),
        source,
    })?;
    if front.len() > FRONTMATTER_CAP_BYTES {
        return Err(PlanOpError::Store(StoreError::FrontmatterOverCap {
            id: id.clone(),
            bytes: front.len(),
            cap: FRONTMATTER_CAP_BYTES,
        }));
    }
    Ok(())
}

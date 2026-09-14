//! The pure half of the engine: the journal reduces to state with no delegate and no filesystem.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use yi_types::plan::canonical::Digest;
use yi_types::plan::doc::{
    AgentId, GoalText, Plan, PlanId, PlanState, PlanTier, SPAWN_CAP, Todo, TodoAddr, TodoLabel,
    TodoState, TodoStateName,
};
use yi_types::plan::ledger::{AttemptId, EffectId, JournalRecord, Seq};
use yi_types::url::Url;

use yi_types::plan::op::{Op, Reaped, Reconciliation, Resolution, Resolve, SetRow, TodoSpec};

use super::ops::{OWNER_AGENT, PlanOpError};
use super::table::{
    OpKind, add_edge, append_todos, charge_retry, check_plan_state, check_terminal, locate_step,
    new_todo, reorder_todos, step, validate_plan,
};

pub const KIND_IMPORT: &str = "import";
pub const KIND_SPAWN_INTENT: &str = "spawn_intent";
pub const KIND_SPAWN_RESULT: &str = "spawn_result";
pub const KIND_RECONCILED: &str = "reconciled";
pub const KIND_ACCEPTED_BY_USER: &str = "accepted_by_user";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub plan: PlanId,
    pub label: TodoLabel,
    pub attempt: AttemptId,
    pub outcome: IntentOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentOutcome {
    Pending,
    Spawned { agent: AgentId },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RootState {
    pub plans: BTreeMap<PlanId, Plan>,
    pub intents: BTreeMap<EffectId, Intent>,
    /// Every effect id the journal has named: `intents` loses its entry when the `start` that
    /// consumes it lands, so it cannot tell a replayed record from a new one.
    pub effects: BTreeSet<EffectId>,
    pub mark: Option<(Seq, Digest)>,
}

impl RootState {
    pub fn plan(&self, id: &PlanId) -> Result<&Plan, PlanOpError> {
        self.plans.get(id).ok_or(PlanOpError::NoPlan)
    }

    fn plan_mut(&mut self, id: &PlanId) -> Result<&mut Plan, PlanOpError> {
        self.plans.get_mut(id).ok_or(PlanOpError::NoPlan)
    }

    pub fn intent_for(&self, plan: &PlanId, label: &TodoLabel) -> Option<(&EffectId, &Intent)> {
        self.intents
            .iter()
            .find(|(_, intent)| &intent.plan == plan && &intent.label == label)
    }

    pub fn kin(&self, id: &PlanId) -> Vec<PlanId> {
        self.plans
            .keys()
            .filter(|other| {
                other
                    .as_str()
                    .strip_prefix(id.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
            })
            .cloned()
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReduceError {
    #[error("record {seq} ({op}): args do not decode: {detail}")]
    Args {
        seq: Seq,
        op: String,
        detail: String,
    },
    #[error("record {seq} ({op}) does not apply: {source}")]
    Op {
        seq: Seq,
        op: String,
        source: Box<PlanOpError>,
    },
    #[error("record {seq}: unknown record kind {op:?}")]
    UnknownKind { seq: Seq, op: String },
    #[error("record {seq} ({op}) names no todo")]
    NoTodo { seq: Seq, op: String },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decided {
    pub reaped: Vec<Reaped>,
    pub subplan: Option<PlanId>,
}

impl Decided {
    pub fn into_extra(self) -> Result<Map<String, Value>, serde_json::Error> {
        let mut extra = Map::new();
        if !self.reaped.is_empty() {
            extra.insert("reaped".to_owned(), serde_json::to_value(&self.reaped)?);
        }
        if let Some(subplan) = self.subplan {
            extra.insert(
                "subplan".to_owned(),
                Value::String(subplan.as_str().to_owned()),
            );
        }
        Ok(extra)
    }

    fn from_extra(extra: &Map<String, Value>) -> Result<Self, String> {
        let reaped = match extra.get("reaped") {
            None => Vec::new(),
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
        };
        let subplan = match extra.get("subplan") {
            None => None,
            Some(Value::String(id)) => Some(PlanId::new(id).map_err(|e| e.to_string())?),
            Some(other) => return Err(format!("subplan is not a string: {other}")),
        };
        Ok(Self { reaped, subplan })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Applied {
    pub from: Option<TodoStateName>,
    pub subplan: Option<PlanId>,
}

pub fn root_of(id: &PlanId) -> Result<PlanId, PlanOpError> {
    match id.as_str().split_once('.') {
        Some((root, _)) => Ok(PlanId::new(root)?),
        None => Ok(id.clone()),
    }
}

pub fn op_of(record: &JournalRecord) -> Result<Op, ReduceError> {
    let mut args = match &record.args {
        Value::Object(map) => map.clone(),
        other => {
            return Err(ReduceError::Args {
                seq: record.seq,
                op: record.record.op.clone(),
                detail: format!("args is not an object: {other}"),
            });
        }
    };
    args.insert("op".to_owned(), Value::String(record.record.op.clone()));
    serde_json::from_value(Value::Object(args)).map_err(|error| ReduceError::Args {
        seq: record.seq,
        op: record.record.op.clone(),
        detail: error.to_string(),
    })
}

/// The whole journal, reduced. Takes no delegate: nothing here can dispatch.
pub fn reduce(records: &[JournalRecord]) -> Result<RootState, ReduceError> {
    let mut state = RootState::default();
    for record in records {
        apply(&mut state, record)?;
    }
    Ok(state)
}

pub fn apply(state: &mut RootState, record: &JournalRecord) -> Result<(), ReduceError> {
    let seq = record.seq;
    let kind = record.record.op.as_str();
    let failed = |source: PlanOpError| ReduceError::Op {
        seq,
        op: kind.to_owned(),
        source: Box::new(source),
    };
    let bad_args = |detail: String| ReduceError::Args {
        seq,
        op: kind.to_owned(),
        detail,
    };
    let id = record.record.plan.clone();
    if record.is_refusal() {
        if let (Some(label), Some(plan)) = (&record.record.todo, state.plans.get_mut(&id))
            && let Some(todo) = plan.todos.iter_mut().find(|todo| &todo.label == label)
        {
            todo.refusals = todo.refusals.saturating_add(1);
        }
        state.mark = Some((record.seq, record.digest));
        return Ok(());
    }
    match kind {
        KIND_IMPORT => {
            let plan: Plan = record
                .args
                .get("plan")
                .cloned()
                .ok_or_else(|| bad_args("import carries no plan".to_owned()))
                .and_then(|value| {
                    serde_json::from_value(value).map_err(|e| bad_args(e.to_string()))
                })?;
            validate_plan(&plan).map_err(failed)?;
            state.plans.insert(plan.id.clone(), plan.unmarked());
        }
        KIND_SPAWN_INTENT => {
            let label = record.record.todo.clone().ok_or(ReduceError::NoTodo {
                seq,
                op: kind.to_owned(),
            })?;
            let effect = effect_of(record).map_err(bad_args)?;
            // Invariant: an effect id names one intent for the life of the journal; a second
            // charges the fuse twice and leaves a pending intent on a running todo.
            if !state.effects.insert(effect.clone()) {
                return Err(bad_args(format!("spawn intent {effect} already exists")));
            }
            let attempt = record.attempt.unwrap_or(AttemptId::FIRST);
            let root = root_of(&id).map_err(failed)?;
            let plan = state.plan_mut(&root).map_err(failed)?;
            if plan.spawns() >= SPAWN_CAP {
                return Err(failed(PlanOpError::SpawnCeilingExhausted {
                    spent: plan.spawns(),
                    cap: SPAWN_CAP,
                }));
            }
            plan.charge_spawn();
            state.intents.insert(
                effect,
                Intent {
                    plan: id,
                    label,
                    attempt,
                    outcome: IntentOutcome::Pending,
                },
            );
        }
        KIND_SPAWN_RESULT => {
            let effect = effect_of(record).map_err(bad_args)?;
            if !state.intents.contains_key(&effect) {
                return Err(bad_args(format!("no intent {effect} to resolve")));
            }
            match record.args.get("agent") {
                Some(Value::String(agent)) => {
                    let agent = AgentId::new(agent).map_err(|e| bad_args(e.to_string()))?;
                    if let Some(intent) = state.intents.get_mut(&effect) {
                        intent.outcome = IntentOutcome::Spawned { agent };
                    }
                }
                _ => {
                    state.intents.remove(&effect);
                }
            }
        }
        KIND_RECONCILED | KIND_ACCEPTED_BY_USER => {}
        _ => {
            let op = op_of(record)?;
            let decided = Decided::from_extra(&record.record.extra).map_err(bad_args)?;
            apply_op(state, &id, &op, &decided).map_err(failed)?;
        }
    }
    state.mark = Some((record.seq, record.digest));
    Ok(())
}

fn effect_of(record: &JournalRecord) -> Result<EffectId, String> {
    match record.args.get("effect_id") {
        Some(Value::String(id)) => EffectId::new(id).map_err(|e| e.to_string()),
        _ => Err("no effect_id".to_owned()),
    }
}

pub fn leaving_running(
    state: &RootState,
    id: &PlanId,
    op: &Op,
) -> Result<Vec<(PlanId, Todo)>, PlanOpError> {
    let plan = state.plan(id)?;
    let live =
        |todo: &Todo| todo.delegation.is_some() && matches!(todo.state, TodoState::Running { .. });
    let named = |label: &TodoLabel| -> Vec<(PlanId, Todo)> {
        plan.todo(label)
            .filter(|todo| live(todo))
            .map(|todo| vec![(id.clone(), todo.clone())])
            .unwrap_or_default()
    };
    Ok(match op {
        Op::Done { label, .. } | Op::Fail { label, .. } | Op::Block { label, .. } => named(label),
        Op::Set { rows, .. } => plan
            .todos
            .iter()
            .filter(|todo| live(todo) && !rows.iter().any(|row| row.spec.label == todo.label))
            .map(|todo| (id.clone(), todo.clone()))
            .collect(),
        Op::Supersede { .. } => {
            let mut out: Vec<(PlanId, Todo)> = plan
                .todos
                .iter()
                .filter(|todo| live(todo))
                .map(|todo| (id.clone(), todo.clone()))
                .collect();
            for sub in state.kin(id) {
                if let Some(plan) = state.plans.get(&sub) {
                    out.extend(
                        plan.todos
                            .iter()
                            .filter(|todo| live(todo))
                            .map(|todo| (sub.clone(), todo.clone())),
                    );
                }
            }
            out
        }
        Op::Init { .. }
        | Op::Append { .. }
        | Op::Drop { .. }
        | Op::Unblock { .. }
        | Op::Reorder { .. }
        | Op::AddEdge { .. }
        | Op::Start { .. }
        | Op::Retry { .. }
        | Op::Decompose { .. }
        | Op::View { .. }
        | Op::FuseReset
        | Op::Repair { .. }
        | Op::Import { .. }
        | Op::Reconcile { .. } => Vec::new(),
    })
}

fn reaped_last(decided: &Decided, plan: &PlanId, label: &TodoLabel) -> Option<Url> {
    decided
        .reaped
        .iter()
        .find(|reaped| &reaped.plan == plan && &reaped.todo == label)
        .and_then(|reaped| reaped.last.clone())
}

fn step_todo<F>(
    plan: &mut Plan,
    label: &TodoLabel,
    op: OpKind,
    decided: &Decided,
    make: F,
) -> Result<(), PlanOpError>
where
    F: FnOnce(Option<Url>) -> TodoState,
{
    let index = locate_step(plan, label, op)?;
    let id = plan.id.clone();
    let todo = plan
        .todos
        .get_mut(index)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    let last = match step(&todo.state, op) {
        Some(TodoStateName::Running) | None => None,
        Some(_) => reaped_last(decided, &id, label),
    };
    todo.state = make(last);
    Ok(())
}

pub fn apply_op(
    state: &mut RootState,
    id: &PlanId,
    op: &Op,
    decided: &Decided,
) -> Result<Applied, PlanOpError> {
    for reaped in &decided.reaped {
        check_terminal(&reaped.todo, reaped.last.as_ref())?;
    }
    if let Op::Init { goal, todos } = op {
        let plan = Plan::opening(
            id.clone(),
            goal.clone(),
            PlanTier::Root,
            todos.iter().cloned().map(new_todo).collect(),
        );
        validate_plan(&plan)?;
        state.plans.insert(id.clone(), plan);
        return Ok(Applied::default());
    }
    let kind = op.kind();
    let plan = state.plan(id)?;
    check_plan_state(plan, kind)?;
    let from = op
        .label()
        .and_then(|label| plan.todo(label))
        .map(|todo| TodoStateName::of(&todo.state));
    let mut applied = Applied {
        from,
        subplan: None,
    };
    match op {
        Op::Init { .. } | Op::View { .. } | Op::Import { .. } => {
            return Err(PlanOpError::NotJournaled { op: kind });
        }
        Op::Append { todos } => append_todos(state.plan_mut(id)?, todos.clone())?,
        Op::Drop { label } => {
            step_todo(state.plan_mut(id)?, label, OpKind::Drop, decided, |_| {
                TodoState::Abandoned
            })?;
        }
        Op::Block { label, on, note } => {
            let (on, note) = (on.clone(), note.clone());
            step_todo(
                state.plan_mut(id)?,
                label,
                OpKind::Block,
                decided,
                move |_| TodoState::Blocked { on, note },
            )?;
        }
        Op::Unblock { label } => {
            step_todo(state.plan_mut(id)?, label, OpKind::Unblock, decided, |_| {
                TodoState::Pending
            })?;
        }
        Op::Reorder { labels } => reorder_todos(state.plan_mut(id)?, labels.clone())?,
        Op::AddEdge { todo, after } => {
            add_edge(state.plan_mut(id)?, todo.clone(), after.clone())?;
        }
        Op::Start { label } => apply_start(state, id, label)?,
        Op::Done { label, output } => {
            let output = output.clone();
            check_terminal(label, output.as_ref())?;
            step_todo(
                state.plan_mut(id)?,
                label,
                OpKind::Done,
                decided,
                move |_| TodoState::Done { output },
            )?;
        }
        Op::Fail { label, cause } => {
            let cause = cause.clone();
            step_todo(
                state.plan_mut(id)?,
                label,
                OpKind::Fail,
                decided,
                move |last| TodoState::Failed { cause, last },
            )?;
        }
        Op::Retry { label, delegation } => {
            apply_retry(state.plan_mut(id)?, label, delegation.as_deref())?;
        }
        Op::Decompose { label, todos } => {
            applied.subplan = Some(apply_decompose(state, id, label, todos, decided)?);
        }
        Op::Supersede { reason, todos } => apply_supersede(state, id, reason, todos, decided)?,
        Op::Set { rows, .. } => apply_set(state.plan_mut(id)?, rows)?,
        Op::FuseReset => {
            let root = root_of(id)?;
            state.plan_mut(&root)?.reset_spawns();
        }
        Op::Repair { resolutions } => {
            for resolution in resolutions {
                apply_resolution(state, id, resolution)?;
            }
        }
        Op::Reconcile {
            label,
            effect_id,
            outcome,
        } => apply_reconcile(state, id, label, effect_id.as_ref(), outcome)?,
    }
    let plan = state.plan_mut(id)?;
    plan.touched = plan.touched.bump();
    if matches!(plan.state, PlanState::Active | PlanState::Done) {
        plan.state = if plan.finished() {
            PlanState::Done
        } else {
            PlanState::Active
        };
    }
    Ok(applied)
}

fn take_spawned(state: &mut RootState, id: &PlanId, label: &TodoLabel) -> Option<AgentId> {
    let effect = state
        .intents
        .iter()
        .find(|(_, intent)| {
            &intent.plan == id
                && &intent.label == label
                && matches!(intent.outcome, IntentOutcome::Spawned { .. })
        })
        .map(|(effect, _)| effect.clone())?;
    match state.intents.remove(&effect)?.outcome {
        IntentOutcome::Spawned { agent } => Some(agent),
        IntentOutcome::Pending => None,
    }
}

fn apply_start(state: &mut RootState, id: &PlanId, label: &TodoLabel) -> Result<(), PlanOpError> {
    let plan = state.plan(id)?;
    let index = locate_step(plan, label, OpKind::Start)?;
    let delegated = plan
        .todos
        .get(index)
        .is_some_and(|todo| todo.delegation.is_some());
    let by = if delegated {
        take_spawned(state, id, label).ok_or_else(|| PlanOpError::StartWithoutSpawn {
            label: label.clone(),
        })?
    } else {
        AgentId::new(OWNER_AGENT)?
    };
    step_todo(
        state.plan_mut(id)?,
        label,
        OpKind::Start,
        &Decided::default(),
        move |_| TodoState::Running { by },
    )
}

fn apply_retry(
    plan: &mut Plan,
    label: &TodoLabel,
    delegation: Option<&yi_types::plan::doc::Delegation>,
) -> Result<(), PlanOpError> {
    let index = locate_step(plan, label, OpKind::Retry)?;
    let id = plan.id.clone();
    let missing = || PlanOpError::UnknownLabel {
        plan: id.clone(),
        label: label.clone(),
    };
    let (spent, attempt) = match plan.todos.get(index) {
        Some(todo) => (todo.retries, todo.attempt),
        None => return Err(missing()),
    };
    let bumped = charge_retry(label, spent)?;
    let attempt = attempt.next().map_err(|_| PlanOpError::AttemptsExhausted {
        label: label.clone(),
    })?;
    step_todo(plan, label, OpKind::Retry, &Decided::default(), |_| {
        TodoState::Pending
    })?;
    let todo = plan.todos.get_mut(index).ok_or_else(missing)?;
    todo.retries = bumped;
    todo.attempt = attempt;
    if let Some(replacement) = delegation {
        todo.delegation = Some(replacement.clone());
    }
    Ok(())
}

fn apply_decompose(
    state: &mut RootState,
    id: &PlanId,
    label: &TodoLabel,
    specs: &[TodoSpec],
    decided: &Decided,
) -> Result<PlanId, PlanOpError> {
    let plan = state.plan(id)?;
    match &plan.tier {
        PlanTier::Root => {}
        PlanTier::Sub { .. } | PlanTier::Other { .. } => {
            return Err(PlanOpError::DepthExhausted {
                plan: plan.id.clone(),
            });
        }
    }
    let index = locate_step(plan, label, OpKind::Decompose)?;
    if let Some(existing) = plan.todos.get(index).and_then(|todo| todo.subplan.clone()) {
        return Err(PlanOpError::PlanExists { id: existing });
    }
    let sub_id = decided
        .subplan
        .clone()
        .ok_or_else(|| PlanOpError::SubplanUndecided {
            label: label.clone(),
        })?;
    if state.plans.contains_key(&sub_id) {
        return Err(PlanOpError::PlanExists { id: sub_id });
    }
    let sub = Plan::opening(
        sub_id.clone(),
        GoalText::new(label.as_str())?,
        PlanTier::Sub {
            parent: TodoAddr {
                plan: id.clone(),
                todo: label.clone(),
            },
        },
        specs.iter().cloned().map(new_todo).collect(),
    );
    validate_plan(&sub)?;
    let plan = state.plan_mut(id)?;
    let todo = plan
        .todos
        .get_mut(index)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    todo.subplan = Some(sub_id.clone());
    state.plans.insert(sub_id.clone(), sub);
    Ok(sub_id)
}

fn fail_superseded(plan: &mut Plan, reason: &str, decided: &Decided) {
    let id = plan.id.clone();
    for todo in &mut plan.todos {
        if todo.delegation.is_none() || !matches!(todo.state, TodoState::Running { .. }) {
            continue;
        }
        todo.state = TodoState::Failed {
            cause: format!("superseded: {reason}"),
            last: reaped_last(decided, &id, &todo.label),
        };
    }
}

fn apply_supersede(
    state: &mut RootState,
    id: &PlanId,
    reason: &str,
    specs: &[TodoSpec],
    decided: &Decided,
) -> Result<(), PlanOpError> {
    let mut next = state.plan(id)?.clone();
    next.version = next.version.bump();
    next.todos = specs.iter().cloned().map(new_todo).collect();
    validate_plan(&next)?;
    for sub in state.kin(id) {
        if let Some(plan) = state.plans.get_mut(&sub) {
            fail_superseded(plan, reason, decided);
            plan.state = PlanState::Abandoned;
        }
    }
    state.plans.insert(id.clone(), next);
    Ok(())
}

/// The checklist is the whole cut: a surviving label keeps its edges, delegation and
/// sub-plan, a new one starts plain, a missing one leaves, and every state is as written.
fn apply_set(plan: &mut Plan, rows: &[SetRow]) -> Result<(), PlanOpError> {
    let mut todos: Vec<Todo> = Vec::with_capacity(rows.len());
    for row in rows {
        let existing = plan.todos.iter().find(|todo| todo.label == row.spec.label);
        let mut todo = match existing {
            Some(existing) => existing.clone(),
            None => new_todo(row.spec.clone()),
        };
        todo.children = row.spec.children.clone();
        if TodoStateName::of(&todo.state) != row.state {
            todo.state = match row.state {
                TodoStateName::Running => TodoState::Running {
                    by: AgentId::new(OWNER_AGENT)?,
                },
                TodoStateName::Done => TodoState::Done { output: None },
                TodoStateName::Pending
                | TodoStateName::Blocked
                | TodoStateName::Failed
                | TodoStateName::Abandoned
                | TodoStateName::Other(_) => TodoState::Pending,
            };
        }
        todos.push(todo);
    }
    plan.todos = todos;
    plan.version = plan.version.bump();
    validate_plan(plan)
}

fn drop_intents(state: &mut RootState, id: &PlanId, label: &TodoLabel) {
    state
        .intents
        .retain(|_, intent| !(&intent.plan == id && &intent.label == label));
}

fn apply_resolution(
    state: &mut RootState,
    id: &PlanId,
    resolution: &Resolution,
) -> Result<(), PlanOpError> {
    let label = &resolution.label;
    let plan = state.plan(id)?;
    let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
        plan: id.clone(),
        label: label.clone(),
    })?;
    let running = matches!(todo.state, TodoState::Running { .. });
    if !running && state.intent_for(id, label).is_none() {
        return Err(PlanOpError::NotReconcilable {
            label: label.clone(),
        });
    }
    let attempt = todo.attempt;
    let index = plan
        .todos
        .iter()
        .position(|todo| &todo.label == label)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    drop_intents(state, id, label);
    let plan = state.plan_mut(id)?;
    let todo = plan
        .todos
        .get_mut(index)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    match &resolution.action {
        Resolve::Retry => {
            todo.attempt = attempt.next().map_err(|_| PlanOpError::AttemptsExhausted {
                label: label.clone(),
            })?;
            todo.state = TodoState::Pending;
        }
        Resolve::Fail { cause } => {
            todo.state = TodoState::Failed {
                cause: cause.clone(),
                last: None,
            };
        }
    }
    Ok(())
}

fn apply_reconcile(
    state: &mut RootState,
    id: &PlanId,
    label: &TodoLabel,
    effect_id: Option<&EffectId>,
    outcome: &Reconciliation,
) -> Result<(), PlanOpError> {
    let plan = state.plan(id)?;
    let index = plan
        .todos
        .iter()
        .position(|todo| &todo.label == label)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    let state_now = plan.todos.get(index).map(|todo| &todo.state);
    let reconcilable = matches!(
        state_now,
        Some(TodoState::Running { .. } | TodoState::Pending)
    ) && (matches!(state_now, Some(TodoState::Running { .. }))
        || state.intent_for(id, label).is_some());
    if !reconcilable {
        return Err(PlanOpError::NotReconcilable {
            label: label.clone(),
        });
    }
    if let Some(effect) = effect_id
        && !state.intents.contains_key(effect)
    {
        return Err(PlanOpError::NotReconcilable {
            label: label.clone(),
        });
    }
    drop_intents(state, id, label);
    let plan = state.plan_mut(id)?;
    let todo = plan
        .todos
        .get_mut(index)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: id.clone(),
            label: label.clone(),
        })?;
    match outcome {
        Reconciliation::Reattached { agent } => {
            todo.state = TodoState::Running { by: agent.clone() };
        }
        Reconciliation::Reused { output } => {
            check_terminal(label, Some(output))?;
            todo.state = TodoState::Done {
                output: Some(output.clone()),
            };
        }
    }
    Ok(())
}

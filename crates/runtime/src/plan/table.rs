use std::collections::HashSet;

use serde_json::Map;
use yi_types::plan::doc::{
    AgentId, Plan, PlanIssue, PlanState, RetryCount, SPAWN_CAP, Todo, TodoLabel, TodoState,
    terminal_durability,
};
use yi_types::url::{Durability, Url};

use super::ops::{Actor, OWNER_AGENT, PlanOpError, TodoSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    Init,
    Append,
    Drop,
    Block,
    Unblock,
    Reorder,
    AddEdge,
    Start,
    Done,
    Fail,
    Retry,
    Decompose,
    Supersede,
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateName {
    Pending,
    Running,
    Blocked,
    Done,
    Failed,
    Abandoned,
}

impl StateName {
    pub fn of(state: &TodoState) -> Option<Self> {
        match state {
            TodoState::Pending => Some(Self::Pending),
            TodoState::Running { .. } => Some(Self::Running),
            TodoState::Blocked { .. } => Some(Self::Blocked),
            TodoState::Done { .. } => Some(Self::Done),
            TodoState::Failed { .. } => Some(Self::Failed),
            TodoState::Abandoned => Some(Self::Abandoned),
            TodoState::Other(_) => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }
}

impl std::fmt::Display for StateName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub from: StateName,
    pub op: OpKind,
    pub to: StateName,
}

pub const STEPS: &[Step] = &[
    Step {
        from: StateName::Pending,
        op: OpKind::Start,
        to: StateName::Running,
    },
    Step {
        from: StateName::Pending,
        op: OpKind::Block,
        to: StateName::Blocked,
    },
    Step {
        from: StateName::Pending,
        op: OpKind::Drop,
        to: StateName::Abandoned,
    },
    Step {
        from: StateName::Pending,
        op: OpKind::AddEdge,
        to: StateName::Pending,
    },
    Step {
        from: StateName::Running,
        op: OpKind::Done,
        to: StateName::Done,
    },
    Step {
        from: StateName::Running,
        op: OpKind::Fail,
        to: StateName::Failed,
    },
    Step {
        from: StateName::Running,
        op: OpKind::Block,
        to: StateName::Blocked,
    },
    Step {
        from: StateName::Running,
        op: OpKind::Decompose,
        to: StateName::Running,
    },
    Step {
        from: StateName::Blocked,
        op: OpKind::Unblock,
        to: StateName::Pending,
    },
    Step {
        from: StateName::Blocked,
        op: OpKind::Drop,
        to: StateName::Abandoned,
    },
    Step {
        from: StateName::Blocked,
        op: OpKind::AddEdge,
        to: StateName::Blocked,
    },
    Step {
        from: StateName::Done,
        op: OpKind::AddEdge,
        to: StateName::Done,
    },
    Step {
        from: StateName::Failed,
        op: OpKind::Retry,
        to: StateName::Pending,
    },
    Step {
        from: StateName::Failed,
        op: OpKind::AddEdge,
        to: StateName::Failed,
    },
];

pub fn step(from: &TodoState, op: OpKind) -> Option<StateName> {
    let name = StateName::of(from)?;
    STEPS
        .iter()
        .find(|step| step.from == name && step.op == op)
        .map(|step| step.to)
}

/// Invariant: a plan that is not Active admits `view` alone, with one
/// carve-out — `retry` on a finished plan, the §9 ladder's first rung, which
/// only a Failed todo satisfies, so completed work cannot be resurrected.
pub(super) fn check_plan_state(plan: &Plan, op: OpKind) -> Result<(), PlanOpError> {
    let allowed = match &plan.state {
        PlanState::Active => true,
        PlanState::Done => matches!(op, OpKind::View | OpKind::Retry),
        PlanState::Superseded { .. } | PlanState::Abandoned | PlanState::Other(_) => {
            matches!(op, OpKind::View)
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(PlanOpError::NotActive {
            id: plan.id.clone(),
            state: plan.state.clone(),
        })
    }
}

pub(super) fn op_name(op: OpKind) -> &'static str {
    match op {
        OpKind::Init => "init",
        OpKind::Append => "append",
        OpKind::Drop => "drop",
        OpKind::Block => "block",
        OpKind::Unblock => "unblock",
        OpKind::Reorder => "reorder",
        OpKind::AddEdge => "add_edge",
        OpKind::Start => "start",
        OpKind::Done => "done",
        OpKind::Fail => "fail",
        OpKind::Retry => "retry",
        OpKind::Decompose => "decompose",
        OpKind::Supersede => "supersede",
        OpKind::View => "view",
    }
}

pub(super) fn check_actor(actor: &Actor, op: OpKind) -> Result<(), PlanOpError> {
    let allowed = match actor {
        Actor::Owner => true,
        Actor::User(_) | Actor::Host => matches!(op, OpKind::Unblock | OpKind::View),
        Actor::Child(_) => matches!(op, OpKind::View),
    };
    if allowed {
        Ok(())
    } else {
        Err(PlanOpError::NotOwner { op })
    }
}

pub(super) fn charge_spawn(plan: &mut Plan) -> Result<(), PlanOpError> {
    let spent = plan.spawns();
    if spent >= SPAWN_CAP {
        return Err(PlanOpError::SpawnCeilingExhausted {
            spent,
            cap: SPAWN_CAP,
        });
    }
    plan.charge_spawn();
    Ok(())
}

/// Invariant: a cap is refused and reported, never silently applied — the
/// saturating counter alone would cap retries by wedging at the same number
/// forever, so the table refuses before the bump.
pub const RETRY_CAP: RetryCount = RetryCount(8);

pub(super) fn charge_retry(
    label: &TodoLabel,
    spent: RetryCount,
) -> Result<RetryCount, PlanOpError> {
    if spent >= RETRY_CAP {
        return Err(PlanOpError::RetriesExhausted {
            label: label.clone(),
            spent,
            cap: RETRY_CAP,
        });
    }
    Ok(spent.bump())
}

pub(super) fn check_terminal(label: &TodoLabel, url: Option<&Url>) -> Result<(), PlanOpError> {
    let Some(url) = url else { return Ok(()) };
    if terminal_durability(url, &AgentId::new(OWNER_AGENT)?) == Durability::Ephemeral {
        return Err(PlanOpError::EphemeralTerminal {
            label: label.clone(),
            url: url.clone(),
        });
    }
    Ok(())
}

pub(super) fn in_flight(plan: &Plan) -> usize {
    plan.todos
        .iter()
        .filter(|todo| matches!(todo.state, TodoState::Running { .. }) && todo.delegation.is_some())
        .count()
}

pub(super) fn ready_labels(plan: &Plan) -> Vec<TodoLabel> {
    plan.ready()
        .into_iter()
        .map(|todo| todo.label.clone())
        .collect()
}

pub(super) fn admissible(plan: &Plan, slots: usize) -> Vec<TodoLabel> {
    let mut slots = slots;
    let mut out = Vec::new();
    for todo in plan.ready() {
        if todo.delegation.is_none() {
            out.push(todo.label.clone());
        } else if slots > 0 {
            slots = slots.saturating_sub(1);
            out.push(todo.label.clone());
        }
    }
    out
}

pub(super) fn validate_plan(plan: &Plan) -> Result<(), PlanOpError> {
    let mut issues = plan.validate();
    match issues.drain(..).next() {
        None => Ok(()),
        Some(PlanIssue::DuplicateLabel { label }) => Err(PlanOpError::LabelNotUnique { label }),
        Some(PlanIssue::SlugCollision { second, .. }) => {
            Err(PlanOpError::LabelNotUnique { label: second })
        }
        Some(issue @ (PlanIssue::UnresolvedEdge { .. } | PlanIssue::Cycle { .. })) => {
            Err(PlanOpError::Invalid { issue })
        }
    }
}

pub(super) fn locate_step(
    plan: &Plan,
    label: &TodoLabel,
    op: OpKind,
) -> Result<usize, PlanOpError> {
    let index = plan
        .todos
        .iter()
        .position(|todo| &todo.label == label)
        .ok_or_else(|| PlanOpError::UnknownLabel {
            plan: plan.id.clone(),
            label: label.clone(),
        })?;
    let Some(todo) = plan.todos.get(index) else {
        return Err(PlanOpError::UnknownLabel {
            plan: plan.id.clone(),
            label: label.clone(),
        });
    };
    let Some(from) = StateName::of(&todo.state) else {
        let state = match &todo.state {
            TodoState::Other(tag) => tag.clone(),
            TodoState::Pending
            | TodoState::Running { .. }
            | TodoState::Blocked { .. }
            | TodoState::Done { .. }
            | TodoState::Failed { .. }
            | TodoState::Abandoned => String::new(),
        };
        return Err(PlanOpError::UnknownState {
            label: label.clone(),
            state,
        });
    };
    if step(&todo.state, op).is_none() {
        return Err(PlanOpError::IllegalStep {
            label: label.clone(),
            from,
            op,
        });
    }
    if op == OpKind::Start
        && let Some(unmet) = todo
            .after
            .iter()
            .find(|after| !plan.todo(after).is_some_and(|dep| dep.state.clears_edge()))
    {
        return Err(PlanOpError::UnmetEdge {
            label: label.clone(),
            after: unmet.clone(),
        });
    }
    Ok(index)
}

pub(super) fn new_todo(spec: TodoSpec) -> Todo {
    Todo {
        label: spec.label,
        after: spec.after,
        state: TodoState::Pending,
        delegation: spec.delegation,
        subplan: None,
        retries: RetryCount::default(),
        extra: Map::new(),
    }
}

pub(super) fn append_todos(plan: &mut Plan, specs: Vec<TodoSpec>) -> Result<(), PlanOpError> {
    for spec in specs {
        plan.todos.push(new_todo(spec));
    }
    validate_plan(plan)
}

pub(super) fn reorder_todos(plan: &mut Plan, labels: Vec<TodoLabel>) -> Result<(), PlanOpError> {
    let expected = plan.todos.len();
    let have: HashSet<&TodoLabel> = plan.todos.iter().map(|todo| &todo.label).collect();
    let want: HashSet<&TodoLabel> = labels.iter().collect();
    if labels.len() != expected || want.len() != labels.len() || have != want {
        return Err(PlanOpError::NotAPermutation {
            got: labels.len(),
            expected,
        });
    }
    let mut old = std::mem::take(&mut plan.todos);
    for label in &labels {
        if let Some(position) = old.iter().position(|todo| &todo.label == label) {
            plan.todos.push(old.remove(position));
        }
    }
    Ok(())
}

pub(super) fn add_edge(
    plan: &mut Plan,
    todo: TodoLabel,
    after: TodoLabel,
) -> Result<(), PlanOpError> {
    if plan.todo(&after).is_none() {
        return Err(PlanOpError::UnknownLabel {
            plan: plan.id.clone(),
            label: after,
        });
    }
    let index = locate_step(plan, &todo, OpKind::AddEdge)?;
    let plan_id = plan.id.clone();
    let Some(entry) = plan.todos.get_mut(index) else {
        return Err(PlanOpError::UnknownLabel {
            plan: plan_id,
            label: todo,
        });
    };
    if !entry.after.contains(&after) {
        entry.after.push(after);
    }
    validate_plan(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::plan::doc::{AgentId, BlockedOn};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const OPS: [OpKind; 14] = [
        OpKind::Init,
        OpKind::Append,
        OpKind::Drop,
        OpKind::Block,
        OpKind::Unblock,
        OpKind::Reorder,
        OpKind::AddEdge,
        OpKind::Start,
        OpKind::Done,
        OpKind::Fail,
        OpKind::Retry,
        OpKind::Decompose,
        OpKind::Supersede,
        OpKind::View,
    ];

    fn named_states() -> Result<Vec<(StateName, TodoState)>, Box<dyn std::error::Error>> {
        Ok(vec![
            (StateName::Pending, TodoState::Pending),
            (
                StateName::Running,
                TodoState::Running {
                    by: AgentId::new("main")?,
                },
            ),
            (
                StateName::Blocked,
                TodoState::Blocked {
                    on: BlockedOn::User,
                    note: String::new(),
                },
            ),
            (StateName::Done, TodoState::Done { output: None }),
            (
                StateName::Failed,
                TodoState::Failed {
                    cause: "probe disagreed".to_owned(),
                    last: None,
                },
            ),
            (StateName::Abandoned, TodoState::Abandoned),
        ])
    }

    #[test]
    fn every_transition_agrees_with_the_table() -> TestResult {
        for (name, state) in named_states()? {
            for op in OPS {
                let expected = STEPS
                    .iter()
                    .find(|step| step.from == name && step.op == op)
                    .map(|step| step.to);
                assert_eq!(step(&state, op), expected, "{name} x {op:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn known_legal_steps_land_where_the_fixtures_expect() -> TestResult {
        let running = TodoState::Running {
            by: AgentId::new("main")?,
        };
        assert_eq!(
            step(&TodoState::Pending, OpKind::Start),
            Some(StateName::Running)
        );
        assert_eq!(step(&running, OpKind::Done), Some(StateName::Done));
        assert_eq!(step(&running, OpKind::Fail), Some(StateName::Failed));
        assert_eq!(step(&running, OpKind::Decompose), Some(StateName::Running));
        assert_eq!(
            step(
                &TodoState::Failed {
                    cause: "x".to_owned(),
                    last: None
                },
                OpKind::Retry
            ),
            Some(StateName::Pending)
        );
        assert_eq!(
            step(&TodoState::Done { output: None }, OpKind::AddEdge),
            Some(StateName::Done)
        );
        assert_eq!(step(&TodoState::Pending, OpKind::Done), None);
        assert_eq!(step(&TodoState::Pending, OpKind::Retry), None);
        assert_eq!(step(&running, OpKind::Start), None);
        assert_eq!(step(&running, OpKind::AddEdge), None);
        Ok(())
    }

    #[test]
    fn terminal_states_admit_only_retry_and_edge_bookkeeping() -> TestResult {
        for (name, state) in named_states()? {
            match name {
                StateName::Abandoned => {
                    for op in OPS {
                        assert_eq!(step(&state, op), None, "{name} x {op:?}");
                    }
                }
                StateName::Done | StateName::Failed => {
                    for op in OPS {
                        let expected = match op {
                            OpKind::Retry if name == StateName::Failed => Some(StateName::Pending),
                            OpKind::AddEdge => Some(name),
                            OpKind::Init
                            | OpKind::Append
                            | OpKind::Drop
                            | OpKind::Block
                            | OpKind::Unblock
                            | OpKind::Reorder
                            | OpKind::Retry
                            | OpKind::Start
                            | OpKind::Done
                            | OpKind::Fail
                            | OpKind::Decompose
                            | OpKind::Supersede
                            | OpKind::View => None,
                        };
                        assert_eq!(step(&state, op), expected, "{name} x {op:?}");
                    }
                }
                StateName::Pending | StateName::Running | StateName::Blocked => {}
            }
        }
        Ok(())
    }

    #[test]
    fn unknown_state_admits_no_op_at_all() {
        let other = TodoState::Other("paused".to_owned());
        for op in OPS {
            assert_eq!(step(&other, op), None);
        }
    }

    #[test]
    fn the_retry_cap_refuses_rather_than_saturates() -> TestResult {
        let label = TodoLabel::new("flaky step")?;
        assert_eq!(charge_retry(&label, RetryCount(0))?, RetryCount(1));
        let refused = charge_retry(&label, RETRY_CAP);
        match refused {
            Err(PlanOpError::RetriesExhausted { spent, cap, .. }) => {
                assert_eq!(spent, RETRY_CAP);
                assert_eq!(cap, RETRY_CAP);
            }
            other => return Err(format!("expected a cap refusal, got {other:?}").into()),
        }
        Ok(())
    }
}

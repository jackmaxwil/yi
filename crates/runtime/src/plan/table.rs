use std::collections::HashSet;

use yi_types::plan::doc::{
    AgentId, Plan, PlanIssue, PlanState, RetryCount, Todo, TodoLabel, TodoState, TodoStateName,
    terminal_durability,
};
use yi_types::plan::op::UNCONTRACTED_WORKTREE;
use yi_types::url::{Durability, Url};

use super::ops::{Actor, OWNER_AGENT, Op, PlanOpError, TodoSpec};

pub use yi_types::plan::op::{ALL_OPS, OpKind, op_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub from: TodoStateName,
    pub op: OpKind,
    pub to: TodoStateName,
}

pub const STEPS: &[Step] = &[
    Step {
        from: TodoStateName::Pending,
        op: OpKind::Start,
        to: TodoStateName::Running,
    },
    Step {
        from: TodoStateName::Pending,
        op: OpKind::Block,
        to: TodoStateName::Blocked,
    },
    Step {
        from: TodoStateName::Pending,
        op: OpKind::Drop,
        to: TodoStateName::Abandoned,
    },
    Step {
        from: TodoStateName::Pending,
        op: OpKind::AddEdge,
        to: TodoStateName::Pending,
    },
    Step {
        from: TodoStateName::Running,
        op: OpKind::Done,
        to: TodoStateName::Done,
    },
    Step {
        from: TodoStateName::Running,
        op: OpKind::Fail,
        to: TodoStateName::Failed,
    },
    Step {
        from: TodoStateName::Running,
        op: OpKind::Block,
        to: TodoStateName::Blocked,
    },
    Step {
        from: TodoStateName::Running,
        op: OpKind::Decompose,
        to: TodoStateName::Running,
    },
    Step {
        from: TodoStateName::Running,
        op: OpKind::Accept,
        to: TodoStateName::Done,
    },
    Step {
        from: TodoStateName::Blocked,
        op: OpKind::Unblock,
        to: TodoStateName::Pending,
    },
    Step {
        from: TodoStateName::Blocked,
        op: OpKind::Drop,
        to: TodoStateName::Abandoned,
    },
    Step {
        from: TodoStateName::Blocked,
        op: OpKind::AddEdge,
        to: TodoStateName::Blocked,
    },
    Step {
        from: TodoStateName::Blocked,
        op: OpKind::Accept,
        to: TodoStateName::Done,
    },
    Step {
        from: TodoStateName::Done,
        op: OpKind::AddEdge,
        to: TodoStateName::Done,
    },
    Step {
        from: TodoStateName::Failed,
        op: OpKind::Retry,
        to: TodoStateName::Pending,
    },
    Step {
        from: TodoStateName::Failed,
        op: OpKind::AddEdge,
        to: TodoStateName::Failed,
    },
    Step {
        from: TodoStateName::Failed,
        op: OpKind::Accept,
        to: TodoStateName::Done,
    },
];

/// The legal move a refusal named the rule for and not the road to: F0e sessions repeated
/// `done` on three sibling pending todos in a row because nothing said what to do (#472).
pub(super) fn illegal_hint(op: OpKind, from: &TodoStateName) -> &'static str {
    match (op, from) {
        (OpKind::Done, TodoStateName::Pending) => {
            "; start it first, or resend set with the row marked \"- [x]\" for a todo carrying no contract"
        }
        (OpKind::Drop, TodoStateName::Running) => "; a running todo ends with fail and a cause",
        (OpKind::Fail, TodoStateName::Pending) => "; a pending todo that will not run is dropped",
        (OpKind::Retry, TodoStateName::Pending) => "; it has not failed, so start it",
        (OpKind::Retry, TodoStateName::Running) => "; retry takes a failed todo: fail it first",
        _ => "",
    }
}

pub fn step(from: &TodoState, op: OpKind) -> Option<TodoStateName> {
    let name = TodoStateName::of(from);
    STEPS
        .iter()
        .find(|step| step.from == name && step.op == op)
        .map(|step| step.to.clone())
}

/// Invariant: a plan that is not Active admits `view` alone, bar `retry` on a finished plan,
/// the §9 ladder's first rung, and `set`, whose rows reopen it; its state is the rows' again.
pub(super) fn check_plan_state(plan: &Plan, op: OpKind) -> Result<(), PlanOpError> {
    let allowed = match &plan.state {
        PlanState::Active => true,
        PlanState::Done => {
            matches!(
                op,
                OpKind::View | OpKind::Retry | OpKind::Program | OpKind::Set
            )
        }
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

/// Invariant: authority is a channel (plan section 5.6). `User` is minted only by the confirmed
/// path, so the ops it alone may apply are the ones the owner is refused here.
pub(super) fn check_actor(actor: &Actor, op: &Op) -> Result<(), PlanOpError> {
    let kind = op.kind();
    let allowed = match actor {
        Actor::Owner => match op {
            Op::FuseReset | Op::Resolve { .. } | Op::Accept { .. } => false,
            Op::Repair { resolutions } => resolutions.is_empty(),
            _ => true,
        },
        Actor::User(_) => true,
        Actor::Classifier => matches!(op, Op::Accept { .. }),
        Actor::Host => matches!(kind, OpKind::Unblock | OpKind::Reconcile),
        Actor::Child(_) => matches!(kind, OpKind::View | OpKind::Submit),
        Actor::Engine => matches!(
            kind,
            OpKind::Start | OpKind::Submit | OpKind::Done | OpKind::Fail | OpKind::Retry
        ),
    };
    if allowed {
        Ok(())
    } else {
        Err(PlanOpError::NotOwner { op: kind })
    }
}

/// Invariant: a cap is refused and reported, never silently applied: a saturating counter
/// alone would wedge retries at the same number forever, so the table refuses first.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub label: TodoLabel,
    pub position: usize,
    pub delegated_ready: usize,
    pub slots: usize,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} is delegated ready todo {} of {}; {} slot(s) free",
            self.label, self.position, self.delegated_ready, self.slots
        )
    }
}

/// The ready labels `admit` does not refuse at `slots`; the rest are the held list.
pub(super) fn admitted(plan: &Plan, slots: usize) -> Vec<TodoLabel> {
    ready_labels(plan)
        .into_iter()
        .filter(|label| admit(plan, label, slots).is_ok())
        .collect()
}

pub fn admit(plan: &Plan, label: &TodoLabel, slots: usize) -> Result<(), Refusal> {
    let ready = plan.ready();
    let delegated: Vec<&Todo> = ready
        .iter()
        .copied()
        .filter(|todo| todo.delegation.is_some())
        .collect();
    let Some(todo) = ready.iter().find(|todo| &todo.label == label) else {
        return Err(Refusal {
            label: label.clone(),
            position: 0,
            delegated_ready: delegated.len(),
            slots,
        });
    };
    if todo.delegation.is_none() {
        return Ok(());
    }
    let position = delegated
        .iter()
        .position(|todo| &todo.label == label)
        .map_or(0, |index| index.saturating_add(1));
    if position <= slots {
        Ok(())
    } else {
        Err(Refusal {
            label: label.clone(),
            position,
            delegated_ready: delegated.len(),
            slots,
        })
    }
}

/// The shape alone. The declaration rule of plan section 6.6 is the parse of a `TodoSpec`, so a
/// stored document and an import are read as they were written; `retry` alone re-checks it.
pub(super) fn validate_plan(plan: &Plan) -> Result<(), PlanOpError> {
    let mut issues = plan.validate();
    match issues.drain(..).next() {
        None => Ok(()),
        Some(PlanIssue::DuplicateLabel { label }) => Err(PlanOpError::LabelNotUnique { label }),
        Some(PlanIssue::SlugCollision { second, .. }) => {
            Err(PlanOpError::LabelNotUnique { label: second })
        }
        Some(
            issue @ (PlanIssue::UnresolvedEdge { .. }
            | PlanIssue::Cycle { .. }
            | PlanIssue::Contract { .. }
            | PlanIssue::Unanswered { .. }),
        ) => Err(PlanOpError::Invalid { issue }),
    }
}

/// A `retry` swaps a delegation onto a todo the parse never saw beside its contract.
pub(super) fn check_contracted(todo: &Todo) -> Result<(), PlanOpError> {
    if super::acceptance::is_worktree(todo) && todo.contract.is_none() {
        return Err(PlanOpError::Contract {
            label: todo.label.clone(),
            detail: UNCONTRACTED_WORKTREE.to_owned(),
        });
    }
    Ok(())
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
    let from = TodoStateName::of(&todo.state);
    if let TodoStateName::Other(state) = from {
        return Err(PlanOpError::UnknownState {
            label: label.clone(),
            state,
        });
    }
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
        after: spec.after,
        delegation: spec.delegation,
        children: spec.children,
        contract: spec.contract,
        cites: spec.cites,
        ..Todo::pending(spec.label)
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
    if labels.len() != expected || want.len() != labels.len() {
        return Err(PlanOpError::NotAPermutation {
            got: labels.len(),
            expected,
        });
    }
    if let Some(label) = labels.iter().find(|label| !have.contains(label)) {
        return Err(PlanOpError::UnknownLabel {
            plan: plan.id.clone(),
            label: label.clone(),
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
    use serde_json::Map;
    use yi_types::plan::doc::{AgentId, BlockedOn};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const OPS: [OpKind; 23] = ALL_OPS;

    fn named_states() -> Result<Vec<(TodoStateName, TodoState)>, Box<dyn std::error::Error>> {
        Ok(vec![
            (TodoStateName::Pending, TodoState::Pending),
            (
                TodoStateName::Running,
                TodoState::Running {
                    by: AgentId::new("main")?,
                },
            ),
            (
                TodoStateName::Blocked,
                TodoState::Blocked {
                    on: BlockedOn::User,
                    note: String::new(),
                },
            ),
            (
                TodoStateName::Done,
                TodoState::Done {
                    output: None,
                    resolution: None,
                },
            ),
            (
                TodoStateName::Failed,
                TodoState::Failed {
                    cause: "probe disagreed".to_owned(),
                    last: None,
                },
            ),
            (TodoStateName::Abandoned, TodoState::Abandoned),
        ])
    }

    #[test]
    fn every_transition_agrees_with_the_table() -> TestResult {
        for (name, state) in named_states()? {
            for op in OPS {
                let expected = STEPS
                    .iter()
                    .find(|step| step.from == name && step.op == op)
                    .map(|step| step.to.clone());
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
            Some(TodoStateName::Running)
        );
        assert_eq!(step(&running, OpKind::Done), Some(TodoStateName::Done));
        assert_eq!(step(&running, OpKind::Fail), Some(TodoStateName::Failed));
        assert_eq!(
            step(&running, OpKind::Decompose),
            Some(TodoStateName::Running)
        );
        assert_eq!(
            step(
                &TodoState::Failed {
                    cause: "x".to_owned(),
                    last: None
                },
                OpKind::Retry
            ),
            Some(TodoStateName::Pending)
        );
        assert_eq!(
            step(
                &TodoState::Done {
                    output: None,
                    resolution: None
                },
                OpKind::AddEdge
            ),
            Some(TodoStateName::Done)
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
            match &name {
                TodoStateName::Abandoned => {
                    for op in OPS {
                        assert_eq!(step(&state, op), None, "{name} x {op:?}");
                    }
                }
                TodoStateName::Done | TodoStateName::Failed => {
                    for op in OPS {
                        let expected = match op {
                            OpKind::Retry if name == TodoStateName::Failed => {
                                Some(TodoStateName::Pending)
                            }
                            OpKind::AddEdge => Some(name.clone()),
                            OpKind::Accept if name == TodoStateName::Failed => {
                                Some(TodoStateName::Done)
                            }
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
                            | OpKind::Set
                            | OpKind::View
                            | OpKind::FuseReset
                            | OpKind::Repair
                            | OpKind::Import
                            | OpKind::Reconcile
                            | OpKind::Submit
                            | OpKind::Resolve
                            | OpKind::Accept
                            | OpKind::Program => None,
                        };
                        assert_eq!(step(&state, op), expected, "{name} x {op:?}");
                    }
                }
                TodoStateName::Pending
                | TodoStateName::Running
                | TodoStateName::Blocked
                | TodoStateName::Other(_) => {}
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

    #[test]
    fn admit_refuses_the_ninth_delegated_start_with_the_count() -> TestResult {
        use yi_types::plan::doc::{Check, Delegation, GoalText, PlanId, PlanTier, SpawnSpec};
        let delegation = Delegation {
            spec: SpawnSpec {
                role: None,
                model: None,
                effort: None,
                tools: Vec::new(),
                isolation: None,
                budget: None,
                wall: None,
                parent_close: None,
                extra: Map::new(),
            },
            accept: Check::Stated("it works".to_owned()),
            output: None,
            context: Vec::new(),
            note: None,
            extra: Map::new(),
        };
        let mut todos = Vec::new();
        for index in 1..=9 {
            let mut todo = Todo::pending(TodoLabel::new(format!("job {index}"))?);
            todo.delegation = Some(delegation.clone());
            todos.push(todo);
        }
        todos.push(Todo::pending(TodoLabel::new("inline note")?));
        let plan = yi_types::plan::doc::Plan::opening(
            PlanId::new("wide")?,
            GoalText::new("nine delegated jobs")?,
            PlanTier::Root,
            todos,
        );
        for index in 1..=8 {
            admit(&plan, &TodoLabel::new(format!("job {index}"))?, 8)
                .map_err(|refusal| format!("job {index}: {refusal}"))?;
        }
        let refused = admit(&plan, &TodoLabel::new("job 9")?, 8);
        assert_eq!(
            refused,
            Err(Refusal {
                label: TodoLabel::new("job 9")?,
                position: 9,
                delegated_ready: 9,
                slots: 8,
            })
        );
        admit(&plan, &TodoLabel::new("inline note")?, 0)
            .map_err(|refusal| format!("inline: {refusal}"))?;
        let refused: Vec<TodoLabel> = plan
            .ready()
            .into_iter()
            .filter(|todo| admit(&plan, &todo.label, 8).is_err())
            .map(|todo| todo.label.clone())
            .collect();
        assert_eq!(
            refused,
            vec![TodoLabel::new("job 9")?],
            "eight delegated and the inline one pass; the ninth delegated is refused, never reordered"
        );
        Ok(())
    }
}

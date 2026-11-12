//! The red ladder (design section 13.1): the refused verdicts on a todo's attempt pick a rung,
//! and the rung says whether `done` runs its check and which op, by whom, buys one more claim.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use yi_types::plan::ask::{Ask, AskOption};
use yi_types::plan::doc::{Note, Plan, PlanId, TODO_LABEL_MAX, Todo, TodoLabel, TodoState};
use yi_types::plan::ledger::{AttemptId, JournalRecord};
use yi_types::plan::op::{Op, SplitSpec};

use super::ops::PlanOpError;
use super::state::{Decided, KIND_DONE_REFUSED};
use super::table::{OpKind, op_name, validate_plan};

pub const READMIT_KEY: &str = "readmit";
pub const SPLIT_PARENT_KEY: &str = "split_parent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    Anyone,
    User,
}

#[derive(Debug)]
pub(super) struct Rung {
    pub(super) demand: Option<&'static str>,
    pub(super) exits: &'static [(OpKind, By)],
    pub(super) blocks: bool,
}

/// Invariant: a rung that refuses a claim names an exit, or its todo deadlocks there.
pub(super) const LADDER: [Rung; 3] = [
    Rung {
        demand: None,
        exits: &[],
        blocks: false,
    },
    Rung {
        demand: Some(
            "make a structural change first: `{\"op\": \"split\", \"label\": LABEL, \"subtasks\": [{\"title\": \"one part\", \"acceptance\": \"how you know it holds\"}]}` (1 to 4 subtasks, each may carry a `contract`), or `{\"op\": \"append\", \"for_task\": LABEL, \"todos\": [...]}`",
        ),
        exits: &[(OpKind::Split, By::Anyone), (OpKind::Append, By::Anyone)],
        blocks: false,
    },
    Rung {
        demand: Some(
            "only the user's reply to a question buys another claim: when it is blocked on one, `unblock` it after they answer; otherwise `block` it on the user with `options` first",
        ),
        exits: &[(OpKind::Unblock, By::User)],
        blocks: true,
    },
];

pub(super) fn rung(red: u32, cap: u32) -> &'static Rung {
    let [free, structural, user] = &LADDER;
    if red >= cap {
        user
    } else if red > 0 && red.saturating_add(1) == cap {
        structural
    } else {
        free
    }
}

/// The decided `done_refused` verdicts on this todo's attempt: a `retry` opens a fresh count
/// (D212) unless the attempt it ends reached the cap, whose count the next attempt carries.
pub(super) fn red(
    records: &[JournalRecord],
    plan: &PlanId,
    label: &TodoLabel,
    attempt: AttemptId,
    cap: u32,
) -> u32 {
    let mut per_attempt: BTreeMap<u32, u32> = BTreeMap::new();
    for record in records.iter().filter(|record| {
        record.record.op == KIND_DONE_REFUSED
            && &record.record.plan == plan
            && record.record.todo.as_ref() == Some(label)
            && record
                .verdict
                .as_ref()
                .and_then(|verdict| verdict.get("outcome"))
                .and_then(Value::as_str)
                != Some("abstain")
    }) {
        if let Some(at) = record.attempt {
            let count = per_attempt.entry(at.get()).or_default();
            *count = count.saturating_add(1);
        }
    }
    let mut carried = 0;
    for at in 1..attempt.get() {
        let red = per_attempt
            .get(&at)
            .copied()
            .unwrap_or(0)
            .saturating_add(carried);
        carried = if red >= cap { red } else { 0 };
    }
    let own = per_attempt.get(&attempt.get()).copied().unwrap_or(0);
    own.saturating_add(carried)
}

/// The question the cap's block asks, so the user's reply is what `unblock` carries.
pub(super) fn cap_ask() -> Result<Ask, PlanOpError> {
    let option = |(id, label): (&str, &str)| -> Result<AskOption, PlanOpError> {
        Ok(AskOption {
            id: id
                .to_owned()
                .try_into()
                .map_err(|e: yi_types::plan::ask::AskError| PlanOpError::Ladder(e.to_string()))?,
            label: TodoLabel::new(label)?,
            preview: None,
            extra: Map::new(),
        })
    };
    let options = [
        ("again", "Try it once more"),
        ("accept", "Accept it as it is"),
        ("drop", "Drop it"),
    ];
    let options = options
        .into_iter()
        .map(option)
        .collect::<Result<Vec<_>, _>>()?;
    Ask::new(options).map_err(|error| PlanOpError::Ladder(error.to_string()))
}

fn with_label(demand: &str, label: &TodoLabel) -> String {
    let quoted = serde_json::to_string(label.as_str()).unwrap_or_default();
    demand.replace("LABEL", &quoted)
}

fn refused(op: OpKind, label: &TodoLabel, detail: impl std::fmt::Display) -> PlanOpError {
    PlanOpError::Ladder(format!("{} for {label:?} refused: {detail}", op_name(op)))
}

fn split_from(todo: &Todo) -> Option<&str> {
    todo.extra.get(SPLIT_PARENT_KEY).and_then(Value::as_str)
}

pub(super) fn gate(todo: &Todo, red: u32, cap: u32) -> Result<(), PlanOpError> {
    match rung(red, cap).demand {
        Some(demand) if todo.extra.get(READMIT_KEY) != Some(&Value::Bool(true)) => {
            let demand = with_label(demand, &todo.label);
            let detail = format!("before its check at red {red} of {cap}: {demand}");
            Err(refused(OpKind::Done, &todo.label, detail))
        }
        _ => Ok(()),
    }
}

/// Whether `op` takes the exit of its todo's rung; a `split` or targeted `append` that would buy
/// nothing is refused, an `unblock` still unblocks.
pub(super) fn admit(
    records: &[JournalRecord],
    plan: &Plan,
    op: &Op,
    cap: u32,
) -> Result<bool, PlanOpError> {
    let (label, by_user) = match op {
        Op::Split { label, .. }
        | Op::Append {
            for_task: Some(label),
            ..
        } => (label, false),
        Op::Unblock { label, answer } => (label, answer.is_some()),
        _ => return Ok(false),
    };
    let todo = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
        plan: plan.id.clone(),
        label: label.clone(),
    })?;
    let red = red(records, &plan.id, label, todo.attempt, cap);
    let rung = rung(red, cap);
    let opens =
        (rung.exits.iter()).any(|(kind, by)| *kind == op.kind() && (*by == By::Anyone || by_user));
    if opens || matches!(op, Op::Unblock { .. }) {
        return Ok(opens);
    }
    let needs = rung
        .demand
        .unwrap_or("its next `done` runs its check already");
    let detail = format!("it buys no claim at red {red} of {cap}: {needs}");
    Err(refused(op.kind(), label, detail))
}

pub(super) fn grant<'a>(plan: &'a mut Plan, op: &Op, decided: &Decided) -> &'a mut Plan {
    if decided.readmit
        && let Some(label) = op.label()
        && let Some(todo) = plan.todos.iter_mut().find(|todo| &todo.label == label)
    {
        todo.extra.insert(READMIT_KEY.to_owned(), Value::Bool(true));
    }
    plan
}

pub(super) fn spend(plan: &mut Plan, label: &TodoLabel) {
    if let Some(todo) = plan.todos.iter_mut().find(|todo| &todo.label == label) {
        todo.extra.remove(READMIT_KEY);
    }
}

/// Invariant: a split parent completes only once each of its children is done or dropped.
pub(super) fn split_pending(plan: &Plan, label: &TodoLabel) -> Result<(), PlanOpError> {
    let open = plan.todos.iter().find(|todo| {
        split_from(todo) == Some(label.as_str())
            && !matches!(todo.state, TodoState::Done { .. } | TodoState::Abandoned)
    });
    match open {
        Some(child) => {
            let detail = format!("its split child {:?} is not finished", child.label);
            Err(refused(OpKind::Done, label, detail))
        }
        None => Ok(()),
    }
}

const SPLIT_WIDTH: usize = 4;

const SPLIT_SUFFIX: usize = 8;

pub(super) fn split(
    plan: &mut Plan,
    label: &TodoLabel,
    subtasks: &[SplitSpec],
) -> Result<(), PlanOpError> {
    let refuse = |detail: &dyn std::fmt::Display| refused(OpKind::Split, label, detail);
    if subtasks.is_empty() || subtasks.len() > SPLIT_WIDTH {
        let width = subtasks.len();
        return Err(refuse(&format!(
            "{width} subtasks; a split takes 1 to {SPLIT_WIDTH}"
        )));
    }
    let parent = plan.todo(label).ok_or_else(|| PlanOpError::UnknownLabel {
        plan: plan.id.clone(),
        label: label.clone(),
    })?;
    let refusal = if split_from(parent).is_some() {
        Some("a split child is never split again")
    } else if matches!(parent.state, TodoState::Done { .. }) {
        Some("a done todo is not split")
    } else if (plan.todos.iter()).any(|todo| split_from(todo) == Some(label.as_str())) {
        Some("it was split once already; finish or drop those children")
    } else {
        None
    };
    if let Some(refusal) = refusal {
        return Err(refuse(&refusal));
    }
    let keep = TODO_LABEL_MAX.saturating_sub(SPLIT_SUFFIX);
    let prefix: String = (label.as_str().char_indices())
        .take_while(|(at, ch)| at.saturating_add(ch.len_utf8()) <= keep)
        .map(|(_, ch)| ch)
        .collect();
    let mut children = Vec::with_capacity(subtasks.len());
    for (n, spec) in (1usize..).zip(subtasks) {
        if spec.title.trim().is_empty() || spec.acceptance.trim().is_empty() {
            return Err(refuse(&"each subtask needs a title and an acceptance"));
        }
        let child_label = TodoLabel::new(format!("{prefix}-split-{n}"));
        let note = Note::new(format!("{}\n\nAcceptance: {}", spec.title, spec.acceptance));
        let mut child = Todo::pending(child_label.map_err(|error| refuse(&error))?);
        child.note = Some(note.map_err(|error| refuse(&error))?);
        let parent_label = Value::String(label.as_str().to_owned());
        child
            .extra
            .insert(SPLIT_PARENT_KEY.to_owned(), parent_label);
        child.after.clone_from(&parent.after);
        child.cites = parent.cites.clone();
        child.contract = spec.contract.clone();
        children.push(child);
    }
    let edges: Vec<TodoLabel> = children.iter().map(|child| child.label.clone()).collect();
    plan.todos.extend(children);
    if let Some(parent) = plan.todos.iter_mut().find(|todo| &todo.label == label) {
        parent.after.extend(edges);
    }
    plan.version = plan.version.bump();
    validate_plan(plan)
}

#[cfg(test)]
mod tests {
    use super::{LADDER, rung};
    use crate::plan::table::step;
    use yi_types::plan::doc::{BlockedOn, TodoState, TodoStateName};

    #[test]
    fn every_rung_that_refuses_a_claim_has_an_exit_its_state_admits() {
        let blocked = TodoState::Blocked {
            on: BlockedOn::User,
            note: String::new(),
        };
        for rung in &LADDER {
            assert_eq!(rung.demand.is_some(), !rung.exits.is_empty(), "{rung:?}");
            for (kind, _) in rung.exits.iter().filter(|_| rung.blocks) {
                assert_eq!(
                    step(&blocked, *kind),
                    Some(TodoStateName::Pending),
                    "{rung:?}"
                );
            }
        }
    }

    #[test]
    fn the_cap_lever_moves_both_gated_rungs() {
        let demands = |cap| {
            (0..5)
                .map(|red| rung(red, cap).demand.is_some())
                .collect::<Vec<_>>()
        };
        assert_eq!(demands(3), [false, false, true, true, true]);
        assert_eq!(demands(5), [false, false, false, false, true]);
        assert_eq!(demands(1), [false, true, true, true, true]);
        assert!(rung(1, 2).exits.len() == 2 && !rung(1, 2).blocks && rung(2, 2).blocks);
    }
}

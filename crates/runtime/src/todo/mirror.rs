use std::sync::Arc;

use serde_json::Value;
use yi_types::plan::doc::{self, Plan, PlanState, Todo};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::todo::{PhaseName, TodoList, TodoPhase};

use super::TodoStore;
use crate::plan::ops::{Actor, Op, OpRequest, OpSink, PlanEngine};
use crate::plan::store::PlanStore;

pub const ENGINE_ACTOR: &str = "engine";
pub const PLAN_KEY: &str = "plan";
/// Marks a mirrored row the session wrote before a plan absorbed it, so it outlives that plan.
const OWN_KEY: &str = "own";

pub fn plan_of(list: &TodoList) -> Option<&str> {
    list.extra.get(PLAN_KEY).and_then(Value::as_str)
}

/// The plan a mirrored row came from; a row of the session's own has none.
pub fn row_plan(item: &Todo) -> Option<doc::PlanId> {
    doc::PlanId::new(item.extra.get(PLAN_KEY)?.as_str()?).ok()
}

/// Invariant: each committed plan op re-projects its root plan into the list every reader shares.
pub struct Mirror {
    pub inner: Arc<dyn OpSink>,
    pub todos: Arc<TodoStore>,
    pub store: PlanStore,
}

impl Mirror {
    pub fn resync(store: PlanStore) -> Arc<super::ResyncFn> {
        Arc::new(move |current| {
            let plan = doc::PlanId::new(plan_of(current)?).ok()?;
            let root = crate::plan::state::root_of(&plan).ok()?;
            Some(projected(&store.read(&root).ok()?, current))
        })
    }
}

impl OpSink for Mirror {
    fn record(&self, record: PlanOpRecord) -> Result<(), String> {
        let root = crate::plan::state::root_of(&record.plan).map_err(|error| error.to_string())?;
        let recorded = self.inner.record(record);
        let plan = self.store.read(&root).map_err(|error| error.to_string())?;
        self.todos
            .replace_with(|current| Some(projected(&plan, current)), ENGINE_ACTOR);
        recorded
    }
}

fn row(todo: &Todo, plan: &Plan, was: Option<&Todo>) -> Todo {
    let mut item = todo.clone();
    // Invariant: the plan journal holds these; the list rides every session record, per op.
    item.delegation = None;
    item.contract = None;
    item.contract_hash = None;
    item.note = None;
    item.id = was.and_then(|seen| seen.id.clone());
    item.extra.insert(
        PLAN_KEY.to_owned(),
        Value::String(plan.id.as_str().to_owned()),
    );
    if was
        .is_some_and(|seen| !seen.extra.contains_key(PLAN_KEY) || seen.extra.contains_key(OWN_KEY))
    {
        item.extra.insert(OWN_KEY.to_owned(), Value::Bool(true));
    }
    item
}

pub fn own(list: &TodoList) -> TodoList {
    let mut own = list.clone();
    for phase in &mut own.phases {
        phase
            .items
            .retain(|item| !item.extra.contains_key(PLAN_KEY));
    }
    own.phases.retain(|phase| !phase.items.is_empty());
    own.extra.remove(PLAN_KEY);
    own
}

pub fn rejoin(mirrored: &TodoList, own: TodoList) -> TodoList {
    let mut list = mirrored.clone();
    list.phases.retain(|phase| {
        phase
            .items
            .iter()
            .any(|item| item.extra.contains_key(PLAN_KEY))
    });
    let plan = std::mem::replace(&mut list.phases, own.phases);
    list.phases.extend(plan);
    list.next_id = list.next_id.max(own.next_id);
    list
}

pub fn carry(engine: std::sync::Weak<PlanEngine>) -> Arc<super::CarryFn> {
    Arc::new(move |label, carried| {
        let engine = engine.upgrade().ok_or("the plan engine is gone")?;
        let plan = match &carried {
            super::Carried::Wait { plan, .. } => Some(plan.clone()),
            _ => None,
        };
        let apply = |op: Op| {
            let request = OpRequest {
                plan: plan.clone(),
                actor: if matches!(op, Op::Unblock { .. }) {
                    Actor::Host
                } else {
                    Actor::Owner
                },
                op: op.clone(),
                request_id: None,
                expected_revision: None,
            };
            engine
                .apply(request)
                .map(|outcome| Some(crate::plan::render::render_outcome(&op, &outcome)))
                .map_err(|error| error.to_string())
        };
        let start = Op::Start {
            label: label.clone(),
        };
        let unblock = Op::Unblock {
            label: label.clone(),
            answer: None,
        };
        let pending = match carried {
            super::Carried::Start => return apply(start),
            super::Carried::Block { on, note, ask } => {
                let label = label.clone();
                return apply(Op::Block {
                    label,
                    on,
                    note,
                    ask,
                });
            }
            super::Carried::Wait { plan, address } => {
                let read = engine
                    .store()
                    .read(&plan)
                    .map_err(|error| error.to_string())?;
                let waits = read.todos.iter().any(|todo| {
                    todo.label == *label
                        && matches!(&todo.state, doc::TodoState::Blocked { on, .. }
                            if crate::schedule::clock::wait_of(on).is_some_and(|(at, _)| at == address))
                });
                return if waits { apply(unblock) } else { Ok(None) };
            }
            super::Carried::Done { pending } => pending,
        };
        if pending {
            apply(start)?;
        }
        apply(Op::Done {
            label: label.clone(),
            output: None,
        })
    })
}

pub fn projected(plan: &Plan, current: &TodoList) -> TodoList {
    let Ok(name) =
        PhaseName::new(plan.id.as_str()).or_else(|_| PhaseName::new(super::DEFAULT_PHASE))
    else {
        return current.clone();
    };
    let items = plan
        .todos
        .iter()
        .map(|todo| {
            let was = current
                .phases
                .iter()
                .flat_map(|phase| &phase.items)
                .find(|seen| seen.label == todo.label);
            let mut item = row(todo, plan, was);
            for child in &mut item.children {
                let seen =
                    was.and_then(|was| was.children.iter().find(|seen| seen.label == child.label));
                *child = row(child, plan, seen);
            }
            item
        })
        .collect();
    let mut phases = own(current).phases;
    // Incident: a second plan's projection dropped the rows the first had absorbed from the
    // session's list, and the model's `todo done` on them was refused as stale.
    let returned: Vec<Todo> = (current.items())
        .filter(|item| item.extra.contains_key(OWN_KEY))
        .filter(|item| row_plan(item).is_some_and(|id| id != plan.id))
        .map(|item| {
            let mut item = item.clone();
            item.extra.remove(PLAN_KEY);
            item.extra.remove(OWN_KEY);
            item
        })
        .collect();
    if let (false, Ok(name)) = (returned.is_empty(), PhaseName::new(super::DEFAULT_PHASE)) {
        phases.push(TodoPhase {
            name,
            items: returned,
            extra: serde_json::Map::new(),
        });
    }
    for phase in &mut phases {
        phase.items.retain(|item| plan.todo(&item.label).is_none());
    }
    phases.retain(|phase| !phase.items.is_empty());
    phases.push(TodoPhase {
        name,
        items,
        extra: serde_json::Map::new(),
    });
    let mut list = TodoList {
        phases,
        next_id: current.next_id,
        extra: serde_json::Map::new(),
    };
    if plan.state == PlanState::Active {
        list.extra.insert(
            PLAN_KEY.to_owned(),
            Value::String(plan.id.as_str().to_owned()),
        );
    }
    super::mint(&mut list);
    list
}

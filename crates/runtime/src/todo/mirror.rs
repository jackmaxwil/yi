use std::sync::Arc;

use serde_json::Value;
use yi_types::plan::doc::{self, Plan, PlanState, Todo, TodoState, TodoStateName};
use yi_types::plan::ledger::PlanOpRecord;
use yi_types::todo::{BlockedOn, PhaseName, TodoItem, TodoList, TodoPhase};

use super::TodoStore;
use crate::plan::ops::OpSink;
use crate::plan::store::PlanStore;

pub const ENGINE_ACTOR: &str = "engine";
pub const PLAN_KEY: &str = "plan";

pub fn plan_of(list: &TodoList) -> Option<&str> {
    list.extra.get(PLAN_KEY).and_then(Value::as_str)
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
            .replace(projected(&plan, &self.todos.list()), ENGINE_ACTOR);
        recorded
    }
}

fn blocked_on(on: &doc::BlockedOn) -> BlockedOn {
    match on {
        doc::BlockedOn::User => BlockedOn::User,
        doc::BlockedOn::External { .. } => BlockedOn::External,
        doc::BlockedOn::Child(_) => BlockedOn::Child,
        doc::BlockedOn::Other(tag) => BlockedOn::Other(tag.clone()),
    }
}

fn row(todo: &Todo, plan: &Plan, was: Option<&TodoItem>) -> TodoItem {
    let mut item = TodoItem::pending(todo.label.clone());
    item.id = was.and_then(|seen| seen.id.clone());
    item.state = TodoStateName::of(&todo.state);
    match &todo.state {
        TodoState::Blocked { on, note } => {
            item.on = Some(blocked_on(on));
            item.note = Some(note.clone());
        }
        TodoState::Failed { cause, .. } => item.note = Some(cause.clone()),
        TodoState::Running { by } => {
            item.extra
                .insert("by".to_owned(), Value::String(by.as_str().to_owned()));
        }
        TodoState::Pending
        | TodoState::Done { .. }
        | TodoState::Abandoned
        | TodoState::Other(_) => {}
    }
    item.extra.insert(
        PLAN_KEY.to_owned(),
        Value::String(plan.id.as_str().to_owned()),
    );
    item
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
            item.children = todo
                .children
                .iter()
                .map(|child| {
                    let seen = was
                        .and_then(|was| was.children.iter().find(|seen| seen.label == child.label));
                    row(child, plan, seen)
                })
                .collect();
            item
        })
        .collect();
    let mut list = TodoList {
        phases: vec![TodoPhase {
            name,
            items,
            extra: serde_json::Map::new(),
        }],
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

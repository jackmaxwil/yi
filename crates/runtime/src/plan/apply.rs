//! Whole-plan apply: `set` with rows sends the plan as it should be, and every row the engine
//! cannot reach is a condition on that row in the reply, never a refusal of the call.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use yi_types::plan::doc::{PlanIssue, TodoStateName};

use super::ops::{Op, OpRequest, PlanOpError, SetRow};
use super::table::OpKind;
use super::tool::{ArgError, PlanTool, PlanToolError, opt, todo_specs};

/// A plan the repairs cannot settle in this many passes is refused as it stands.
const REPAIRS: usize = 32;

/// `set` with a row that carries more than a label: a contract, edges, a delegation or a state.
pub(super) fn wants(args: &Map<String, Value>) -> bool {
    let rich =
        |row: &Value| (row.as_object()).is_some_and(|row| row.keys().any(|key| key != "label"));
    args.get("op").and_then(Value::as_str) == Some("set")
        && (args.get("todos").and_then(Value::as_array)).is_some_and(|rows| rows.iter().any(rich))
}

/// `set`'s rows as objects: each one's `state` (pending unless named) beside its spec.
pub(super) fn set_rows(args: &Map<String, Value>, op: OpKind) -> Result<Vec<SetRow>, ArgError> {
    let mut stripped = args.clone();
    let mut states = Vec::new();
    let rows = stripped
        .get_mut("todos")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten();
    for row in rows.filter_map(Value::as_object_mut) {
        states.push(opt::<TodoStateName>(row, op, "state")?.unwrap_or(TodoStateName::Pending));
        row.remove("state");
    }
    let specs = todo_specs(&stripped, op)?;
    if specs.is_empty() {
        return Err(ArgError::EmptyList);
    }
    Ok(specs
        .into_iter()
        .zip(states)
        .map(|(spec, state)| SetRow { spec, state })
        .collect())
}

pub(super) fn apply(tool: &PlanTool, args: &Map<String, Value>) -> Result<String, PlanToolError> {
    let (now, mut rows, mut deferred, mut conditions) =
        (held(tool, args), Vec::new(), Vec::new(), Vec::new());
    for row in args
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(mut row) = row.as_object().cloned() else {
            conditions.push(format!("{row}: not a row object; left out"));
            continue;
        };
        let label = row
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let moved_by_engine = row.contains_key("contract") || row.contains_key("delegation");
        let (on, note, options) = (row.remove("on"), row.remove("note"), row.remove("options"));
        let engine_op = match row.get("state").and_then(Value::as_str) {
            Some("done") if moved_by_engine => Some(json!({"op": "done", "label": label})),
            Some("running") if row.contains_key("delegation") => {
                Some(json!({"op": "start", "label": label}))
            }
            Some("blocked") => Some(json!({"op": "block", "label": label,
                "on": on.unwrap_or_else(|| json!({"user": null})), "note": note.unwrap_or_else(|| json!(label)),
                "options": options.unwrap_or(Value::Null)})),
            _ => None,
        };
        if let Some(op) = engine_op {
            deferred.push(op);
            let state = match now.get(&label) {
                Some(TodoStateName::Running) => "running",
                Some(TodoStateName::Done) => "done",
                _ => "pending",
            };
            row.insert("state".to_owned(), json!(state));
        }
        let alone = json!({"op": "set", "todos": [Value::Object(row.clone())]});
        match super::tool::declared(tool.actor(), alone.as_object().unwrap_or(&Map::new())) {
            Ok(_) => rows.push(row),
            Err(error) => {
                let error = error.to_string();
                let error = error.strip_prefix("set todos[0]: ").unwrap_or(&error);
                conditions.push(format!("{label}: left out: {error}"));
            }
        }
    }
    let mut set = args.clone();
    for _ in 0..REPAIRS {
        set.insert(
            "todos".to_owned(),
            rows.iter().cloned().map(Value::Object).collect(),
        );
        let issue = match tool.apply_one(&set) {
            Ok(_) => break,
            Err(PlanToolError::Op(PlanOpError::LabelNotUnique { label })) => {
                PlanIssue::DuplicateLabel { label }
            }
            Err(PlanToolError::Op(PlanOpError::Invalid { issue })) => issue,
            Err(error) => return Err(error),
        };
        conditions.push(repair(&mut rows, &issue).ok_or(PlanOpError::Invalid { issue })?);
    }
    for op in &deferred {
        let label = op.get("label").and_then(Value::as_str).unwrap_or_default();
        if let Err(error) = tool.apply_one(op.as_object().unwrap_or(&Map::new())) {
            conditions.push(format!("{label}: {error}"));
        }
    }
    let mut text = tool.apply_one(json!({"op": "view"}).as_object().unwrap_or(&Map::new()))?;
    for condition in conditions {
        text.push_str(&format!("\nnote: {condition}"));
    }
    Ok(text)
}

/// Each row's state in the open plan, by label; a row the engine moves keeps it until it moves.
fn held(tool: &PlanTool, args: &Map<String, Value>) -> HashMap<String, TodoStateName> {
    let plan = args
        .get("plan")
        .and_then(|plan| serde_json::from_value(plan.clone()).ok());
    let view = OpRequest {
        plan,
        actor: tool.actor().clone(),
        op: Op::View { full: true },
        request_id: None,
        expected_revision: None,
    };
    let todos = tool
        .engine()
        .apply(view)
        .map(|seen| seen.plan.todos)
        .unwrap_or_default();
    (todos.iter())
        .map(|todo| {
            (
                todo.label.as_str().to_owned(),
                TodoStateName::of(&todo.state),
            )
        })
        .collect()
}

/// The smallest change that answers the engine's issue, named as the row's condition.
fn repair(rows: &mut Vec<Map<String, Value>>, issue: &PlanIssue) -> Option<String> {
    let label_of =
        |row: &Map<String, Value>| row.get("label").and_then(Value::as_str).map(str::to_owned);
    let at = |rows: &[Map<String, Value>], label: &str| {
        rows.iter()
            .rposition(|row| label_of(row).as_deref() == Some(label))
    };
    let unlink = |row: &mut Map<String, Value>, after: &str| {
        if let Some(Value::Array(edges)) = row.get_mut("after") {
            edges.retain(|edge| edge.as_str() != Some(after));
        }
    };
    match issue {
        PlanIssue::Cycle { labels } => {
            let names: Vec<&str> = labels.iter().map(|label| label.as_str()).collect();
            let (index, after) = rows.iter().enumerate().rev().find_map(|(index, row)| {
                let edges = row.get("after")?.as_array()?;
                let closing = edges
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|edge| names.contains(edge))?;
                names
                    .contains(&label_of(row)?.as_str())
                    .then(|| (index, closing.to_owned()))
            })?;
            let row = rows.get_mut(index)?;
            unlink(row, &after);
            let own = label_of(row).unwrap_or_default();
            Some(format!(
                "{own}: after {after:?} left out: it would close a cycle through {}",
                names.join(", ")
            ))
        }
        PlanIssue::UnresolvedEdge { todo, after } => {
            let index = at(rows, todo.as_str())?;
            unlink(rows.get_mut(index)?, after.as_str());
            Some(format!(
                "{}: after {:?} left out: no todo has that label",
                todo.as_str(),
                after.as_str()
            ))
        }
        PlanIssue::DuplicateLabel { label: second } | PlanIssue::SlugCollision { second, .. } => {
            rows.remove(at(rows, second.as_str())?);
            Some(format!(
                "{}: a second row with this name was left out",
                second.as_str()
            ))
        }
        PlanIssue::Contract { label, issue } => {
            let index = at(rows, label.as_str())?;
            rows.get_mut(index)?.remove("contract")?;
            Some(format!("{}: contract left out: {issue}", label.as_str()))
        }
        PlanIssue::Unanswered { .. } => None,
    }
}

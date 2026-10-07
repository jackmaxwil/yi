//! Whole-plan apply: `set` with rows sends the plan as it should be, and every row the engine
//! cannot reach is a condition on that row in the reply, never a refusal of the call.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use yi_types::plan::doc::{PlanId, PlanIssue, Todo, TodoLabel, TodoStateName};

use super::declare::Blob;
use super::ops::{Op, OpRequest, Outcome, PlanOpError, SetRow, TodoSpec};
use super::table::OpKind;
use super::tool::{ArgError, PlanTool, PlanToolError, opt, todo_specs, view_request};

/// A plan the repairs cannot settle in this many passes is refused as it stands.
const REPAIRS: usize = 32;

/// The call as a whole-plan apply when it is one: `set` with a row that carries more than a
/// label, a contract, edges, a delegation or a state. A string of rows is read first.
pub(super) fn whole(args: &Map<String, Value>) -> Option<(Map<String, Value>, Vec<String>)> {
    let (mut args, mut said) = (args.clone(), Vec::new());
    let text = args.get("todos").and_then(Value::as_str);
    if let Some(rows) = text.and_then(|text| super::natural::rows_text(text, &mut said)) {
        args.insert("todos".to_owned(), rows);
    }
    super::natural::cut_labels(&mut args, &mut said);
    let rich =
        |row: &Value| (row.as_object()).is_some_and(|row| row.keys().any(|key| key != "label"));
    let rows = args.get("todos").and_then(Value::as_array);
    (args.get("op").and_then(Value::as_str) == Some("set")
        && rows.is_some_and(|rows| rows.iter().any(rich)))
    .then_some((args, said))
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
    let (now, mut rows, mut conditions) = (held(tool, args), Vec::new(), Vec::new());
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
        let mut moves = None;
        match now.get(&label) {
            Some(TodoStateName::Blocked) => {
                conditions.push(format!("{label}: still blocked on the user; left as is"));
                row.insert("state".to_owned(), json!("blocked"));
            }
            _ => {
                if let Some(mut op) = engine_op {
                    if let (Some(map), Some(plan)) = (op.as_object_mut(), args.get("plan")) {
                        map.insert("plan".to_owned(), plan.clone());
                    }
                    moves = Some(op);
                    let state = match now.get(&label) {
                        Some(TodoStateName::Running) => "running",
                        Some(TodoStateName::Done) => "done",
                        _ => "pending",
                    };
                    row.insert("state".to_owned(), json!(state));
                }
            }
        }
        let alone = json!({"op": "set", "todos": [Value::Object(row.clone())]});
        let (alone, _) = super::natural::natural(alone.as_object().unwrap_or(&Map::new()));
        match super::tool::declared(tool.actor(), &alone) {
            Ok(_) => rows.push((row, moves)),
            Err(error) => {
                let error = error.to_string();
                let error = error.strip_prefix("set todos[0]: ").unwrap_or(&error);
                conditions.push(format!("{label}: left out: {error}"));
            }
        }
    }
    if rows.is_empty() {
        return Err(ArgError::Declared(format!(
            "every row was left out\n{}",
            conditions.join("\n")
        ))
        .into());
    }
    let mut set = args.clone();
    if set.remove("list").is_some() {
        conditions.push(
            "the `list` checklist was left out: the `todos` rows state the whole plan".to_owned(),
        );
    }
    let mut open = None;
    for _ in 0..REPAIRS {
        set.insert(
            "todos".to_owned(),
            rows.iter()
                .map(|(row, _)| Value::Object(row.clone()))
                .collect(),
        );
        let issue = match tool.apply(&set) {
            Ok(_) => {
                open = None;
                break;
            }
            Err(PlanToolError::Op(PlanOpError::LabelNotUnique { label })) => {
                PlanIssue::DuplicateLabel { label }
            }
            Err(PlanToolError::Op(PlanOpError::Invalid { issue })) => issue,
            Err(error) => return Err(error),
        };
        let Some(note) = repair(&mut rows, &issue) else {
            return Err(PlanOpError::Invalid { issue }.into());
        };
        conditions.push(note);
        open = Some(issue);
    }
    if let Some(issue) = open {
        return Err(PlanOpError::Invalid { issue }.into());
    }
    for op in rows.iter().filter_map(|(_, moves)| moves.as_ref()) {
        let label = op.get("label").and_then(Value::as_str).unwrap_or_default();
        if let Err(error) = tool.apply(op.as_object().unwrap_or(&Map::new())) {
            conditions.push(format!("{label}: {error}"));
        }
    }
    let mut view = Map::new();
    view.insert("op".to_owned(), json!("view"));
    if let Some(plan) = args.get("plan") {
        view.insert("plan".to_owned(), plan.clone());
    }
    let mut text = tool.apply(&view)?;
    for condition in conditions {
        text.push_str(&format!("\nnote: {condition}"));
    }
    Ok(text)
}

/// Each row's state in the open plan, by label; a row the engine moves keeps it until it moves.
fn held(tool: &PlanTool, args: &Map<String, Value>) -> HashMap<String, TodoStateName> {
    let plan: Option<PlanId> = args
        .get("plan")
        .and_then(|plan| serde_json::from_value(plan.clone()).ok());
    let todos = tool
        .engine()
        .apply(view_request(tool.actor(), plan))
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

/// The smallest change that answers the engine's issue, named as the row's condition; a row
/// removed takes its deferred engine move with it.
fn repair(
    rows: &mut Vec<(Map<String, Value>, Option<Value>)>,
    issue: &PlanIssue,
) -> Option<String> {
    let label_of =
        |row: &Map<String, Value>| row.get("label").and_then(Value::as_str).map(str::to_owned);
    let at = |rows: &[(Map<String, Value>, Option<Value>)], label: &str| {
        rows.iter()
            .rposition(|(row, _)| label_of(row).as_deref() == Some(label))
    };
    let unlink = |row: &mut Map<String, Value>, after: &str| {
        if let Some(Value::Array(edges)) = row.get_mut("after") {
            edges.retain(|edge| edge.as_str() != Some(after));
        }
    };
    match issue {
        PlanIssue::Cycle { labels } => {
            let names: Vec<&str> = labels.iter().map(|label| label.as_str()).collect();
            let (index, after) = rows
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, (row, _))| {
                    let edges = row.get("after")?.as_array()?;
                    let closing = edges
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|edge| names.contains(edge))?;
                    names
                        .contains(&label_of(row)?.as_str())
                        .then(|| (index, closing.to_owned()))
                })?;
            let (row, _) = rows.get_mut(index)?;
            unlink(row, &after);
            let own = label_of(row).unwrap_or_default();
            Some(format!(
                "{own}: after {after:?} left out: it would close a cycle through {}",
                names.join(", ")
            ))
        }
        PlanIssue::UnresolvedEdge { todo, after } => {
            let index = at(rows, todo.as_str())?;
            unlink(&mut rows.get_mut(index)?.0, after.as_str());
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
            rows.get_mut(index)?.0.remove("contract")?;
            Some(format!("{}: contract left out: {issue}", label.as_str()))
        }
        PlanIssue::Unanswered { .. } => None,
    }
}

/// `init` while a plan is open adds the todos it does not hold to that plan; the goal stays.
pub(super) fn init_on_open(
    tool: &PlanTool,
    request: &OpRequest,
    id: PlanId,
    blobs: &[Blob],
    said: &mut Vec<String>,
) -> Result<Outcome, PlanToolError> {
    let Op::Init { todos, .. } = &request.op else {
        return Err(PlanOpError::PlanExists { id }.into());
    };
    let on = |op| OpRequest {
        plan: Some(id.clone()),
        op,
        ..request.clone()
    };
    let held = tool.engine().apply(on(Op::View { full: true }))?.plan.todos;
    let (old, new): (Vec<TodoSpec>, Vec<TodoSpec>) =
        (todos.iter().cloned()).partition(|todo| held.iter().any(|row| row.label == todo.label));
    said.push(format!(
        "plan {id} was open, so init added its new todos to it; its goal stays"
    ));
    if !old.is_empty() {
        let names: Vec<&str> = old.iter().map(|todo| todo.label.as_str()).collect();
        said.push(format!("{} were in it already", names.join(", ")));
    }
    Ok(if new.is_empty() {
        tool.engine().apply(on(Op::View { full: false }))?
    } else {
        tool.engine()
            .apply_with(on(Op::Append { todos: new }), blobs)?
    })
}

/// A `reorder` naming some of the todos puts those first and keeps the rest in their order.
pub(super) fn reorder_rest(
    tool: &PlanTool,
    request: OpRequest,
    blobs: &[Blob],
    said: &mut Vec<String>,
) -> Option<Result<Outcome, PlanToolError>> {
    let Op::Reorder { labels } = &request.op else {
        return None;
    };
    let view = view_request(tool.actor(), request.plan.clone());
    let held = match tool.engine().apply(view) {
        Ok(seen) => seen.plan.todos,
        Err(error) => return Some(Err(error.into())),
    };
    let mut order = labels.clone();
    order.extend(
        (held.iter())
            .map(|todo| todo.label.clone())
            .filter(|label| !labels.contains(label)),
    );
    said.push("the todos not named keep their order after the named ones".to_owned());
    let reorder = OpRequest {
        op: Op::Reorder { labels: order },
        ..request
    };
    Some(tool.engine().apply_with(reorder, blobs).map_err(Into::into))
}

/// The open plan's todos, or those of the plan named.
fn todos_of(tool: &PlanTool, plan: Option<PlanId>) -> Vec<Todo> {
    let view = view_request(tool.actor(), plan);
    (tool.engine().apply(view)).map_or_else(|_| Vec::new(), |seen| seen.plan.todos)
}

/// `decompose` naming no todo splits the one todo running in the plan, when there is one.
pub(super) fn decompose_target(
    tool: &PlanTool,
    args: &mut Map<String, Value>,
    said: &mut Vec<String>,
) {
    let named = ["label", "todo"].iter().any(|key| args.contains_key(*key));
    if named || args.get("op").and_then(Value::as_str) != Some("decompose") {
        return;
    }
    let plan = args
        .get("plan")
        .and_then(|plan| serde_json::from_value(plan.clone()).ok());
    let todos = todos_of(tool, plan);
    let running: Vec<&Todo> = (todos.iter())
        .filter(|todo| TodoStateName::of(&todo.state) == TodoStateName::Running)
        .collect();
    if let [one] = running.as_slice() {
        said.push(format!(
            "decompose named no todo; {} is the one running, so it was split",
            one.label
        ));
        args.insert("label".to_owned(), json!(one.label.as_str()));
    }
}

/// A call on a label the open plan lacks runs on the one sub-plan that holds it.
pub(super) fn in_holder(
    tool: &PlanTool,
    args: &Map<String, Value>,
    plan: PlanId,
    label: TodoLabel,
    mut said: Vec<String>,
) -> Result<String, PlanToolError> {
    let sub = holder(tool, &label).ok_or(PlanOpError::UnknownLabel { plan, label })?;
    said.push(format!(
        "that todo is in sub-plan {sub}, so the call ran there"
    ));
    let mut again = args.clone();
    again.insert("plan".to_owned(), json!(sub.as_str()));
    Ok(noted(tool.run(&again)?, said))
}

fn holder(tool: &PlanTool, label: &TodoLabel) -> Option<PlanId> {
    let mut queue: Vec<PlanId> = (todos_of(tool, None).iter())
        .filter_map(|todo| todo.subplan.clone())
        .collect();
    let mut found = Vec::new();
    while let Some(plan) = queue.pop() {
        let todos = todos_of(tool, Some(plan.clone()));
        if todos.iter().any(|todo| todo.label == *label) {
            found.push(plan);
        }
        queue.extend(todos.into_iter().filter_map(|todo| todo.subplan));
    }
    match found.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// `block` on a failed todo retries it first, since a terminal todo waits on nothing.
pub(super) fn block_failed(
    tool: &PlanTool,
    request: &OpRequest,
    blobs: &[Blob],
    said: &mut Vec<String>,
) -> Option<Result<Outcome, PlanToolError>> {
    let Op::Block { label, .. } = &request.op else {
        return None;
    };
    let failed = (todos_of(tool, request.plan.clone()).iter()).any(|todo| {
        todo.label == *label && TodoStateName::of(&todo.state) == TodoStateName::Failed
    });
    if !failed {
        return None;
    }
    let retry = OpRequest {
        op: Op::Retry {
            label: label.clone(),
            delegation: None,
        },
        ..request.clone()
    };
    said.push("it had failed, so it was retried, then blocked".to_owned());
    Some(
        (tool.engine().apply(retry))
            .and_then(|_| tool.engine().apply_with(request.clone(), blobs))
            .map_err(Into::into),
    )
}

/// The reply with each note on its own `note:` line.
pub(super) fn noted(mut text: String, said: Vec<String>) -> String {
    for line in said {
        text.push_str(&format!("\nnote: {line}"));
    }
    text
}

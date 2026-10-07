//! The shapes models send for a plan call the tool understands, made canonical before the strict
//! parse; every case here was a refused call in the dogfood sessions of #982.

use std::borrow::Cow;

use serde_json::{Map, Value, json};
use yi_types::plan::doc::TODO_LABEL_MAX;
use yi_types::plan::op::{ALL_OPS, MODEL_OPS, op_name};

use super::table::OpKind;
use super::tool::{ArgError, PlanToolError, TODO_SPEC_KEYS, field_hint, known_keys, misplaced};

/// A string of rows needing more mending than this is refused as it was sent.
const ROWS_MENDED: usize = 16;

/// Keys an op does not record that models attach to explain themselves; the call's arguments in
/// the transcript keep them, and the reply says so.
const ANNOTATIONS: [&str; 5] = ["reason", "note", "evidence", "why", "explanation"];

/// Ops that act on one todo, so a bare string under the op's own key, or one of several `labels`,
/// is that todo.
pub(super) const TARGETED: [&str; 10] = [
    "start",
    "done",
    "fail",
    "retry",
    "drop",
    "block",
    "unblock",
    "decompose",
    "add_edge",
    "accepted_by_user",
];

pub(super) fn natural(args: &Map<String, Value>) -> (Map<String, Value>, Vec<String>) {
    let mut args = unleak(&unstring_todos(args));
    let mut said = Vec::new();
    infer_op(&mut args);
    let rows =
        (args.get("todos").and_then(Value::as_str)).and_then(|text| rows_text(text, &mut said));
    if let Some(rows) = rows {
        args.insert("todos".to_owned(), rows);
    }
    let op = args
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    unsent_nulls(&mut args);
    fold_row(&mut args, &mut said);
    labels(&op, &mut args);
    cut_labels(&mut args, &mut said);
    let rowless = ["set", "supersede", "decompose", "append"].contains(&op.as_str())
        && !args.contains_key("list")
        && !args.contains_key("todos");
    let op = if rowless {
        args.retain(|key, _| ["plan", "full"].contains(&key.as_str()));
        args.insert("op".to_owned(), json!("view"));
        said.push(format!(
            "{op} takes its rows in todos and none came, so nothing changed; this is the plan as it stands"
        ));
        "view".to_owned()
    } else {
        op
    };
    if let ("init" | "append", Some(shared)) = (op.as_str(), args.remove("delegation"))
        && let Some(Value::Array(todos)) = args.get_mut("todos")
    {
        for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
            todo.entry("delegation").or_insert_with(|| shared.clone());
        }
        said.push("the delegation given beside todos went to each todo without one".to_owned());
    }
    if op == "block" && !args.contains_key("note") {
        let asked = args.remove("question").or_else(|| {
            said.push(
                "block needs a note for whoever unblocks it; the todo's label stands in".to_owned(),
            );
            args.get("label").or_else(|| args.get("todo")).cloned()
        });
        if let Some(note) = asked {
            args.insert("note".to_owned(), note);
        }
    }
    if let Some(Value::String(said_how)) = args.get("disposition")
        && !["retained", "discarded"].contains(&said_how.as_str())
    {
        args.remove("disposition");
        said.push(
            "disposition is retained or discarded; the text given stays in this call's arguments"
                .to_owned(),
        );
    }
    if op == "supersede" && !args.contains_key("reason") {
        args.insert("reason".to_owned(), json!("none given"));
        said.push("supersede records a reason and none was given".to_owned());
    }
    match op.as_str() {
        "set" | "supersede" | "append" => checklist(&op, &mut args, &mut said),
        "init" if !args.contains_key("goal") => {
            if let Some(goal) = first_label(&args) {
                args.insert("goal".to_owned(), json!(goal));
                said.push("no goal was given, so the first todo names the plan".to_owned());
            }
        }
        "fail" if !args.contains_key("cause") => {
            if let Some(reason) = args.remove("reason") {
                args.insert("cause".to_owned(), reason);
            }
        }
        "done" => {
            let url = args
                .get("output")
                .and_then(Value::as_str)
                .is_some_and(|out| out.contains("://") && !out.contains(char::is_whitespace));
            if !url && args.remove("output").is_some() {
                said.push(
                    "output takes a url; that text stays in this call's arguments".to_owned(),
                );
            }
        }
        _ => {}
    }
    let kept: &[&str] = match op.as_str() {
        "block" | "accepted_by_user" | "accept" => &["note"],
        "supersede" => &["reason"],
        _ => &[],
    };
    let mut left: Vec<String> = (ANNOTATIONS.iter())
        .filter(|key| !kept.contains(key) && args.remove(**key).is_some())
        .map(|key| (*key).to_owned())
        .collect();
    unknown(&op, &mut args, &mut left);
    if !left.is_empty() {
        said.push(format!(
            "{op} reads no {}; left out, they stay in this call's arguments",
            left.join(", ")
        ));
    }
    weights(&mut args, &mut said);
    (args, said)
}

/// Invariant: a key the op does not read is left out and named, unless it is one edit from a key
/// the tool reads or carries a hint: a misspelled `contract` must refuse, never land unverified.
fn unknown(op: &str, args: &mut Map<String, Value>, left: &mut Vec<String>) {
    let Some(kind) = ALL_OPS.into_iter().find(|kind| op_name(*kind) == op) else {
        return;
    };
    let every: Vec<&str> = (ALL_OPS.into_iter().flat_map(known_keys))
        .copied()
        .chain(TODO_SPEC_KEYS)
        .collect();
    let set_state = |key: &str, in_todo: bool| in_todo && key == "state" && kind == OpKind::Set;
    let loose = |key: &str, legal: &[&str], in_todo: bool| {
        let kept = legal.contains(&key) || ["actor", "labels"].contains(&key);
        !(kept || set_state(key, in_todo))
            && misplaced(kind, in_todo, key).is_none()
            && field_hint(key).is_empty()
            && (every.contains(&key)
                || !every
                    .iter()
                    .any(|known| crate::memory::edit_distance(key, known) <= 1))
    };
    args.retain(|key, _| {
        let drop = loose(key, known_keys(kind), false);
        if drop {
            left.push(key.clone());
        }
        !drop
    });
    if let Some(Value::Array(todos)) = args.get_mut("todos") {
        for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
            if !todo.contains_key("delegation")
                && let Some(delegation) = todo.remove("delegate")
            {
                todo.insert("delegation".to_owned(), delegation);
            }
            todo.retain(|key, _| {
                let drop = loose(key, &TODO_SPEC_KEYS, true);
                if drop && !left.contains(key) {
                    left.push(format!("todos[].{key}"));
                }
                !drop
            });
        }
    }
}

/// Incident: validate read a todo's on and a targeted op's labels as unknown keys, which the tool's
/// run splits into calls of their own, so the loop refused what execute lands. These are those calls.
pub(super) fn calls(args: &Map<String, Value>) -> Vec<Map<String, Value>> {
    let mut args = args.clone();
    let blocks = blocks(&mut args);
    let (args, _) = natural(&args);
    let targeted =
        (args.get("op").and_then(Value::as_str)).is_some_and(|op| TARGETED.contains(&op));
    let mut out = match args.get("labels") {
        Some(Value::Array(labels)) if targeted => labels
            .iter()
            .flat_map(|label| {
                let mut one = args.clone();
                one.remove("labels");
                one.insert("todo".to_owned(), label.clone());
                calls(&one)
            })
            .collect(),
        _ => vec![args.clone()],
    };
    out.extend(blocks.iter().flat_map(calls));
    out
}

/// A todo written with `on` is blocked by its own call once the todo exists.
pub(super) fn blocks(args: &mut Map<String, Value>) -> Vec<Map<String, Value>> {
    let Some(Value::Array(todos)) = args.get_mut("todos") else {
        return Vec::new();
    };
    let block = |todo: &mut Map<String, Value>| {
        let on = todo.remove("on")?;
        let label = todo.get("label").cloned()?;
        let mut block = Map::from_iter([
            ("op".to_owned(), json!("block")),
            ("label".to_owned(), label),
        ]);
        block.insert("on".to_owned(), on);
        for key in ["note", "options"] {
            if let Some(value) = todo.remove(key) {
                block.insert(key.to_owned(), value);
            }
        }
        Some(block)
    };
    todos
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .filter_map(block)
        .collect()
}

/// Incident: GLM's call markup leaked into a key, `{"block<arg_key>note": …}`, or a value,
/// `{"done": "evidence</arg_key><arg_value>…"}`; the field reads back and the op is the leaked name.
fn unleak(args: &Map<String, Value>) -> Map<String, Value> {
    let leaked = args
        .iter()
        .find_map(|(key, value)| match key.split_once("<arg_key>") {
            Some((op, _)) => Some(op),
            None => (value.as_str()?.contains("</arg_key><arg_value>")).then_some(key.as_str()),
        });
    let leaked = leaked.filter(|op| ALL_OPS.iter().any(|kind| op_name(*kind) == *op));
    let mut fixed = crate::todo::tool::unleak(args).into_owned();
    if let Some(op) = leaked {
        fixed.entry("op").or_insert_with(|| json!(op));
    }
    fixed
}

/// A string of rows that parses is the rows. Incident: glm-5.3-flash stringified rich rows and
/// miscounted their braces, closing each contract after its label; each miscount is mended.
pub(super) fn rows_text(text: &str, said: &mut Vec<String>) -> Option<Value> {
    let mut text = text.to_owned();
    let mut mended = false;
    for _ in 0..ROWS_MENDED {
        let mut stream = serde_json::Deserializer::from_str(&text).into_iter::<Value>();
        let error = match stream.next()? {
            Ok(Value::Array(mut rows)) => {
                let rest = text.get(stream.byte_offset()..).unwrap_or_default();
                let closer = |c: char| c.is_whitespace() || c == '}' || c == ']';
                if !rest.trim_matches(closer).is_empty() {
                    return None;
                }
                if mended || !rest.trim().is_empty() {
                    said.push("the todos string's braces did not balance; they were mended where each row opens and closes".to_owned());
                    rows.iter_mut().for_each(|row| lift_row(row, said));
                }
                return Some(Value::Array(rows));
            }
            Ok(_) => return None,
            Err(error) => error,
        };
        let line: usize = (text.split_inclusive('\n'))
            .take(error.line().saturating_sub(1))
            .map(str::len)
            .sum();
        let at = line.saturating_add(error.column().saturating_sub(1));
        let (head, tail) = (text.get(..at)?, text.get(at..)?);
        text = match tail.chars().next()? {
            '{' => {
                let comma = head.rfind(',')?;
                format!("{}}}{}", text.get(..comma)?, text.get(comma..)?)
            }
            ']' => format!("{head}}}{tail}"),
            '}' => format!("{head}{}", tail.get(1..)?),
            _ => return None,
        };
        mended = true;
    }
    None
}

/// A row whose label sits inside its contract, which a row left open puts there, gets it back,
/// with every other row field written there beside it.
fn lift_row(row: &mut Value, said: &mut Vec<String>) {
    const CONTRACT_KEYS: [&str; 5] = ["class", "items", "covers", "threshold", "min_coverage"];
    let Some(row) = row.as_object_mut() else {
        return;
    };
    let Some(Value::Object(contract)) = row.get_mut("contract") else {
        return;
    };
    if !contract.contains_key("label") {
        return;
    }
    let moved: Vec<String> = (contract.keys())
        .filter(|key| !CONTRACT_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    let fields: Vec<(String, Value)> = (moved.iter())
        .filter_map(|key| Some((key.clone(), contract.remove(key)?)))
        .collect();
    for (key, value) in fields {
        row.entry(key).or_insert(value);
    }
    said.push(format!(
        "{} moved out of the contract to the row",
        moved.join(", ")
    ));
}

/// A key sent as null is a key not sent, in the call and in each of its rows.
fn unsent_nulls(args: &mut Map<String, Value>) {
    args.retain(|_, value| !value.is_null());
    let rows = args.get_mut("todos").and_then(Value::as_array_mut);
    for row in rows.into_iter().flatten().filter_map(Value::as_object_mut) {
        row.retain(|_, value| !value.is_null());
    }
}

/// A label past the cap is cut to it wherever it is named, so the todo and a later call that
/// names it in full meet at the same label.
pub(super) fn cut_labels(args: &mut Map<String, Value>, said: &mut Vec<String>) {
    let mut cut = |value: &mut Value| {
        let Some(text) = value.as_str() else {
            return;
        };
        let chars = text.chars().count();
        if chars > TODO_LABEL_MAX {
            let kept: String = text.chars().take(TODO_LABEL_MAX).collect();
            said.push(format!(
                "a label of {chars} chars was cut to {TODO_LABEL_MAX}, the label cap: {kept}; a call may name it in full"
            ));
            *value = json!(kept);
        }
    };
    let mut names = |value: &mut Value| match value {
        Value::Array(items) => items.iter_mut().for_each(&mut cut),
        other => cut(other),
    };
    for key in ["label", "todo", "after", "labels"] {
        if let Some(value) = args.get_mut(key) {
            names(value);
        }
    }
    let rows = args.get_mut("todos").and_then(Value::as_array_mut);
    for row in rows.into_iter().flatten().filter_map(Value::as_object_mut) {
        for key in ["label", "after"] {
            if let Some(value) = row.get_mut(key) {
                names(value);
            }
        }
    }
}

/// `goal` and `todos` open a plan and `list` sets one; a key named after an op, as a flag, holding
/// the arguments or naming the todo, is that op.
fn infer_op(args: &mut Map<String, Value>) {
    let given = args.get("op").and_then(Value::as_str).map(str::to_owned);
    let named = given.or_else(|| {
        let mut ops = ALL_OPS.iter().take(MODEL_OPS).map(|op| op_name(*op));
        ops.find(|name| args.contains_key(*name)).map(str::to_owned)
    });
    let op = match named {
        Some(name) => {
            match args.remove(&name) {
                Some(Value::Object(inner)) => {
                    for (key, value) in inner {
                        args.entry(key).or_insert(value);
                    }
                }
                Some(target @ Value::String(_))
                    if TARGETED.contains(&name.as_str())
                        && !args.contains_key("label")
                        && !args.contains_key("todo") =>
                {
                    args.insert("todo".to_owned(), target);
                }
                _ => {}
            }
            name
        }
        None if args.contains_key("list") => "set".to_owned(),
        None if args.contains_key("goal")
            && (args.contains_key("todos") || args.contains_key("label")) =>
        {
            "init".to_owned()
        }
        None if args.contains_key("todos") => "append".to_owned(),
        None if args.keys().all(|key| key == "plan") => "view".to_owned(),
        None => return,
    };
    args.insert("op".to_owned(), json!(op));
}

/// `labels` lists rows for an op that takes rows, and the todo for one that takes a single todo;
/// several on such an op are run one call each by the tool.
fn labels(op: &str, args: &mut Map<String, Value>) {
    let rows = ["init", "append", "set", "supersede"].contains(&op);
    if rows
        && !args.contains_key("todos")
        && let Some(label @ Value::String(_)) = args.remove("label")
    {
        args.insert("labels".to_owned(), json!([label]));
    }
    let Some(Value::Array(labels)) = args.get("labels") else {
        return;
    };
    if rows && !args.contains_key("todos") && !args.contains_key("list") {
        let todos: Vec<Value> = labels
            .iter()
            .map(|label| json!({ "label": label }))
            .collect();
        args.remove("labels");
        args.insert("todos".to_owned(), Value::Array(todos));
    } else if let [one] = labels.as_slice()
        && TARGETED.contains(&op)
    {
        let one = one.clone();
        args.remove("labels");
        args.entry("todo").or_insert(one);
    }
}

/// `set` takes rows as `todos` or `list`; `supersede` takes them as `list`. A line that is not a
/// row leaves the list, as a struck row (`- [-]`, `- [~]`) does; a box set cannot read is unchecked.
fn checklist(op: &str, args: &mut Map<String, Value>, said: &mut Vec<String>) {
    let rows: Vec<String> = match (op, args.remove("list"), args.remove("todos")) {
        ("set", Some(Value::String(list)), todos) => {
            if let Some(todos) = todos {
                args.insert("todos".to_owned(), todos);
            }
            list.lines().map(str::to_owned).collect()
        }
        ("set", None, Some(Value::Array(rows))) => {
            let bare = |row: &Value| {
                let label = row.get("label").is_some_and(Value::is_string);
                row.is_string() || (label && row.as_object().is_some_and(|o| o.len() == 1))
            };
            // Invariant: a row carrying a contract, edges or a delegation is no checklist line, and
            // set refuses it by its `todos` hint rather than land the todo unverified.
            if rows.is_empty() || !rows.iter().all(bare) {
                args.insert("todos".to_owned(), Value::Array(rows));
                return;
            }
            rows.iter().filter_map(row_text).collect()
        }
        ("supersede" | "append", Some(Value::String(list)), todos) => {
            let labels: Vec<Value> = list
                .lines()
                .filter_map(|line| line.trim_start().strip_prefix("- ["))
                .filter_map(|line| line.get(2..).map(str::trim))
                .filter(|label| !label.is_empty())
                .map(|label| json!({ "label": label }))
                .collect();
            args.insert("todos".to_owned(), todos.unwrap_or(Value::Array(labels)));
            return;
        }
        (_, list, todos) => {
            for (key, value) in [("list", list), ("todos", todos)] {
                if let Some(value) = value {
                    args.insert(key.to_owned(), value);
                }
            }
            return;
        }
    };
    let struck = |line: &str| {
        ["- [-]", "- [~]"]
            .iter()
            .any(|mark| line.trim_start().starts_with(mark))
    };
    if rows.iter().any(|line| struck(line)) {
        said.push("struck rows (- [-]) leave the list, as an omitted row does".to_owned());
    }
    let mut list = Vec::new();
    for line in rows.iter().filter(|line| !struck(line)) {
        let body = line.trim_start();
        let indent = line
            .get(..line.len().saturating_sub(body.len()))
            .unwrap_or_default();
        match body
            .strip_prefix("- [")
            .and_then(|rest| rest.split_at_checked(1))
        {
            Some((" " | ">" | "x" | "X", _)) => list.push(line.clone()),
            Some((_, rest)) if rest.starts_with(']') => {
                said.push("a box set does not read, such as [!], was kept unchecked".to_owned());
                list.push(format!("{indent}- [ {rest}"));
            }
            _ if body.is_empty() || body.starts_with('#') => {}
            _ => said.push("lines that are not checklist rows were left out".to_owned()),
        }
    }
    args.insert("list".to_owned(), json!(list.join("\n")));
}

/// A checklist row from a string or a `{label}` object, unchecked unless it carries a box.
fn row_text(row: &Value) -> Option<String> {
    let text = row
        .as_str()
        .or_else(|| row.get("label").and_then(Value::as_str))?;
    Some(if text.trim_start().starts_with("- [") {
        text.to_owned()
    } else {
        format!("- [ ] {text}")
    })
}

fn first_label(args: &Map<String, Value>) -> Option<String> {
    let labels: Vec<&str> = (args
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten())
    .filter_map(|todo| todo.get("label").and_then(Value::as_str))
    .collect();
    let first = labels.first()?;
    Some(match labels.len() {
        1 => (*first).to_owned(),
        more => format!("{first}, and {} more", more.saturating_sub(1)),
    })
}

/// A todo's fields written beside the goal are that todo's row, unless a row already holds it.
fn fold_row(args: &mut Map<String, Value>, said: &mut Vec<String>) {
    let rows =
        ["init", "append"].contains(&args.get("op").and_then(Value::as_str).unwrap_or_default());
    let label = args.get("label").and_then(Value::as_str).map(str::to_owned);
    if let (true, Some(label)) = (rows, label) {
        let keys = [
            "label",
            "after",
            "contract",
            "delegation",
            "intent",
            "waived",
            "state",
        ];
        let row: Map<String, Value> = (keys.iter())
            .filter_map(|key| Some(((*key).to_owned(), args.remove(*key)?)))
            .collect();
        let todos = args.entry("todos").or_insert_with(|| json!([]));
        let repeated = (todos.as_array()).is_some_and(|rows| {
            rows.iter()
                .any(|row| row.get("label").and_then(Value::as_str) == Some(&label))
        });
        match todos.as_array_mut() {
            Some(rows) if !repeated => {
                rows.insert(0, Value::Object(row));
                said.push("a todo's fields beside the goal are its one row in todos".to_owned());
            }
            _ => said
                .push("a todo's fields beside the goal repeat a row in todos; left out".to_owned()),
        }
    }
}

/// The delegation shapes models send: a context path is a `local://` address, `stated: true`
/// states the todo's label, and an inline output schema has no url, so it is left out.
fn delegation_shape(
    delegation: &mut Map<String, Value>,
    label: Option<&str>,
    said: &mut Vec<String>,
) {
    if let Some(Value::Array(context)) = delegation.get_mut("context") {
        for entry in context.iter_mut() {
            if let Some(path) = entry.as_str().filter(|path| !path.contains("://")) {
                *entry = json!(format!("local://{path}"));
            }
        }
    }
    if let (Some(stated), Some(label)) = (
        delegation
            .get_mut("accept")
            .and_then(|accept| accept.get_mut("stated")),
        label,
    ) && !stated.is_string()
    {
        *stated = json!(label);
    }
    let inline = delegation
        .get("output")
        .and_then(|output| output.get("schema"));
    if inline.is_some_and(|schema| !schema.is_string()) {
        delegation.remove("output");
        said.push(
            "output.schema takes the url of a schema; the inline schema was left out".to_owned(),
        );
    }
}

/// A contract item's weight is a share from 1 to 100: a missing one is 1, and items weighted past
/// 100 are rescaled together so their ratios hold. An item with an empty decider checks nothing.
fn weights(args: &mut Map<String, Value>, said: &mut Vec<String>) {
    if let Some(Value::Object(delegation)) = args.get_mut("delegation") {
        delegation_shape(delegation, None, said);
    }
    let Some(Value::Array(todos)) = args.get_mut("todos") else {
        return;
    };
    for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
        let label = todo.get("label").and_then(Value::as_str).map(str::to_owned);
        if let Some(Value::Object(delegation)) = todo.get_mut("delegation") {
            delegation_shape(delegation, label.as_deref(), said);
        }
        if let Some(Value::Array(items)) = todo.get_mut("contract").and_then(|c| c.get_mut("items"))
        {
            for id in items.iter_mut().filter_map(|item| item.get_mut("id")) {
                if let Some(text) = id
                    .as_str()
                    .filter(|text| text.contains(char::is_whitespace))
                {
                    *id = json!(text.split_whitespace().collect::<Vec<_>>().join("-"));
                    said.push("a contract item id takes no spaces; they became dashes".to_owned());
                }
            }
        }
        if let Some(Value::Object(contract)) = todo.get_mut("contract")
            && !contract.contains_key("class")
        {
            contract.insert("class".to_owned(), json!("inline"));
            said.push("a contract without a class is inline, the class with no floor".to_owned());
        }
        let Some(Value::Array(items)) = todo.get_mut("contract").and_then(|c| c.get_mut("items"))
        else {
            continue;
        };
        let before = items.len();
        items.retain(|item| {
            item.get("decider")
                .is_some_and(|decider| decider.as_object().is_none_or(|keys| !keys.is_empty()))
        });
        if items.len() < before {
            said.push(
                "a contract item with an empty decider checks nothing and was left out".to_owned(),
            );
        }
        if before > 0 && items.is_empty() {
            todo.remove("contract");
            continue;
        }
        let max = items
            .iter()
            .filter_map(|item| item.get("weight").and_then(Value::as_u64))
            .max()
            .unwrap_or(1);
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            let weight = item.get("weight").and_then(Value::as_u64);
            let scaled = match weight {
                None => 1,
                Some(weight) if max > 100 => (weight.saturating_mul(100) / max).max(1),
                Some(weight) => weight,
            };
            if weight != Some(scaled) {
                item.insert("weight".to_owned(), json!(scaled));
            }
        }
        if max > 100 {
            said.push(
                "contract weights past 100 were rescaled to 1..100, keeping their ratios"
                    .to_owned(),
            );
        }
    }
}

impl super::tool::PlanTool {
    /// One call per label, in order; a refusal among them makes the whole reply an error.
    pub(super) fn each(
        &self,
        args: &Map<String, Value>,
        labels: &[Value],
        mut said: Vec<String>,
    ) -> Result<String, PlanToolError> {
        let (mut text, mut refused) = (String::new(), false);
        for label in labels {
            let mut one = args.clone();
            one.remove("labels");
            one.insert("todo".to_owned(), label.clone());
            let reply = self.run(&one).unwrap_or_else(|err| {
                refused = true;
                format!("{label}: refused: {err}")
            });
            text.push_str(&format!("{reply}\n"));
        }
        said.push("labels named several todos, so each ran as its own call, in order".to_owned());
        let mut text = text.trim_end().to_owned();
        for line in said {
            text.push_str(&format!("\nnote: {line}"));
        }
        let text = text.trim_end().to_owned();
        if refused {
            Err(ArgError::Declared(text).into())
        } else {
            Ok(text)
        }
    }
}

/// `todos` sent as a JSON string is the array it names, on every op that takes rows: the
/// whole-plan reading and the strict parse both see one spelling.
pub(super) fn unstring_todos(args: &Map<String, Value>) -> Cow<'_, Map<String, Value>> {
    if let Some(Value::String(text)) = args.get("todos")
        && let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(text)
    {
        let mut fixed = args.clone();
        fixed.insert("todos".to_owned(), parsed);
        return Cow::Owned(fixed);
    }
    Cow::Borrowed(args)
}

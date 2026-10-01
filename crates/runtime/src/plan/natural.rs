//! The shapes models send for a plan call the tool understands, made canonical before the strict
//! parse; every case here was a refused call in the dogfood sessions of #982.

use serde_json::{Map, Value, json};

use yi_types::plan::op::{ALL_OPS, MODEL_OPS, op_name};

use super::tool::{TODO_SPEC_KEYS, field_hint, known_keys, misplaced};

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
    let mut args = args.clone();
    let mut said = Vec::new();
    infer_op(&mut args);
    if let Some(Value::String(text)) = args.get("todos")
        && let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(text)
    {
        args.insert("todos".to_owned(), parsed);
    }
    let op = args
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    labels(&op, &mut args);
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
        "set" | "supersede" => checklist(&op, &mut args, &mut said),
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
    let loose = |key: &str, legal: &[&str], in_todo: bool| {
        !legal.contains(&key)
            && !["actor", "labels"].contains(&key)
            && misplaced(kind, in_todo, key).is_none()
            && field_hint(key).is_empty()
            && (every.contains(&key) || !every.iter().any(|known| one_edit(key, known)))
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

/// One insertion, deletion, substitution or swap of neighbours apart.
fn one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let same = short
        .iter()
        .zip(long.iter())
        .take_while(|(x, y)| x == y)
        .count();
    let (s, l) = (
        short.get(same..).unwrap_or_default(),
        long.get(same..).unwrap_or_default(),
    );
    match long.len().saturating_sub(short.len()) {
        0 => {
            s.get(1..) == l.get(1..)
                || (s.first() == l.get(1) && s.get(1) == l.first() && s.get(2..) == l.get(2..))
        }
        1 => l.get(1..) == Some(s),
        _ => false,
    }
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
        None if args.contains_key("todos") && args.contains_key("goal") => "init".to_owned(),
        None if args.contains_key("todos") => "append".to_owned(),
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
        ("set", None, Some(Value::Array(rows))) => rows.iter().filter_map(row_text).collect(),
        ("supersede", Some(Value::String(list)), todos) => {
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

/// A contract item's weight is a share from 1 to 100: a missing one is 1, and items weighted past
/// 100 are rescaled together so their ratios hold. An item with an empty decider checks nothing.
fn weights(args: &mut Map<String, Value>, said: &mut Vec<String>) {
    let Some(Value::Array(todos)) = args.get_mut("todos") else {
        return;
    };
    for todo in todos.iter_mut().filter_map(Value::as_object_mut) {
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

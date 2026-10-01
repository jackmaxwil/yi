//! The shapes models send for a plan call the tool understands, made canonical before the strict
//! parse; every case here was a refused call in the dogfood sessions of #982.

use serde_json::{Map, Value, json};

use yi_types::plan::op::{ALL_OPS, MODEL_OPS, op_name};

/// Keys an op does not record that models attach to explain themselves; the call's arguments in
/// the transcript keep them, and the reply says so.
const ANNOTATIONS: [&str; 5] = ["reason", "note", "evidence", "why", "explanation"];

pub(super) fn natural(args: &Map<String, Value>) -> (Map<String, Value>, Vec<&'static str>) {
    let mut args = args.clone();
    let mut said = Vec::new();
    if !args.contains_key("op") {
        infer_op(&mut args);
    }
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
    match op.as_str() {
        "set" | "supersede" => checklist(&op, &mut args, &mut said),
        "init" if !args.contains_key("goal") => {
            if let Some(goal) = first_label(&args) {
                args.insert("goal".to_owned(), json!(goal));
                said.push("no goal was given, so the first todo names the plan");
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
                .is_some_and(|out| out.contains("://"));
            if !url && args.remove("output").is_some() {
                said.push("output takes a url; that text stays in this call's arguments");
            }
        }
        _ => {}
    }
    let kept: &[&str] = match op.as_str() {
        "block" | "accepted_by_user" | "accept" => &["note"],
        "supersede" => &["reason"],
        _ => &[],
    };
    if ANNOTATIONS
        .iter()
        .filter(|key| !kept.contains(key))
        .any(|key| args.remove(*key).is_some())
    {
        said.push("the plan records no note on this op; it stays in this call's arguments");
    }
    weights(&mut args, &mut said);
    (args, said)
}

/// `goal` and `todos` open a plan and `list` sets one; a key named after an op, as a flag or
/// holding the arguments, is that op.
fn infer_op(args: &mut Map<String, Value>) {
    let named = ALL_OPS
        .iter()
        .take(MODEL_OPS)
        .map(|op| op_name(*op))
        .find(|name| args.contains_key(*name));
    let op = match named {
        Some(name) => {
            if let Some(Value::Object(inner)) = args.remove(name) {
                args.extend(inner);
            } else {
                args.remove(name);
            }
            name
        }
        None if args.contains_key("list") => "set",
        None if args.contains_key("todos") && args.contains_key("goal") => "init",
        None => return,
    };
    args.insert("op".to_owned(), json!(op));
}

/// `set` takes rows as `todos` or `list`; `supersede` takes them as `list`. Headings and blank
/// lines are not rows, a struck row (`- [-]`, `- [~]`) leaves the list, and the parser judges the rest.
fn checklist(op: &str, args: &mut Map<String, Value>, said: &mut Vec<&'static str>) {
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
    let kept: Vec<&String> = rows
        .iter()
        .filter(|line| {
            let line = line.trim_start();
            !(line.is_empty() || line.starts_with('#') || struck(line))
        })
        .collect();
    if rows.iter().any(|line| struck(line)) {
        said.push("struck rows (- [-]) leave the list, as an omitted row does");
    }
    let list: Vec<&str> = kept.iter().map(|line| line.as_str()).collect();
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
/// 100 are rescaled together so their ratios hold.
fn weights(args: &mut Map<String, Value>, said: &mut Vec<&'static str>) {
    let Some(Value::Array(todos)) = args.get_mut("todos") else {
        return;
    };
    for todo in todos {
        let Some(Value::Array(items)) = todo.pointer_mut("/contract/items") else {
            continue;
        };
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
            said.push("contract weights past 100 were rescaled to 1..100, keeping their ratios");
        }
    }
}

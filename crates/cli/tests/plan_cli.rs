//! `yi plan set` reads its rows as the plan tool does, through the spawned binary: a row it does
//! not name stays as it is, a struck row drops, and a JSON-string todos list is read.
use std::error::Error;

use serde_json::{Value, json};
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::doc::Plan;

use crate::plan_ops::{streams, yi_plan};
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn rig(name: &str) -> Result<Scratch, Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-plan-cli-{name}"))?;
    std::fs::create_dir_all(root.join("project"))?;
    root.home()?;
    Ok(root)
}

fn plan(root: &Scratch, op: &str, args: Value) -> TestResult {
    let output = yi_plan(root, &[op, &args.to_string()])?;
    let text = streams(&output);
    (output.status.success())
        .then_some(())
        .ok_or_else(|| format!("{op} {args}: {text}").into())
}

/// Every row's label and state, read from the store the binary wrote.
fn rows(root: &Scratch) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let store = PlanStore::open(root.join("project/.yi/plans"))?;
    let id = store.roots()?.into_iter().next().ok_or("no plan")?;
    let plan: Plan = store.read(&id)?;
    let state = |todo: &yi_types::plan::doc::Todo| {
        yi_types::plan::doc::TodoStateName::of(&todo.state).to_string()
    };
    Ok(plan
        .todos
        .iter()
        .map(|todo| (todo.label.to_string(), state(todo)))
        .collect())
}

fn row(rows: &[(String, String)], label: &str) -> Option<String> {
    rows.iter()
        .find(|(name, _)| name == label)
        .map(|(_, state)| state.clone())
}

/// Each step is judged on its own, so one red names every reading the CLI got wrong.
#[test]
fn a_cli_set_keeps_unnamed_rows_drops_struck_ones_and_reads_string_todos() -> TestResult {
    let root = rig("set")?;
    plan(
        &root,
        "set",
        json!({"goal": "ship it", "list": "- [ ] x\n- [ ] y\n- [ ] z"}),
    )?;
    let mut wrong = Vec::new();
    let steps = [
        (
            "set",
            json!({"todos": [{"label": "x", "state": "running"}]}),
            "z",
            "pending",
        ),
        ("set", json!({"list": "- [-] y"}), "y", "abandoned"),
        (
            "append",
            json!({"todos": json!([{"label": "w"}]).to_string()}),
            "w",
            "pending",
        ),
    ];
    for (op, args, label, state) in steps {
        let landed = plan(&root, op, args.clone()).map_err(|error| error.to_string());
        let seen = rows(&root)?;
        if landed.is_err() || row(&seen, label).as_deref() != Some(state) {
            wrong.push(format!(
                "{op} {args}: {landed:?}, {label} wanted {state} in {seen:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

#[test]
fn a_cli_set_leaves_a_failed_row_it_does_not_name_failed() -> TestResult {
    let root = rig("failed")?;
    plan(
        &root,
        "set",
        json!({"goal": "ship it", "list": "- [ ] a\n- [ ] b"}),
    )?;
    plan(&root, "start", json!({"label": "a"}))?;
    plan(
        &root,
        "fail",
        json!({"label": "a", "cause": "the build broke"}),
    )?;
    plan(
        &root,
        "set",
        json!({"todos": [{"label": "b", "state": "running"}]}),
    )?;
    let after = rows(&root)?;
    assert_eq!(row(&after, "a").as_deref(), Some("failed"), "{after:?}");
    Ok(())
}

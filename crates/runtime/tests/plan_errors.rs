//! The plan tool's schema shows three ops; every refusal the model reads names only those, or the
//! guide, and a wall it hits names the move that gets past it.
use crate::scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_runtime::plan::ops::{Actor, Op, OpRequest, PlanEngine, TodoSpec};
use yi_runtime::plan::store::PlanStore;
use yi_runtime::plan::tool::PlanTool;
use yi_tools::{Tool, ToolContext};
use yi_types::plan::doc::{GoalText, TodoLabel};

type TestResult = Result<(), Box<dyn Error>>;

fn rig(name: &str) -> Result<(Scratch, Arc<PlanEngine>, PlanTool), Box<dyn Error>> {
    let temp = Scratch::new(&format!("yi-plan-errors-{name}"))?;
    let store = PlanStore::open(temp.to_path_buf())?;
    let engine = Arc::new(PlanEngine::new(
        store,
        Arc::new(crate::plan_e2e::NoChildren),
    ));
    let tool = PlanTool::new(Arc::clone(&engine), Actor::Owner);
    Ok((temp, engine, tool))
}

/// The loop's order: validate refuses before execute runs.
fn call(tool: &PlanTool, args: Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    if let Err(refusal) = tool.validate(&input) {
        return (true, refusal);
    }
    let output = tool.execute(input, &ToolContext::new(std::env::temp_dir()));
    let text = yi_types::message::join_text(&output.result.content, "\n");
    (output.is_error, text)
}

fn owner(plan: Option<yi_types::plan::doc::PlanId>, op: Op) -> OpRequest {
    OpRequest {
        plan,
        actor: Actor::Owner,
        op,
        request_id: None,
        expected_revision: None,
    }
}

fn spec(text: &str) -> Result<TodoSpec, Box<dyn Error>> {
    Ok(TodoSpec {
        label: TodoLabel::new(text)?,
        after: Vec::new(),
        delegation: None,
        contract: None,
        children: Vec::new(),
        cites: Default::default(),
    })
}

/// Hidden op names a hint spelled (a head may echo the op sent); `fail` stays, as a dogfood
/// fixture pins "ends with fail and a cause" for a drop on a running todo.
const HIDDEN: [&str; 8] = [
    "init",
    "append",
    "start",
    "decompose",
    "supersede",
    "add_edge",
    "op=block",
    "op=accept",
];

fn hidden_in(text: &str) -> Vec<&'static str> {
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '='))
        .collect();
    HIDDEN.into_iter().filter(|op| words.contains(op)).collect()
}

#[test]
fn an_unknown_op_lists_exactly_the_ops_the_schema_shows() -> TestResult {
    let (_temp, _engine, tool) = rig("unknown")?;
    let shown: Vec<String> =
        serde_json::from_value(tool.schema()["properties"]["op"]["enum"].clone())?;
    let (refused, text) = call(&tool, json!({"op": "plan.create", "goal": "ship it"}));
    let listed = text.split_once("legal ops are ").ok_or(text.clone())?.1;
    let listed: Vec<&str> = listed.split(", ").collect();
    assert!(refused, "{text}");
    assert_eq!(listed, shown, "{text}");
    Ok(())
}

#[test]
fn each_refusal_hint_names_only_the_shown_ops() -> TestResult {
    let (_temp, engine, tool) = rig("hints")?;
    let (_, no_plan) = call(&tool, json!({"op": "view"}));
    let (_, misplaced) = call(
        &tool,
        json!({"op": "init", "goal": "ask first", "todos": [{"label": "wait"}], "on": {"user": null}}),
    );
    let apply = |plan, op| engine.apply(owner(plan, op));
    let label = |text: &str| TodoLabel::new(text);
    let todos = vec![spec("parent job")?, spec("later")?];
    apply(
        None,
        Op::Init {
            goal: GoalText::new("ship the widget")?,
            todos,
        },
    )?;
    let done = Op::Done {
        label: label("later")?,
        output: None,
    };
    let illegal = apply(None, done)
        .err()
        .ok_or("done on a pending todo landed")?
        .to_string();
    apply(
        None,
        Op::Start {
            label: label("parent job")?,
        },
    )?;
    let split = Op::Decompose {
        label: label("parent job")?,
        todos: vec![spec("small piece")?],
    };
    let sub = apply(None, split)?.subplan.ok_or("no sub-plan")?;
    apply(
        Some(sub.clone()),
        Op::Start {
            label: label("small piece")?,
        },
    )?;
    let deeper = Op::Decompose {
        label: label("small piece")?,
        todos: vec![spec("depth three")?],
    };
    let depth = apply(Some(sub), deeper)
        .err()
        .ok_or("a third level opened")?
        .to_string();
    let mut wrong = Vec::new();
    for (what, text, says) in [
        ("no plan", &no_plan, "no plan is open"),
        ("misplaced on", &misplaced, "does not take \"on\""),
        ("done on pending", &illegal, "is illegal"),
        ("depth", &depth, "DepthExhausted"),
    ] {
        let hint = text
            .split_once("; ")
            .map_or(text.as_str(), |(_, hint)| hint);
        if !text.contains(says) || !hidden_in(hint).is_empty() {
            wrong.push(format!("{what}: {:?} in {text}", hidden_in(hint)));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// The wall's remedy, run as the model would: the checkpoint is a view of the journal, so a
/// deleted `plan.json` comes back from it and the next set lands.
#[test]
fn an_edited_checkpoint_is_deleted_and_the_next_set_lands() -> TestResult {
    let (temp, _engine, tool) = rig("external")?;
    let (refused, text) = call(
        &tool,
        json!({"op": "set", "goal": "ship it", "list": "- [ ] build it"}),
    );
    assert!(!refused, "{text}");
    let checkpoint = std::fs::read_dir(&temp)?
        .flatten()
        .map(|entry| entry.path().join("plan.json"))
        .find(|path| path.is_file())
        .ok_or("no plan.json")?;
    std::fs::write(&checkpoint, "{")?;
    let (refused, text) = call(&tool, json!({"op": "set", "list": "- [ ] test it"}));
    assert!(
        refused && text.contains("changed behind the engine"),
        "{text}"
    );
    assert!(
        text.contains(&format!("delete {}", checkpoint.display())),
        "{text}"
    );
    std::fs::remove_file(&checkpoint)?;
    let (refused, text) = call(&tool, json!({"op": "set", "list": "- [ ] test it"}));
    assert!(!refused && text.contains("test it"), "{text}");
    let rebuilt = std::fs::read_to_string(&checkpoint)?;
    assert!(
        rebuilt.contains("build it") && rebuilt.contains("test it"),
        "{rebuilt}"
    );
    Ok(())
}

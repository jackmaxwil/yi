use std::collections::HashSet;
use std::error::Error;
use std::path::Path;

use serde_json::{Value, json};
use yi_types::graph::{Edge, Graph, PREDICATES, Predicate};

fn edge(condition: &str) -> Result<Edge, serde_json::Error> {
    serde_json::from_value(json!({"from": "read", "relation": "then", "to": "grep",
        "condition": condition, "guidance": "grep for it", "weight": 1}))
}

/// Invariant: a condition is a name from the closed set; there is no expression to evaluate.
#[test]
fn an_unknown_predicate_fails_to_parse() -> Result<(), serde_json::Error> {
    for (name, arguments) in PREDICATES {
        if arguments.is_empty() {
            assert_eq!(edge(name)?.condition.as_str(), *name);
        }
        for argument in arguments.split_whitespace() {
            let condition = format!("{name}({argument})");
            assert_eq!(edge(&condition)?.condition.as_str(), condition);
        }
    }
    for unknown in [
        "the_model_thinks_so",
        "",
        "Always",
        "always()",
        "result_error()",
        "result_error(denied not_found)",
        "result_error",
        "result_error(banana)",
        "result_error(denied",
        "todo_state(running) or always",
        "not(always)",
        "result_ok && todo_open",
        "child_state(running)(finished)",
    ] {
        assert!(edge(unknown).is_err(), "{unknown:?} parsed");
        assert!(Predicate::try_from(unknown.to_owned()).is_err());
    }
    Ok(())
}

/// The offline refiner judges the same file with its own `check` (`evals/graph/test_refine.py`),
/// so a rule that moves on one side fails on the other.
#[test]
fn the_shared_fixture_is_judged_alike_on_both_sides() -> Result<(), Box<dyn Error>> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/fixtures/graph/structural.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let strings = |value: &Value| -> Vec<String> {
        let items = value.as_array().into_iter().flatten();
        items.filter_map(Value::as_str).map(str::to_owned).collect()
    };
    let table: Vec<String> = PREDICATES
        .iter()
        .flat_map(|(name, arguments)| match arguments.is_empty() {
            true => vec![(*name).to_owned()],
            false => arguments
                .split_whitespace()
                .map(|argument| format!("{name}({argument})"))
                .collect(),
        })
        .collect();
    assert_eq!(
        strings(&fixture["conditions"]),
        table,
        "the closed set, in the table's order"
    );
    let verbs = strings(&fixture["verbs"]);
    let verbs: HashSet<&str> = verbs.iter().map(String::as_str).collect();
    let cases = fixture["cases"].as_array().ok_or("no cases")?;
    assert!(cases.len() > 20, "the fixture lost its cases");
    for case in cases {
        let mut graph = case["graph"].clone();
        let repeat = usize::try_from(case["repeat"].as_u64().ok_or("no repeat")?)?;
        let once = graph["edges"].as_array().ok_or("no edges")?.clone();
        graph["edges"] = (0..repeat).flat_map(|_| once.clone()).collect();
        let judged = match serde_json::from_value::<Graph>(graph) {
            Ok(graph) => graph.check(&verbs).err(),
            Err(_) => Some("parse"),
        };
        assert_eq!(judged, case["fails"].as_str(), "{}", case["name"]);
    }
    Ok(())
}

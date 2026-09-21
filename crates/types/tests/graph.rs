use serde_json::json;
use yi_types::graph::{Edge, PREDICATES, Predicate};

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

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_runtime::permission::PermissionBroker;
use yi_runtime::{AskOutcome, Asker, PermissionMode};
use yi_tools::ToolKind;

type TestResult = Result<(), Box<dyn Error>>;

fn write(path: &str, content: &str) -> Map<String, Value> {
    let mut args = Map::new();
    args.insert("path".to_owned(), json!(path));
    args.insert("content".to_owned(), json!(content));
    args
}

/// Incident: "Always allow" keyed its rule on the full arguments, so the next edit to a
/// sibling file carried different content, missed the rule and asked again.
#[test]
fn always_allow_on_an_edit_does_not_ask_for_the_next_edit_in_that_dir() -> TestResult {
    let asks = Arc::new(Mutex::new(0_usize));
    let offered = Arc::new(Mutex::new(Vec::new()));
    let (counted, labels) = (Arc::clone(&asks), Arc::clone(&offered));
    let asker: Asker = Arc::new(move |ask| {
        if let Ok(mut count) = counted.lock() {
            *count += 1;
        }
        if let Ok(mut labels) = labels.lock() {
            labels.extend(ask.grants.iter().map(|grant| grant.label.clone()));
        }
        AskOutcome::AllowAlways(0)
    });
    let broker = PermissionBroker::new(
        PermissionMode::Ask,
        PathBuf::from("/home/user/project"),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    );
    let decide = |id: &str, path: &str, content: &str| {
        // The write tool reports itself irreversible; a turn checkpoint is what undoes it.
        broker.decide_call(
            "write",
            ToolKind::Write,
            true,
            id,
            &write(path, content),
            None,
        )
    };
    assert!(decide("c1", "crates/tui/src/app.rs", "one").allowed);
    assert!(decide("c2", "crates/tui/src/render.rs", "two").allowed);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        1,
        "the second edit in the granted directory asked"
    );
    assert_eq!(
        offered.lock().map_err(|_| "poisoned")?.as_slice(),
        ["edits under crates/tui/src", "edits anywhere in this tree"]
    );
    assert!(decide("c3", "crates/cli/src/main.rs", "three").allowed);
    assert_eq!(
        *asks.lock().map_err(|_| "poisoned")?,
        2,
        "an edit outside the granted directory must ask"
    );
    Ok(())
}

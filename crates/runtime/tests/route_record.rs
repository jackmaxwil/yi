use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use yi_runtime::ext::{ExtOptions, Host, install};

type TestResult = Result<(), Box<dyn Error>>;

/// 2 imperatives (refactor, migrate), 3 bullets, 1 question word (explain),
/// 1 code fence, 2 occurrences of " and ", 0 named paths.
const CRAFTED: &str = "Refactor the loader and migrate the cache and explain the plan:\n\
- read the manifest once\n\
- drop the retry helper\n\
- land the fix\n\
```rust\n\
let cached = load();\n\
```\n";

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "route-record".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

fn host_for(cwd: &std::path::Path) -> Host {
    install(ExtOptions {
        cwd: cwd.to_path_buf(),
        home: cwd.to_path_buf(),
        mode: yi_runtime::PermissionMode::Auto,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
    })
}

fn records(store: &yi_session::SharedSession, key: &str) -> Result<Vec<Value>, Box<dyn Error>> {
    let entries =
        yi_session::lock_session(store).find_entries(&yi_session::EntryQuery::default())?;
    Ok(entries
        .iter()
        .filter_map(|entry| match entry {
            yi_types::entry::Entry::Custom {
                custom_type, data, ..
            } if custom_type == "ext_record" => data.clone(),
            _ => None,
        })
        .filter(|row| row.get("key").and_then(Value::as_str) == Some(key))
        .filter_map(|row| row.get("value").cloned())
        .collect())
}

/// An offline weight fit reads the persisted row, not the prompt: a component
/// the scorer uses and the row omits cannot be fitted at all, and one that
/// drifts fits the wrong number.
#[test]
fn the_route_record_carries_every_prefilter_score_component() -> TestResult {
    let dir = Scratch::new("yi-route-record")?;
    let store = memory_store();
    let mut host = host_for(&dir);
    host.start(Some(&store), false);
    let event = host.prompt_event(CRAFTED);
    host.dispatch(&event, Some(&store));

    let rows = records(&store, "route")?;
    let row = rows.first().ok_or("no route record was persisted")?;
    let number = |key: &str| -> Result<i64, Box<dyn Error>> {
        row.get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("route row has no numeric `{key}`: {row}").into())
    };
    assert_eq!(row.get("route").and_then(Value::as_str), Some("complex"));
    assert_eq!(number("words")?, 31);
    assert_eq!(number("enums")?, 3);
    assert_eq!(number("imperatives")?, 2);
    assert_eq!(number("questions")?, 1);
    assert_eq!(number("and_count")?, 2);
    assert_eq!(number("named_paths")?, 0);
    assert_eq!(
        row.get("fenced").and_then(Value::as_bool),
        Some(true),
        "the fence penalty is a feature the fit needs: {row}"
    );
    assert_eq!(
        number("score")?,
        5,
        "score is the sum the bounds are compared against: {row}"
    );
    Ok(())
}

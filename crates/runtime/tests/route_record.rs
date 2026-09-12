#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yi_runtime::ext::{Event, ExtOptions, Host, install};

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

/// The route the prefilter gave `prompt`, as the persisted row records it.
fn route_of(prompt: &str) -> Result<String, Box<dyn Error>> {
    let store = memory_store();
    let mut host = host_for(&std::env::temp_dir());
    host.start(Some(&store), false);
    let event = host.prompt_event(prompt);
    host.dispatch(&event, Some(&store));
    let rows = records(&store, "route")?;
    let route = rows.first().and_then(|row| row.get("route"));
    Ok(route
        .and_then(Value::as_str)
        .ok_or("no route record was persisted")?
        .to_owned())
}

#[test]
fn the_prefilter_separates_a_question_from_a_program() -> TestResult {
    assert_eq!(route_of("what does this do?")?, "one_shot");
    assert_eq!(
        route_of(
            "refactor crates/runtime/src/ext and migrate crates/cli/src/main.rs, then split the tests"
        )?,
        "complex"
    );
    assert_eq!(
        route_of("add a retry to crates/ai/src/request.rs when the provider answers 429")?,
        "undecided"
    );
    Ok(())
}

const BULLETED: &str = "Notes from the session, before the next step:\n\
    - the loader reads the manifest twice on startup\n\
    - the second read happens inside the retry helper\n\
    - both reads share one cache entry, so the miss is silent\n\
    - the timing only shows up under a cold cache\n";

#[test]
fn every_commonmark_bullet_marker_counts_as_an_enumeration() -> TestResult {
    let marked = |marker: &str| BULLETED.replace("- ", marker);
    assert_eq!(route_of(BULLETED)?, "complex");
    assert_eq!(route_of(&marked("* "))?, "complex");
    assert_eq!(route_of(&marked("+ "))?, "complex");
    assert_eq!(route_of(&marked("*"))?, "undecided");
    Ok(())
}

/// An escalation is telemetry: the fragment it attached rode every later turn of 12 of row
/// 0028's 21 sessions, and none of them called `rlm`, `plan` or `get_context`.
#[test]
fn an_escalation_is_recorded_and_leaves_the_prompt_alone() -> TestResult {
    let five_calls = Event::TurnEnd {
        turn: 0,
        tool_calls_this_turn: 5,
    };
    let wide_search = Event::ToolResult {
        name: "grep".to_owned(),
        exit: Some(0),
        files_matched: 9,
    };
    for (signal, prompt, trajectory) in [
        ("prefilter", CRAFTED, None),
        ("tool_calls_per_turn", "fix the typo", Some(five_calls)),
        ("files_matched", "fix the typo", Some(wide_search)),
    ] {
        let store = memory_store();
        let mut host = host_for(&std::env::temp_dir());
        let reminders = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&reminders);
        host.set_notice(Arc::new(move |line: &str| {
            if let Ok(mut lines) = sink.lock() {
                lines.push(line.to_owned());
            }
        }));
        host.start(Some(&store), false);
        let before = host.system_prompt();
        let submitted = host.prompt_event(prompt);
        host.dispatch(&submitted, Some(&store));
        if let Some(event) = &trajectory {
            host.dispatch(event, Some(&store));
        }
        assert_eq!(
            records(&store, "orchestrate_attached")?,
            [json!({ "signal": signal })]
        );
        let after = host.system_prompt();
        assert!(
            before == after,
            "{signal}: the escalation added {} bytes to the prompt",
            after.len().saturating_sub(before.len())
        );
        let sent = reminders.lock().map_err(|_| "poisoned")?.join("\n");
        assert!(sent.is_empty(), "{signal}: the escalation sent {sent:?}");
    }
    Ok(())
}

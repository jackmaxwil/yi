use std::error::Error;
use std::path::Path;

use serde_json::{Map, Value, json};
use yi_runtime::affordance;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_spawn_hands_back_the_exact_collect_and_watch_calls() {
    let line = affordance::spawned("porter");
    assert!(line.starts_with(affordance::NEXT));
    assert!(line.contains("rlm.wait(120)"));
    assert!(line.contains("rlm.send('porter'"));
    assert!(
        !line.contains(".jsonl"),
        "F7 writes the transcript, but the spawn reply names its dir as data — \
         the hint stays the two calls the model makes during the run: {line}"
    );
    assert_eq!(line.lines().count(), 1, "at most two lines, one is plenty");
}

#[test]
fn listing_name_names_the_session_name_field() {
    let line = affordance::listing_name();
    assert!(line.starts_with(affordance::NEXT));
    assert!(line.contains("session_name"));
    assert!(line.contains("RLMSpawnHandle.name"));
}

#[test]
fn a_finished_child_says_how_to_take_its_answer_as_data() {
    let line = affordance::child_finished("porter");
    assert!(line.contains("rlm.result('porter', schema="));
}

#[test]
fn compaction_names_where_the_full_history_lives() {
    let with_file = affordance::compacted(Some(Path::new("/tmp/sessions/s.jsonl")));
    assert!(with_file.contains("/tmp/sessions/s.jsonl"));
    assert!(!affordance::compacted(None).contains("stays in"));
}

/// The repair template carries the caller's own arguments, so the model reads
/// its own call back with the missing key filled in as a placeholder.
#[test]
fn the_repair_template_keeps_the_arguments_the_caller_gave() -> TestResult {
    let schema = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "pattern": {"type": "string"},
            "context": {"type": "integer"}
        },
        "required": ["pattern"]
    });
    let mut arguments = Map::new();
    arguments.insert("path".to_owned(), json!("crates/runtime/src/lib.rs"));
    let line = affordance::call_template("grep", &schema, &arguments);
    let rendered = line
        .strip_prefix(&format!("{}call grep as ", affordance::NEXT))
        .ok_or("unexpected shape")?;
    let value: Value = serde_json::from_str(rendered)?;
    assert_eq!(value["path"], "crates/runtime/src/lib.rs");
    assert_eq!(value["pattern"], "<string>");
    assert!(
        value.get("context").is_none(),
        "an optional key the caller never used stays out of the template"
    );
    Ok(())
}

#[test]
fn an_affordance_is_appended_to_the_last_text_block() -> TestResult {
    let mut result = yi_types::event::ToolResult {
        content: vec![yi_types::message::Content::Text {
            text: "no output".to_owned(),
            text_signature: None,
        }],
        details: Value::Object(Map::new()),
        usage: None,
        added_tool_names: None,
        terminate: None,
    };
    affordance::append(&mut result, "next: try grep");
    let Some(yi_types::message::Content::Text { text, .. }) = result.content.first() else {
        return Err("expected a text block".into());
    };
    assert_eq!(text, "no output\nnext: try grep");
    Ok(())
}

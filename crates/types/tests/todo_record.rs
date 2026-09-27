use yi_types::plan::doc::TodoStateName;
use yi_types::todo::{BlockedOn, TodoInterceptRecord, TodoRecord};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_v1_record_fixture_deserializes_and_keeps_unknown_fields() -> TestResult {
    let raw = include_str!("fixtures/todo-record-v1.json");
    let record: TodoRecord = serde_json::from_str(raw)?;
    assert_eq!(record.op, "block");
    assert_eq!(record.touched, 3);
    assert_eq!(record.list.phases.len(), 2);
    let progress = record.list.progress();
    assert_eq!(
        (
            progress.done,
            progress.total,
            progress.open,
            progress.blocked
        ),
        (2, 6, 2, 1)
    );
    let blocked = record
        .list
        .items()
        .find(|item| item.state == TodoStateName::Blocked)
        .ok_or("no blocked item")?;
    assert_eq!(blocked.on, Some(BlockedOn::User));
    assert_eq!(
        record.list.fingerprint(),
        "land the branch=blocked\ntool=pending\nwrite the fix=running"
    );
    let again = serde_json::to_value(&record)?;
    assert_eq!(again["futureTop"], "kept");
    assert_eq!(again["list"]["futureField"]["kept"], true);
    let first: Vec<&str> = again["list"]["phases"][0]["items"][0]
        .as_object()
        .ok_or("item")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(first, vec!["label", "state", "evidence"]);
    Ok(())
}

#[test]
fn an_unknown_blocker_and_an_intercept_record_round_trip() -> TestResult {
    let on: BlockedOn = serde_json::from_str("\"oracle\"")?;
    assert_eq!(on, BlockedOn::Other("oracle".to_owned()));
    assert_eq!(serde_json::to_string(&on)?, "\"oracle\"");
    let record = TodoInterceptRecord {
        at: 5,
        rung: 2,
        reason: "unchanged".to_owned(),
        fingerprint: "a=running".to_owned(),
        cycle_total: 2,
        extra: serde_json::Map::new(),
    };
    let text = serde_json::to_string(&record)?;
    assert_eq!(
        text,
        "{\"at\":5,\"rung\":2,\"reason\":\"unchanged\",\"fingerprint\":\"a=running\",\"cycleTotal\":2}"
    );
    assert_eq!(serde_json::from_str::<TodoInterceptRecord>(&text)?, record);
    Ok(())
}

/// Dies with the intent dropped, renamed or reordered: a session file written with it must read
/// back and re-serialize to the same bytes, and one written before it (v1 above) is unchanged.
#[test]
fn a_record_carrying_intent_round_trips_byte_for_byte() -> TestResult {
    let raw = include_str!("fixtures/todo-record-intent-v1.json");
    let record: TodoRecord = serde_json::from_str(raw)?;
    let intents: Vec<Vec<String>> = record
        .list
        .items()
        .map(|item| item.intent.iter().map(ToString::to_string).collect())
        .collect();
    assert_eq!(intents, [["user://2"], ["user://2"]]);
    assert_eq!(serde_json::to_string(&record)?, raw);
    Ok(())
}

/// Dies with a trailing citation left in the label, or a label word mistaken for one.
#[test]
fn trailing_user_addresses_on_a_row_are_its_intent() -> TestResult {
    let item = yi_types::todo::TodoItem::from_text("wire the parser user://1 user://3")?;
    assert_eq!(item.label.as_str(), "wire the parser");
    let cited: Vec<String> = item.intent.iter().map(ToString::to_string).collect();
    assert_eq!(cited, ["user://1", "user://3"]);
    let plain = yi_types::todo::TodoItem::from_text("read the notes at user://notes")?;
    assert_eq!(plain.label.as_str(), "read the notes at user://notes");
    assert!(plain.intent.is_empty());
    Ok(())
}

/// Dies with the ask dropped, renamed or reordered on a session's list: the record the engine
/// wrote when a todo asked reads back with its options and re-serializes to the same bytes.
#[test]
fn a_record_carrying_an_ask_round_trips_byte_for_byte() -> TestResult {
    let raw = include_str!("fixtures/todo-record-ask-v1.json");
    let record: TodoRecord = serde_json::from_str(raw)?;
    let asked = record
        .list
        .items()
        .find_map(|item| item.ask.as_ref())
        .ok_or("no ask")?;
    assert_eq!(asked.to_string(), "1. Calm · 2. Bold · 3. Dense");
    assert!(asked.answer.is_none());
    assert_eq!(serde_json::to_string(&record)?, raw);
    Ok(())
}

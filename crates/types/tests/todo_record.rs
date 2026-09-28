use serde_json::Value;
use yi_types::plan::doc::{AgentId, BlockedOn, Todo, TodoState};
use yi_types::todo::{TodoInterceptRecord, TodoList, TodoRecord};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FORMAT_ONE: [&str; 3] = [
    include_str!("fixtures/todo-record-v1.json"),
    include_str!("fixtures/todo-record-intent-v1.json"),
    include_str!("fixtures/todo-record-ask-v1.json"),
];

fn item<'a>(list: &'a TodoList, label: &str) -> Result<&'a Todo, String> {
    list.items()
        .find(|item| item.label.as_str() == label)
        .ok_or(format!("no item {label:?}"))
}

/// Dies with a format-1 item losing its payload in the merge: the blocker, the runner, the drop
/// reason, the evidence and every unknown field of the list and the record survive the read.
#[test]
fn the_v1_record_fixture_migrates_and_keeps_unknown_fields() -> TestResult {
    let record: TodoRecord = serde_json::from_str(FORMAT_ONE[0])?;
    assert_eq!(record.op, "block");
    assert_eq!(record.touched, 3);
    let list = &record.list;
    let progress = list.progress();
    assert_eq!(
        (
            progress.done,
            progress.total,
            progress.open,
            progress.blocked
        ),
        (2, 6, 2, 1)
    );
    assert_eq!(
        item(list, "land the branch")?.state,
        TodoState::Blocked {
            on: BlockedOn::User,
            note: "which base branch".to_owned()
        }
    );
    assert_eq!(
        item(list, "write the fix")?.state,
        TodoState::Running {
            by: AgentId::owner()
        }
    );
    let dropped = item(list, "old idea")?;
    assert_eq!(dropped.state, TodoState::Abandoned);
    assert_eq!(
        dropped.note.as_ref().map(|note| note.as_str()),
        Some("superseded")
    );
    assert_eq!(
        item(list, "read the code")?.evidence.as_deref(),
        Some("read crates/runtime/src/todo/mod.rs")
    );
    assert_eq!(
        list.fingerprint(),
        "land the branch=blocked\ntool=pending\nwrite the fix=running"
    );
    let again = serde_json::to_value(&record)?;
    assert_eq!(again["futureTop"], "kept");
    assert_eq!(again["list"]["futureField"]["kept"], true);
    assert_eq!(again["list"]["format"], 2);
    Ok(())
}

fn recorded() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tui/tests/fixtures/sessions/01a0d6e4.jsonl"
    );
    let mut records = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let row: Value = serde_json::from_str(line)?;
        if row.get("customType").and_then(Value::as_str) == Some("todo") {
            records.push(row.get("data").ok_or("no data")?.to_string());
        }
    }
    Ok(records)
}

/// Dies with a migration that is not a fixed point: every format-1 record, the fixtures and a
/// recorded session's thirteen, reads back the same after one write, and a second write
/// changes no byte.
#[test]
fn migrating_a_record_twice_is_a_no_op() -> TestResult {
    let recorded = recorded()?;
    assert_eq!(recorded.len(), 13);
    for raw in FORMAT_ONE
        .iter()
        .copied()
        .chain(recorded.iter().map(String::as_str))
    {
        let once: TodoRecord = serde_json::from_str(raw)?;
        let written = serde_json::to_string(&once)?;
        let twice: TodoRecord = serde_json::from_str(&written)?;
        assert_eq!(twice, once, "{raw}");
        assert_eq!(serde_json::to_string(&twice)?, written);
    }
    Ok(())
}

/// Dies with data another version wrote being dropped: an unknown item field, state tag and
/// blocker survive the format-1 read and then a format-2 round trip.
#[test]
fn unknown_item_fields_and_tags_survive_both_formats() -> TestResult {
    let legacy = r#"{"phases":[{"name":"Tasks","items":[{"label":"a","state":"paused","futureItem":1},{"label":"b","state":"blocked","on":"oracle","note":"n"}]}]}"#;
    let list: TodoList = serde_json::from_str(legacy)?;
    let paused = item(&list, "a")?;
    assert_eq!(paused.state, TodoState::Other("paused".to_owned()));
    assert_eq!(paused.extra.get("futureItem"), Some(&Value::from(1)));
    assert_eq!(
        item(&list, "b")?.state,
        TodoState::Blocked {
            on: BlockedOn::Other("oracle".to_owned()),
            note: "n".to_owned()
        }
    );
    let written = serde_json::to_string(&list)?;
    assert_eq!(serde_json::from_str::<TodoList>(&written)?, list);
    for kept in ["\"futureItem\":1", "\"paused\"", "\"on\":\"oracle\""] {
        assert!(written.contains(kept), "{kept} in {written}");
    }
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

/// Dies with stage 1's intent or stage 2's ask dropped by the migration: the format-1 records
/// the engine wrote keep both on the item that carried them.
#[test]
fn a_format_one_intent_and_ask_survive_the_migration() -> TestResult {
    let cited: TodoRecord = serde_json::from_str(FORMAT_ONE[1])?;
    let intents: Vec<Vec<String>> = cited
        .list
        .items()
        .map(|item| item.cites.intent.iter().map(ToString::to_string).collect())
        .collect();
    assert_eq!(intents, [["user://2"], ["user://2"]]);
    let asked: TodoRecord = serde_json::from_str(FORMAT_ONE[2])?;
    let hero = item(&asked.list, "hero style")?;
    let ask = hero.ask.as_ref().ok_or("no ask")?;
    assert_eq!(ask.to_string(), "1. Calm · 2. Bold · 3. Dense");
    assert!(ask.answer.is_none());
    assert_eq!(hero.extra.get("plan"), Some(&Value::from("land-the-page")));
    Ok(())
}

/// Dies with a trailing citation left in the label, or a label word mistaken for one.
#[test]
fn trailing_user_addresses_on_a_row_are_its_intent() -> TestResult {
    let item = Todo::from_text("wire the parser user://1 user://3")?;
    assert_eq!(item.label.as_str(), "wire the parser");
    let cited: Vec<String> = item.cites.intent.iter().map(ToString::to_string).collect();
    assert_eq!(cited, ["user://1", "user://3"]);
    let plain = Todo::from_text("read the notes at user://notes")?;
    assert_eq!(plain.label.as_str(), "read the notes at user://notes");
    assert!(plain.cites.intent.is_empty());
    Ok(())
}

/// Dies with the format-2 write shape moving: a record the merged binary wrote in a faux run
/// reads back and re-serializes byte for byte, its runner, blocker, ask and intent intact.
#[test]
fn a_format_two_record_round_trips_byte_for_byte() -> TestResult {
    let raw = include_str!("fixtures/todo-record-v2.json");
    let record: TodoRecord = serde_json::from_str(raw)?;
    assert_eq!(serde_json::to_string(&record)?, raw);
    let list = &record.list;
    assert_eq!(
        item(list, "read the schema")?.state,
        TodoState::Running {
            by: AgentId::owner()
        }
    );
    let tool = item(list, "wire the tool")?;
    assert_eq!(
        tool.state,
        TodoState::Blocked {
            on: BlockedOn::User,
            note: "which transport?".to_owned()
        }
    );
    let ask = tool.ask.as_ref().ok_or("no ask")?;
    assert_eq!(ask.to_string(), "1. Stdio · 2. Socket · 3. HTTP");
    let parser = item(list, "write the parser")?;
    let cited: Vec<String> = parser
        .cites
        .intent
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(cited, ["user://1"]);
    assert_eq!(
        parser.evidence.as_deref(),
        Some("`cargo test -p parser` 12 passed")
    );
    Ok(())
}

/// Dies with a list lost to a binary from before the merge: that binary rewrote a format-2 list
/// in its name-only shape, leaving a stale runner and blocker, and its last moves read back.
#[test]
fn a_format_two_list_a_pre_merge_binary_rewrote_reads_its_moves() -> TestResult {
    let raw = include_str!("fixtures/todo-record-v2-pre-merge-rewrite.json");
    let record: TodoRecord = serde_json::from_str(raw)?;
    let list = &record.list;
    let states: Vec<(&str, &TodoState)> = list
        .items()
        .map(|item| (item.label.as_str(), &item.state))
        .collect();
    let running = TodoState::Running {
        by: AgentId::owner(),
    };
    let waiting = TodoState::Blocked {
        on: BlockedOn::External { probe: None },
        note: "waiting on CI".to_owned(),
    };
    let done = TodoState::Done {
        output: None,
        resolution: None,
    };
    assert_eq!(
        states,
        [
            ("write the parser", &TodoState::Pending),
            ("read the schema", &running),
            ("wire the tool", &TodoState::Pending),
            ("ship it", &done),
            ("write the docs", &waiting),
        ]
    );
    assert_eq!(
        item(list, "ship it")?.evidence.as_deref(),
        Some("`true` exit 0")
    );
    let written = serde_json::to_string(&record)?;
    let items = &serde_json::from_str::<Value>(&written)?["list"]["phases"][0]["items"];
    for (at, stale) in [(0, "by"), (1, "blocked"), (3, "on")] {
        assert!(
            items[at].get(stale).is_none(),
            "stale {stale} in {}",
            items[at]
        );
    }
    let again: TodoRecord = serde_json::from_str(&written)?;
    assert_eq!(again, record);
    assert_eq!(serde_json::to_string(&again)?, written);
    Ok(())
}

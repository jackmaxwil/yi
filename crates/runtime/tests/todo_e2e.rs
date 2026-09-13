use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, SharedSession};
use yi_runtime::todo::tool::TodoTool;
use yi_runtime::todo::{Op, Target, TodoError, TodoStore, latest_record, text};
use yi_tools::{Tool, ToolContext};
use yi_types::plan::doc::{TodoLabel, TodoStateName};
use yi_types::todo::{BlockedOn, PhaseName, TodoItem, TodoList};

type TestResult = Result<(), Box<dyn Error>>;

fn session(name: &str) -> Result<(SharedSession, std::path::PathBuf), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("yi-todo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let mut repo = JsonlRepo::new(root.join("sessions"), root.display().to_string());
    let store = repo.create(CreateOptions {
        id: Some(name.to_owned()),
        parent_session_id: None,
        metadata: None,
    })?;
    Ok((store, root))
}

fn store_for(session: &SharedSession) -> Arc<TodoStore> {
    let session = session.clone();
    TodoStore::new(Arc::new(move || Some(session.clone())), "main")
}

fn label(text: &str) -> Result<TodoLabel, Box<dyn Error>> {
    Ok(TodoLabel::new(text)?)
}

fn item(text: &str) -> Result<TodoItem, Box<dyn Error>> {
    Ok(TodoItem::from_text(text)?)
}

fn call(tool: &TodoTool, args: Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    let output = tool.execute(input, &ToolContext::new(std::env::temp_dir()));
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    (output.is_error, text)
}

fn ids(list: &TodoList) -> Vec<(String, String)> {
    list.items()
        .map(|item| {
            (
                item.id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                item.label.to_string(),
            )
        })
        .collect()
}

fn states(list: &TodoList) -> Vec<(String, TodoStateName)> {
    list.items()
        .map(|item| (item.label.to_string(), item.state.clone()))
        .collect()
}

#[test]
fn init_starts_the_first_item_and_done_moves_the_pointer() -> TestResult {
    let (session, _root) = session("init")?;
    let store = store_for(&session);
    let applied = store.apply(
        Op::Init {
            phases: vec![(
                PhaseName::new("Build")?,
                vec![item("read the code")?, item("write the fix")?],
            )],
        },
        None,
    )?;
    assert_eq!(
        states(&applied.list),
        vec![
            ("read the code".to_owned(), TodoStateName::Running),
            ("write the fix".to_owned(), TodoStateName::Pending),
        ]
    );
    let applied = store.apply(
        Op::Done {
            target: Target::Label(label("read the code")?),
            evidence: Some("`read crates/runtime/src/todo/mod.rs` 40 lines shown".to_owned()),
        },
        None,
    )?;
    assert_eq!(
        states(&applied.list),
        vec![
            ("read the code".to_owned(), TodoStateName::Done),
            ("write the fix".to_owned(), TodoStateName::Running),
        ]
    );
    assert_eq!(applied.touched, 2);
    let record = latest_record(&session).ok_or("no record")?;
    assert_eq!(record.op, "done");
    assert_eq!(record.touched, 2);
    assert_eq!(
        record
            .list
            .items()
            .next()
            .and_then(|item| item.evidence.clone()),
        Some("`read crates/runtime/src/todo/mod.rs` 40 lines shown".to_owned())
    );
    Ok(())
}

#[test]
fn the_list_rehydrates_from_the_session_on_a_fresh_store() -> TestResult {
    let (session, _root) = session("rehydrate")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "## Fix\n- [ ] one\n  - [ ] one a\n- [ ] two\n".to_owned(),
        },
        None,
    )?;
    store.apply(
        Op::Block {
            label: label("two")?,
            on: BlockedOn::User,
            note: "which branch to land on".to_owned(),
        },
        None,
    )?;
    let again = store_for(&session);
    assert_eq!(again.touched(), 2);
    let list = again.list();
    let two = list
        .items()
        .find(|item| item.label.as_str() == "two")
        .ok_or("two missing")?;
    assert_eq!(two.state, TodoStateName::Blocked);
    assert_eq!(two.on, Some(BlockedOn::User));
    assert_eq!(two.note.as_deref(), Some("which branch to land on"));
    assert_eq!(list.progress().blocked, 1);
    Ok(())
}

#[test]
fn a_parent_with_open_children_refuses_done_and_names_them() -> TestResult {
    let (session, _root) = session("parent")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "- [ ] parent\n  - [ ] child a\n  - [x] child b\n".to_owned(),
        },
        None,
    )?;
    let error = store
        .apply(
            Op::Done {
                target: Target::Label(label("parent")?),
                evidence: None,
            },
            None,
        )
        .err()
        .ok_or("done on an open parent must fail")?;
    assert!(
        matches!(error, TodoError::ParentOpen { ref open, .. } if open.contains("child a") && !open.contains("child b")),
        "{error}"
    );
    assert_eq!(store.touched(), 1, "a refused op leaves the list untouched");
    Ok(())
}

#[test]
fn set_keeps_a_blocker_the_rewrite_did_not_mention() -> TestResult {
    let (session, _root) = session("carry")?;
    let store = store_for(&session);
    store.apply(
        Op::Init {
            phases: vec![(PhaseName::new("Tasks")?, vec![item("a")?, item("b")?])],
        },
        None,
    )?;
    store.apply(
        Op::Done {
            target: Target::Label(label("a")?),
            evidence: Some("`cargo test` test result: ok".to_owned()),
        },
        None,
    )?;
    store.apply(
        Op::Block {
            label: label("b")?,
            on: BlockedOn::External,
            note: "CI is down".to_owned(),
        },
        None,
    )?;
    let applied = store.apply(
        Op::Set {
            list: "- [x] a\n- [!] b\n- [ ] c\n".to_owned(),
        },
        None,
    )?;
    let b = applied
        .list
        .items()
        .find(|item| item.label.as_str() == "b")
        .ok_or("b missing")?;
    assert_eq!(b.on, Some(BlockedOn::External));
    assert_eq!(b.note.as_deref(), Some("CI is down"));
    assert_eq!(
        applied.list.running().map(|item| item.label.to_string()),
        Some("c".to_owned())
    );
    Ok(())
}

#[test]
fn a_stale_touched_counter_is_refused_so_a_user_edit_survives() -> TestResult {
    let (session, _root) = session("stale")?;
    let store = store_for(&session);
    store.apply(
        Op::Init {
            phases: vec![(PhaseName::new("Tasks")?, vec![item("a")?])],
        },
        None,
    )?;
    store.apply_as(
        Op::Append {
            phase: None,
            under: None,
            items: vec![item("user added")?],
        },
        None,
        "user",
    )?;
    let error = store
        .apply(
            Op::Done {
                target: Target::Label(label("a")?),
                evidence: None,
            },
            Some(1),
        )
        .err()
        .ok_or("a stale op must be refused")?;
    assert!(
        matches!(error, TodoError::Stale { now: 2, sent: 1 }),
        "{error}"
    );
    assert_eq!(
        latest_record(&session).map(|record| record.actor),
        Some("user".to_owned())
    );
    Ok(())
}

#[test]
fn done_without_evidence_is_refused_and_says_what_evidence_is() -> TestResult {
    let (session, _root) = session("evidence")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "- [>] a\n- [ ] b\n".to_owned(),
        },
        None,
    )?;
    for evidence in [None, Some("   ".to_owned())] {
        let error = store
            .apply(
                Op::Done {
                    target: Target::Label(label("a")?),
                    evidence,
                },
                None,
            )
            .err()
            .ok_or("done with no evidence must be refused")?;
        assert!(
            matches!(error, TodoError::NoEvidence { ref label } if label == "a"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .starts_with("done needs evidence: the command you ran"),
            "{error}"
        );
    }
    assert_eq!(store.progress().open, 2, "a refusal moves nothing");
    store.apply(
        Op::Done {
            target: Target::All,
            evidence: Some("`pytest -q` 3 passed in 0.2s".to_owned()),
        },
        None,
    )?;
    assert_eq!(
        store.progress().open,
        0,
        "one evidence closes the whole list"
    );
    // Incident: row 0028 refused all three; a checker that prints `ok` could never close.
    store.apply(
        Op::Set {
            list: "- [ ] e\n- [ ] f\n- [ ] g\n".to_owned(),
        },
        None,
    )?;
    for (item, evidence) in [
        ("e", "`pytest -q` → `3 passed in 0.00s`"),
        ("f", "python3 check.py → ok"),
        ("g", "`./check` ok"),
    ] {
        store.apply(
            Op::Done {
                target: Target::Label(label(item)?),
                evidence: Some(evidence.to_owned()),
            },
            None,
        )?;
    }
    assert_eq!(store.progress().open, 0, "evidence closes in any shape");
    store.apply(
        Op::Set {
            list: "- [ ] c\n- [x] d\n".to_owned(),
        },
        None,
    )?;
    let closed = store
        .apply(
            Op::Set {
                list: "- [x] c\n- [x] d\n".to_owned(),
            },
            None,
        )
        .err()
        .ok_or("a set that moves an item to done must be refused")?;
    assert!(
        matches!(closed, TodoError::SetClosed { ref labels } if labels == "\"c\""),
        "{closed}"
    );
    assert_eq!(store.progress().open, 1, "the refused set moved nothing");
    Ok(())
}

#[test]
fn the_tool_result_teaches_the_next_move_with_the_label_filled_in() -> TestResult {
    let (session, _root) = session("tool")?;
    let tool = TodoTool::new(store_for(&session));
    let mut input: Map<String, Value> = Map::new();
    input.insert("op".to_owned(), json!("init"));
    input.insert(
        "items".to_owned(),
        json!(["wire the stop interception", "write the test"]),
    );
    let context = ToolContext::new(std::env::temp_dir());
    let output = tool.execute(input, &context);
    assert!(!output.is_error);
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    assert!(
        text.starts_with("Todos 0/2 · running: wire the stop interception"),
        "{text}"
    );
    assert!(
        text.contains("- [>] t1 wire the stop interception"),
        "{text}"
    );
    assert!(
        text.contains(
            "next: done t1 evidence=`<command>` <output line> · block t1 on user · drop t1 <reason>"
        ),
        "{text}"
    );
    assert!(text.contains("next: start t2 · drop t2 <reason>"), "{text}");
    assert!(text.ends_with("touched: 1"), "{text}");

    let mut bad: Map<String, Value> = Map::new();
    bad.insert("op".to_owned(), json!("unblock"));
    bad.insert("label".to_owned(), json!("write the test"));
    let output = tool.execute(bad, &context);
    assert!(output.is_error);
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    assert!(
        text.contains("legal here: start, done, block, drop, rm"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_move_returns_the_rows_it_changed_not_the_whole_list() -> TestResult {
    let (session, _root) = session("moved")?;
    let tool = TodoTool::new(store_for(&session));
    let (is_error, text) = call(
        &tool,
        json!({"op": "init", "items": ["read the code", "write the fix", "run the suite"]}),
    );
    assert!(!is_error, "{text}");
    assert!(text.contains("- [ ] t3 run the suite"), "{text}");
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "id": "t1", "evidence": "`wc -l src/lib.rs` 40 src/lib.rs"}),
    );
    assert!(!is_error, "{text}");
    assert!(
        text.starts_with(
            "Todos 1/3 · running: write the fix\n- [x] t1 read the code\n- [>] t2 write the fix\n"
        ),
        "{text}"
    );
    assert!(
        !text.contains("- [ ] t3 run the suite"),
        "an untouched row stays out: {text}"
    );
    assert!(text.contains("next: start t3 · drop t3 <reason>"), "{text}");
    assert!(text.ends_with("touched: 2"), "{text}");
    let (_, text) = call(&tool, json!({"op": "view"}));
    assert!(
        text.contains("- [x] t1 read the code") && text.contains("- [ ] t3 run the suite"),
        "view lists every row: {text}"
    );
    Ok(())
}

#[test]
fn a_checklist_round_trips_through_render() -> TestResult {
    let list =
        text::parse("## Build\n- [x] a\n- [>] b\n  - [ ] b1\n## Verify\n- [!] c\n- [-] d\n")?;
    let rendered = text::checklist(&list).join("\n");
    assert_eq!(
        rendered,
        "## Build\n- [x] a\n- [>] b\n  - [ ] b1\n## Verify\n- [!] c (blocked on user)\n- [-] d"
    );
    let kept = text::parse("- [ ] t7 keep me\n- [ ] fresh\n")?;
    assert_eq!(
        ids(&kept),
        vec![
            ("t7".to_owned(), "keep me".to_owned()),
            (String::new(), "fresh".to_owned())
        ]
    );
    assert!(text::parse("- [ ] a\n    - [ ] too deep\n").is_err());
    assert!(text::parse("- [ ] a\n- [ ] a\n").is_err());
    Ok(())
}

#[test]
fn a_set_that_keeps_no_label_restarts_the_ids_at_t1() -> TestResult {
    let (session, _root) = session("restart")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "- [ ] alpha\n- [ ] beta\n".to_owned(),
        },
        None,
    )?;
    store.apply(
        Op::Set {
            list: "- [x] read everything\n- [>] fix it\n- [ ] verify\n".to_owned(),
        },
        None,
    )?;
    let ids: Vec<String> = store
        .list()
        .items()
        .filter_map(|item| item.id.as_ref().map(ToString::to_string))
        .collect();
    assert_eq!(ids, vec!["t1", "t2", "t3"], "a replaced list is a new list");
    let error = store
        .apply(
            Op::Done {
                target: Target::Label(label("t9")?),
                evidence: Some("`cargo test` test result: ok".to_owned()),
            },
            None,
        )
        .err()
        .ok_or("t9 is not in the list")?;
    assert!(
        error.to_string().contains("a set or init renumbers"),
        "{error}"
    );
    Ok(())
}

#[test]
fn an_id_names_an_item_across_a_set() -> TestResult {
    let (session, _root) = session("ids")?;
    let store = store_for(&session);
    let applied = store.apply(
        Op::Init {
            phases: vec![(
                PhaseName::new("Tasks")?,
                vec![item("alpha")?, item("beta")?],
            )],
        },
        None,
    )?;
    assert_eq!(
        ids(&applied.list),
        vec![
            ("t1".to_owned(), "alpha".to_owned()),
            ("t2".to_owned(), "beta".to_owned())
        ]
    );
    store.apply(
        Op::Done {
            target: Target::Label(label("alpha")?),
            evidence: Some("`cargo test` test result: ok".to_owned()),
        },
        None,
    )?;
    let applied = store.apply(
        Op::Set {
            list: "- [x] alpha\n- [ ] gamma\n- [ ] beta\n".to_owned(),
        },
        None,
    )?;
    assert_eq!(
        ids(&applied.list),
        vec![
            ("t1".to_owned(), "alpha".to_owned()),
            ("t3".to_owned(), "gamma".to_owned()),
            ("t2".to_owned(), "beta".to_owned())
        ],
        "a surviving label keeps its id; a new one is minted past both"
    );
    let applied = store.apply(
        Op::Done {
            target: Target::Label(label("t2")?),
            evidence: Some("`cargo test` test result: ok".to_owned()),
        },
        None,
    )?;
    let beta = applied
        .list
        .items()
        .find(|item| item.label.as_str() == "beta")
        .ok_or("beta missing")?;
    assert_eq!(beta.state, TodoStateName::Done);
    store.apply(
        Op::Rm {
            target: Target::All,
        },
        None,
    )?;
    let applied = store.apply(
        Op::Append {
            phase: None,
            under: None,
            items: vec![item("delta")?],
        },
        None,
    )?;
    assert_eq!(
        ids(&applied.list),
        vec![("t4".to_owned(), "delta".to_owned())],
        "an id is never reused after rm"
    );
    Ok(())
}

#[test]
fn a_unique_prefix_matches_and_an_ambiguous_one_lists_both() -> TestResult {
    let (session, _root) = session("prefix")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "- [ ] Verify against the fixture corpus\n- [ ] Verify against the live forge\n- [ ] Write the changelog row\n".to_owned(),
        },
        None,
    )?;
    let applied = store.apply(
        Op::Start {
            label: label("`write the CHANGELOG`")?,
        },
        None,
    )?;
    assert_eq!(
        applied.list.running().map(|item| item.label.to_string()),
        Some("Write the changelog row".to_owned()),
        "case, backticks and a cut tail do not matter"
    );
    let error = store
        .apply(
            Op::Start {
                label: label("Verify against")?,
            },
            None,
        )
        .err()
        .ok_or("an ambiguous prefix must be refused")?;
    assert!(
        matches!(error, TodoError::Ambiguous { ref candidates, .. } if candidates.contains("t1 \"Verify against the fixture corpus\"") && candidates.contains("t2 \"Verify against the live forge\"")),
        "{error}"
    );
    assert!(error.to_string().ends_with("; name one by id"), "{error}");
    let error = store
        .apply(
            Op::Start {
                label: label("Write t")?,
            },
            None,
        )
        .err()
        .ok_or("a short prefix must not match")?;
    assert!(
        matches!(error, TodoError::NoSuchLabel { ref known, .. } if known.contains("t3 \"Write the changelog row\"")),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_long_label_is_cut_into_its_note_not_refused() -> TestResult {
    let (session, _root) = session("long")?;
    let tool = TodoTool::new(store_for(&session));
    let long = "Verify the parser against every fixture in the corpus, then the live forge, then the replay set";
    let (is_error, text) = call(&tool, json!({"op": "init", "items": [long, "short"]}));
    assert!(!is_error, "{text}");
    assert!(
        text.contains("- [>] t1 Verify the parser against every fixture in the corpus, then the live forge, then…"),
        "{text}"
    );
    let list = latest_record(&session).ok_or("no record")?.list;
    let first = list.items().next().ok_or("no items")?;
    assert_eq!(first.label.as_str().chars().count(), 80);
    assert_eq!(first.note.as_deref(), Some(long));
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "label": long, "evidence": "`pytest fixtures` 12 passed in 1s"}),
    );
    assert!(!is_error, "the full text still names the cut item: {text}");
    Ok(())
}

#[test]
fn a_call_with_items_and_no_op_is_an_append() -> TestResult {
    let (session, _root) = session("infer")?;
    let tool = TodoTool::new(store_for(&session));
    let (is_error, text) = call(&tool, json!({"list": "- [ ] one\n- [ ] two\n"}));
    assert!(!is_error, "{text}");
    assert!(text.starts_with("(op inferred: set)\n"), "{text}");
    let (is_error, text) = call(&tool, json!({"items": ["three"]}));
    assert!(!is_error, "{text}");
    assert!(text.starts_with("(op inferred: append)\n"), "{text}");
    assert!(text.contains("- [ ] t3 three"), "{text}");
    let (is_error, text) = call(
        &tool,
        json!({"id": "t1", "evidence": "`make check` all targets ok"}),
    );
    assert!(!is_error, "{text}");
    assert!(text.starts_with("(op inferred: done)\n"), "{text}");
    assert!(text.contains("- [x] t1 one"), "{text}");
    let (is_error, text) = call(&tool, json!({"label": "two", "reason": "out of scope"}));
    assert!(!is_error, "{text}");
    assert!(text.starts_with("(op inferred: drop)\n"), "{text}");
    let (is_error, text) = call(
        &tool,
        json!({"label": "three", "on": "user", "note": "which?"}),
    );
    assert!(!is_error, "{text}");
    assert!(text.starts_with("(op inferred: block)\n"), "{text}");
    let (is_error, text) = call(&tool, json!({"label": "three"}));
    assert!(is_error, "a bare label names no op");
    assert!(text.starts_with("op is required"), "{text}");
    let (_, text) = call(&tool, json!({"op": "view"}));
    assert!(
        !text.starts_with("(op inferred"),
        "an explicit op says nothing: {text}"
    );
    Ok(())
}

#[test]
fn an_old_session_without_ids_rehydrates_with_ids() -> TestResult {
    let (session, _root) = session("old")?;
    let record = json!({
        "op": "set", "actor": "main", "at": 1, "touched": 3,
        "list": {"phases": [{"name": "Tasks", "items": [
            {"label": "one", "state": "done"},
            {"label": "two", "state": "running", "children": [{"label": "two a", "state": "pending"}]}
        ]}]}
    });
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record))?;
    let store = store_for(&session);
    assert_eq!(store.touched(), 3);
    assert_eq!(
        ids(&store.list()),
        vec![
            ("t1".to_owned(), "one".to_owned()),
            ("t2".to_owned(), "two".to_owned()),
            ("t3".to_owned(), "two a".to_owned())
        ]
    );
    let applied = store.apply(
        Op::Done {
            target: Target::Label(label("t3")?),
            evidence: Some("`make check` all targets ok".to_owned()),
        },
        None,
    )?;
    assert_eq!(applied.list.next_id, 4);
    Ok(())
}

#[test]
fn every_argument_error_ends_with_an_id_call_that_lands() -> TestResult {
    let (session, _root) = session("example")?;
    let tool = TodoTool::new(store_for(&session));
    // In the order their examples can run on one list; each refusal is an argument error.
    let refusals = [
        json!({"op": "init"}),
        json!({"op": "start"}),
        json!({"op": "block"}),
        json!({"op": "unblock"}),
        json!({"label": "first task"}),
        json!({"op": "finish"}),
        json!({"op": "done", "label": "first\ntask"}),
        json!({"op": "set"}),
        json!({"op": "append"}),
        json!({"op": "drop"}),
        json!({"op": "rm", "label": "first\ntask"}),
    ];
    for refusal in refusals {
        let (is_error, text) = call(&tool, refusal.clone());
        assert!(is_error, "{refusal}: {text}");
        let (_, example) = text
            .split_once(" looks like ")
            .ok_or(format!("{refusal} shows no call: {text}"))?;
        assert!(
            !example.contains("\"label\""),
            "every `no todo` in the sweep came from a label, none from an id: {example}"
        );
        let (is_error, landed) = call(&tool, serde_json::from_str(example)?);
        assert!(!is_error, "{example}: {landed}");
    }
    let (_, text) = call(&tool, json!({"label": "first task"}));
    assert!(
        text.ends_with(
            r#"looks like {"op": "done", "id": "t1", "evidence": "`make check` all targets ok"}"#
        ),
        "all 17 `op is required` in the sweep were done calls: {text}"
    );
    Ok(())
}

/// Synthetic well-formed calls across every op and argument form, each with main's result: generated
/// on 7706a472, this change's base on main, by replaying each session as the golden test below does.
const GOLDEN: &str = include_str!("fixtures/todo/golden.jsonl");
/// Synthetic calls of the shapes the 2026-09-11 v4 sweep's models sent, with the call each `means`
/// or the refusal `text` it keeps; the sweep's own calls stay out of the repository.
const SHAPES: &str = include_str!("fixtures/todo/shapes.jsonl");

struct Sequence {
    name: String,
    seed: Option<String>,
    calls: Vec<Value>,
}

fn sequences(source: &str) -> Result<Vec<Sequence>, Box<dyn Error>> {
    let mut sequences: Vec<Sequence> = Vec::new();
    for line in source.lines() {
        let row: Value = serde_json::from_str(line)?;
        let name = row["session"].as_str().ok_or("a row names its session")?;
        if sequences
            .last()
            .is_none_or(|sequence| sequence.name != name)
        {
            sequences.push(Sequence {
                name: name.to_owned(),
                seed: None,
                calls: Vec::new(),
            });
        }
        let sequence = sequences.last_mut().ok_or("pushed above")?;
        match row["seed"].as_str() {
            Some(seed) => sequence.seed = Some(seed.to_owned()),
            None => sequence.calls.push(row),
        }
    }
    Ok(sequences)
}

/// A fresh session seeded as the runtime seeds it, with the session's landed calls before `upto`
/// replayed; a refused call moved nothing, so this is the list the model saw.
fn replayed_to(sequence: &Sequence, upto: usize, tag: &str) -> Result<TodoTool, Box<dyn Error>> {
    let (session, _root) = session(&format!("{}-{upto}-{tag}", sequence.name))?;
    let store = store_for(&session);
    if let Some(seed) = &sequence.seed {
        yi_runtime::todo::coupling::seed(&store, seed);
    }
    let tool = TodoTool::new(store);
    for row in sequence.calls.iter().take(upto) {
        if row["isError"] == false {
            replay(&tool, &row["args"]);
        }
    }
    Ok(tool)
}

/// validate, then execute, as the loop runs a tool call.
fn replay(tool: &TodoTool, args: &Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    match tool.validate(&input) {
        Err(reason) => (true, reason),
        Ok(()) => call(tool, args.clone()),
    }
}

#[test]
fn every_well_formed_call_returns_mains_result() -> TestResult {
    let mut calls = 0;
    for sequence in sequences(GOLDEN)? {
        let tool = replayed_to(&sequence, 0, "same")?;
        for (index, row) in sequence.calls.iter().enumerate() {
            let (is_error, text) = replay(&tool, &row["args"]);
            assert_eq!(
                row["isError"], is_error,
                "{} call {index}: {text}",
                sequence.name
            );
            assert_eq!(row["text"], text, "{} call {index}", sequence.name);
            calls += 1;
        }
    }
    assert_eq!(calls, 87);
    Ok(())
}

#[test]
fn a_call_refused_for_its_shape_lands_as_it_meant() -> TestResult {
    let (mut landed, mut refused, mut wrong) = (0, 0, Vec::new());
    for sequence in sequences(SHAPES)? {
        for (index, row) in sequence.calls.iter().enumerate() {
            if row["isError"] == false {
                continue;
            }
            let tool = replayed_to(&sequence, index, "sent")?;
            let (is_error, text) = replay(&tool, &row["args"]);
            let at = format!("{} call {index}", sequence.name);
            let Some(means) = row.get("means") else {
                // Refused word for word, and the call an argument error shows lands on that list.
                let shown = text.split_once(" looks like ").map(|(_, call)| call);
                let lands = shown.is_none_or(|call| {
                    serde_json::from_str(call).is_ok_and(|call: Value| !replay(&tool, &call).0)
                });
                if is_error && row["text"] == text && lands {
                    refused += 1;
                } else {
                    wrong.push(format!("{at} is not the refusal it was: {text}"));
                }
                continue;
            };
            let (_, meant) = replay(&replayed_to(&sequence, index, "meant")?, means);
            // A set read as init, or an init as set, names the op it took on a line of its own.
            let shown = text
                .strip_prefix("(op inferred: set)\n")
                .or_else(|| text.strip_prefix("(op inferred: init)\n"))
                .unwrap_or(&text);
            if !is_error && shown == meant {
                landed += 1;
            } else {
                wrong.push(format!("{at}: {text}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!((landed, refused), (17, 12), "of the 29 refused calls");
    Ok(())
}

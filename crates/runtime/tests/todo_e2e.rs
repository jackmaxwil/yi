#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

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

fn session(name: &str) -> Result<(Scratch, SharedSession), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-todo-{name}"))?;
    let mut repo = JsonlRepo::new(root.join("sessions"), root.display().to_string());
    let store = repo.create(CreateOptions {
        id: Some(name.to_owned()),
        parent_session_id: None,
        metadata: None,
    })?;
    Ok((root, store))
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
    let (_root, session) = session("init")?;
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
    let (_root, session) = session("rehydrate")?;
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
    let (_root, session) = session("parent")?;
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
    let (_root, session) = session("carry")?;
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
    let (_root, session) = session("stale")?;
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
    let (_root, session) = session("evidence")?;
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
    let prose = store
        .apply(
            Op::Done {
                target: Target::Label(label("a")?),
                evidence: Some(
                    "filter.py reads argv[1]; missing-arg path returns exit 2".to_owned(),
                ),
            },
            None,
        )
        .err()
        .ok_or("prose evidence must be refused")?;
    assert!(
        matches!(prose, TodoError::EvidenceShape { ref label } if label == "a"),
        "{prose}"
    );
    assert!(prose.to_string().contains("`<command>`"), "{prose}");
    store.apply(
        Op::Done {
            target: Target::Label(label("a")?),
            evidence: Some("`python3 check.py` all 12 checks passed".to_owned()),
        },
        None,
    )?;
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
    let (_root, session) = session("tool")?;
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
    let (_root, session) = session("restart")?;
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
    let (_root, session) = session("ids")?;
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
    let (_root, session) = session("prefix")?;
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
    let (_root, session) = session("long")?;
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
    let (_root, session) = session("infer")?;
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
    let (_root, session) = session("old")?;
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

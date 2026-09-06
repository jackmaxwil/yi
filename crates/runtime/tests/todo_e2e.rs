use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, SharedSession};
use yi_runtime::todo::tool::TodoTool;
use yi_runtime::todo::{Op, Target, TodoError, TodoStore, latest_record, text};
use yi_tools::{Tool, ToolContext};
use yi_types::plan::doc::{TodoLabel, TodoStateName};
use yi_types::todo::{BlockedOn, PhaseName, TodoList};

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
                vec![label("read the code")?, label("write the fix")?],
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
            evidence: Some("read crates/runtime/src/todo/mod.rs".to_owned()),
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
        Some("read crates/runtime/src/todo/mod.rs".to_owned())
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
            phases: vec![(PhaseName::new("Tasks")?, vec![label("a")?, label("b")?])],
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
            phases: vec![(PhaseName::new("Tasks")?, vec![label("a")?])],
        },
        None,
    )?;
    store.apply_as(
        Op::Append {
            phase: None,
            under: None,
            items: vec![label("user added")?],
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
        text.contains("next: done \"wire the stop interception\" · block \"wire the stop interception\" on user · drop \"wire the stop interception\" <reason>"),
        "{text}"
    );
    assert!(text.contains("next: start \"write the test\""), "{text}");
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
    assert!(text::parse("- [ ] a\n    - [ ] too deep\n").is_err());
    assert!(text::parse("- [ ] a\n- [ ] a\n").is_err());
    Ok(())
}

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, SharedSession};
use yi_runtime::todo::tool::{TodoTool, unleak};
use yi_runtime::todo::{Op, Target, TodoError, TodoStore, latest_record, text};
use yi_tools::{Tool, ToolContext};
use yi_types::plan::doc::{AgentId, BlockedOn, Todo, TodoLabel, TodoState, TodoStateName};
use yi_types::todo::{PhaseName, TodoList};

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

fn item(text: &str) -> Result<Todo, Box<dyn Error>> {
    Ok(Todo::from_text(text)?)
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
        .map(|item| (item.label.to_string(), TodoStateName::of(&item.state)))
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
            ask: None,
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
    assert_eq!(
        two.state,
        TodoState::Blocked {
            on: BlockedOn::User,
            note: "which branch to land on".to_owned()
        }
    );
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

/// Dies with done re-closing a closed item: the second call overwrote the evidence that closed
/// it, where the plan tool refuses the same move, and a done over all reopened a dropped one.
#[test]
fn done_refuses_a_done_item_and_done_all_leaves_closed_ones_alone() -> TestResult {
    let (_root, session) = session("redone")?;
    let store = store_for(&session);
    store.apply(
        Op::Set {
            list: "- [ ] read\n- [ ] fix\n- [ ] port\n".to_owned(),
        },
        None,
    )?;
    let proof = "`cargo test` 12 passed".to_owned();
    let done = |needle: &str, evidence: &str| -> Result<Op, Box<dyn Error>> {
        Ok(Op::Done {
            target: Target::Label(label(needle)?),
            evidence: Some(evidence.to_owned()),
        })
    };
    store.apply(done("read", &proof)?, None)?;
    let reason = "out of scope".to_owned();
    let port = Target::Label(label("port")?);
    store.apply(
        Op::Drop {
            target: port,
            reason,
        },
        None,
    )?;
    let error = store
        .apply(done("read", "`true` exit 0")?, None)
        .err()
        .ok_or("done on a done item must be refused")?;
    assert!(
        error.to_string().contains("\"read\" in state done"),
        "{error}"
    );
    let evidence = Some("`ls` fixed".to_owned());
    store.apply(
        Op::Done {
            target: Target::All,
            evidence,
        },
        None,
    )?;
    let list = store.list();
    let rows: Vec<(String, TodoStateName, Option<String>)> = list
        .items()
        .map(|item| {
            (
                item.label.to_string(),
                TodoStateName::of(&item.state),
                item.evidence.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("read".to_owned(), TodoStateName::Done, Some(proof)),
            (
                "fix".to_owned(),
                TodoStateName::Done,
                Some("`ls` fixed".to_owned())
            ),
            ("port".to_owned(), TodoStateName::Abandoned, None),
        ]
    );
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
            on: BlockedOn::External { probe: None },
            note: "CI is down".to_owned(),
            ask: None,
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
    assert_eq!(
        b.state,
        TodoState::Blocked {
            on: BlockedOn::External { probe: None },
            note: "CI is down".to_owned()
        }
    );
    assert_eq!(
        applied.list.running().map(|item| item.label.to_string()),
        Some("c".to_owned())
    );
    Ok(())
}

/// Dies with a stale view refused, which sent the model to view and rebuild its call, or with a
/// set from it deleting the row the user added since: the set lands and keeps that row.
#[test]
fn a_stale_set_lands_and_keeps_the_row_a_user_added() -> TestResult {
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
    let applied = store.apply(
        Op::Set {
            list: "- [>] a\n- [ ] b\n".to_owned(),
        },
        Some(1),
    )?;
    let labels: Vec<String> = applied
        .list
        .items()
        .map(|item| item.label.to_string())
        .collect();
    assert!(
        labels.contains(&"user added".to_owned()) && labels.contains(&"b".to_owned()),
        "{labels:?}"
    );
    assert!(
        applied
            .notes
            .iter()
            .any(|note| note.contains("changed since you last saw it"))
            && applied
                .notes
                .iter()
                .any(|note| note.contains("\"user added\"")),
        "{:?}",
        applied.notes
    );
    Ok(())
}

/// Dies with a stale set deleting a subtask the user added under a row it kept, and with a stale
/// point op refused again: the old view lands on the list as it is now, children included.
#[test]
fn a_stale_view_keeps_a_users_subtask_and_lands_a_point_op() -> TestResult {
    let (_root, session) = session("stale-child")?;
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
            under: Some(TodoLabel::new("a")?),
            items: vec![item("user sub")?],
        },
        None,
        "user",
    )?;
    let applied = store.apply(
        Op::Set {
            list: "- [>] a\n- [ ] b\n".to_owned(),
        },
        Some(1),
    )?;
    let a = (applied.list.items())
        .find(|row| row.label.as_str() == "a")
        .ok_or("a missing")?;
    assert!(
        a.children
            .iter()
            .any(|row| row.label.as_str() == "user sub"),
        "{:?}",
        applied.list
    );
    assert!(
        applied
            .notes
            .iter()
            .any(|note| note.contains("\"user sub\"")),
        "{:?}",
        applied.notes
    );
    let appended = store.apply(
        Op::Append {
            phase: None,
            under: None,
            items: vec![item("c")?],
        },
        Some(1),
    )?;
    assert!(appended.list.items().any(|row| row.label.as_str() == "c"));
    assert!(
        (appended.notes.iter()).any(|note| note.contains("changed since you last saw it")),
        "{:?}",
        appended.notes
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
    let closed = store.apply(
        Op::Set {
            list: "- [x] c\n- [x] d\n- [ ] h\n".to_owned(),
        },
        None,
    )?;
    assert!(
        closed
            .notes
            .iter()
            .any(|note| note.starts_with("\"c\" kept open: done needs evidence")),
        "{:?}",
        closed.notes
    );
    assert_eq!(
        store.progress().open,
        2,
        "c stays open and the new row h lands"
    );
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
fn a_move_returns_the_rows_it_changed_not_the_whole_list() -> TestResult {
    let (_root, session) = session("moved")?;
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
    assert_eq!(TodoStateName::of(&beta.state), TodoStateName::Done);
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
    assert_eq!(first.note.as_ref().map(|note| note.as_str()), Some(long));
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "label": long, "evidence": "`pytest fixtures` 12 passed in 1s"}),
    );
    assert!(!is_error, "the full text still names the cut item: {text}");
    Ok(())
}

/// Dies with the options dropped between the call and the list a parent reads, or with a two-option
/// question let through: the child's own list is where its `needs_you` note is read from.
#[test]
fn a_block_on_the_user_carries_three_to_five_options() -> TestResult {
    let (_root, session) = session("asks")?;
    let tool = TodoTool::new(store_for(&session));
    call(&tool, json!({"op": "init", "items": ["pick a name"]}));
    let option = |id: &str| json!({"id": id, "label": format!("name {id}")});
    let block = |options: Vec<Value>| json!({"op": "block", "id": "t1", "on": "user", "note": "which name?", "options": options});
    let (is_error, text) = call(&tool, block(vec![option("a"), option("b")]));
    assert!(
        is_error && text.contains("offers 3 to 5 options, not 2"),
        "{text}"
    );
    let (is_error, text) = call(&tool, block(vec![option("a"), option("b"), option("c")]));
    assert!(!is_error, "{text}");
    assert!(
        text.contains("(blocked on user: which name?) — 1. name a · 2. name b · 3. name c"),
        "{text}"
    );
    let list = latest_record(&session).ok_or("no record")?.list;
    let asked = list
        .items()
        .find_map(|item| item.ask.as_ref())
        .ok_or("no ask")?;
    assert_eq!(asked.options.len(), 3);
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

/// Dies with a batch checked only against the list it joins: both `a` rows land, and a later
/// `rm a` takes the two.
#[test]
fn an_append_that_repeats_a_label_is_refused_and_changes_nothing() -> TestResult {
    let (_root, session) = session("dup-append")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let (is_error, text) = call(&tool, json!({"op": "init", "items": ["one"]}));
    assert!(!is_error, "{text}");
    let (before, touched) = (store.list(), store.touched());
    let (is_error, text) = call(&tool, json!({"op": "append", "items": ["a", "a"]}));
    assert!(is_error, "a repeated label was taken: {text}");
    assert!(text.contains("todo \"a\""), "{text}");
    assert_eq!((store.list(), store.touched()), (before, touched));
    Ok(())
}

#[test]
fn a_flat_init_that_repeats_a_label_is_refused() -> TestResult {
    let (_root, session) = session("dup-init")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let (is_error, text) = call(&tool, json!({"op": "init", "items": ["x", "x"]}));
    assert!(is_error, "a repeated label was taken: {text}");
    assert!(text.contains("todo \"x\""), "{text}");
    assert!(store.list().items().next().is_none());
    Ok(())
}

/// The record an older binary wrote with the label `a` twice: `t1` running, `t2` pending.
fn legacy_twins(name: &str) -> Result<(Scratch, SharedSession), Box<dyn Error>> {
    let (root, session) = session(name)?;
    let record = json!({
        "op": "append", "actor": "main", "at": 1790552725363_u64, "touched": 2,
        "list": {"format": 2, "phases": [{"name": "Tasks", "items": [
            {"id": "t1", "label": "a", "state": "running", "by": "main", "attempt": 1, "refusals": 0},
            {"id": "t2", "label": "a", "state": "pending", "attempt": 1, "refusals": 0}
        ]}], "nextId": 3}
    });
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record))?;
    Ok((root, session))
}

fn id_states(list: &TodoList) -> Vec<(String, TodoStateName)> {
    list.items()
        .map(|item| {
            let id = item.id.as_ref().map(ToString::to_string);
            (id.unwrap_or_default(), TodoStateName::of(&item.state))
        })
        .collect()
}

/// Dies with `rm` keeping only rows of another label: on a list an older binary wrote with two
/// `a` rows, `rm t2` takes `t1` with it.
#[test]
fn rm_on_a_legacy_list_with_a_repeated_label_removes_only_the_named_row() -> TestResult {
    let (_root, session) = legacy_twins("dup-legacy")?;
    let tool = TodoTool::new(store_for(&session));
    let (is_error, text) = call(&tool, json!({"op": "append", "items": ["b"]}));
    assert!(!is_error, "a legacy list must stay repairable: {text}");
    let (is_error, text) = call(&tool, json!({"op": "append", "items": ["a"]}));
    assert!(is_error, "a third `a` was taken: {text}");
    let (is_error, text) = call(&tool, json!({"op": "rm", "id": "t2"}));
    assert!(!is_error, "{text}");
    let list = store_for(&session).list();
    assert_eq!(
        ids(&list),
        vec![
            ("t1".to_owned(), "a".to_owned()),
            ("t3".to_owned(), "b".to_owned())
        ]
    );
    Ok(())
}

/// Dies with `start` demoting other rows by label, which keeps `t1` running and `t2` pending,
/// or with a `done` that names no item resolving the running row's label to `t1`.
#[test]
fn start_and_done_on_a_legacy_list_move_the_named_row_not_its_twin() -> TestResult {
    let (_root, session) = legacy_twins("dup-legacy-start")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let (is_error, text) = call(&tool, json!({"op": "start", "id": "t2"}));
    assert!(!is_error, "{text}");
    let row = |id: &str, state| (id.to_owned(), state);
    let (pending, running) = (TodoStateName::Pending, TodoStateName::Running);
    assert_eq!(
        id_states(&store.list()),
        [row("t1", pending), row("t2", running)]
    );
    let (is_error, text) = call(&tool, json!({"op": "done", "evidence": "`make` ok"}));
    assert!(!is_error, "{text}");
    let (running, done) = (TodoStateName::Running, TodoStateName::Done);
    assert_eq!(
        id_states(&store.list()),
        [row("t1", running), row("t2", done)]
    );
    Ok(())
}

/// Dies with `init` checked only against the list it replaces, which already holds `a` twice.
#[test]
fn an_init_that_repeats_a_label_is_refused_on_a_legacy_list() -> TestResult {
    let (_root, session) = legacy_twins("dup-legacy-init")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let phases = json!([{"name": "A", "items": ["a"]}, {"name": "B", "items": ["a"]}]);
    for args in [
        json!({"op": "init", "phases": phases}),
        json!({"op": "init", "items": ["a", "a"]}),
    ] {
        let (before, touched) = (store.list(), store.touched());
        let (is_error, text) = call(&tool, args);
        assert!(is_error, "a repeated label was taken: {text}");
        assert!(text.contains("todo \"a\""), "{text}");
        assert_eq!((store.list(), store.touched()), (before, touched));
    }
    Ok(())
}

/// Dies with `mint` filling only missing ids: a `set` that copies `t2` onto a second row keeps
/// two rows named `t2`, and a `done` naming no item then closes the first, a pending row.
#[test]
fn a_set_that_repeats_an_id_gives_the_copy_a_new_one() -> TestResult {
    let (_root, session) = session("dup-id")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    for args in [
        json!({"op": "init", "items": ["a", "b"]}),
        json!({"op": "set", "list": "- [ ] t2 a\n- [>] t2 b"}),
        json!({"op": "done", "evidence": "`make` ok"}),
    ] {
        let (is_error, text) = call(&tool, args);
        assert!(!is_error, "{text}");
    }
    let row = |id: &str, state| (id.to_owned(), state);
    let (running, done) = (TodoStateName::Running, TodoStateName::Done);
    assert_eq!(
        id_states(&store.list()),
        [row("t2", running), row("t3", done)]
    );
    Ok(())
}

/// Dies with a `set` row carrying the history of the first row with its label: `t2 a` takes
/// `t1`'s blocker and note.
#[test]
fn a_set_row_that_names_a_twin_by_id_keeps_that_twins_state() -> TestResult {
    let (_root, session) = legacy_twins("dup-legacy-set")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    for args in [
        json!({"op": "block", "id": "t1", "on": "user", "note": "t1 note"}),
        json!({"op": "block", "id": "t2", "on": "external", "note": "t2 note"}),
        json!({"op": "set", "list": "- [!] t2 a"}),
    ] {
        let (is_error, text) = call(&tool, args);
        assert!(!is_error, "{text}");
    }
    assert_eq!(
        text::checklist(&store.list()),
        ["- [!] t2 a (blocked on external: t2 note)"]
    );
    Ok(())
}

#[test]
fn every_argument_error_ends_with_an_id_call_that_lands() -> TestResult {
    let (_root, session) = session("example")?;
    let tool = TodoTool::new(store_for(&session));
    // In the order their examples can run on one list (a done item restarts before its next
    // done); each refusal is an argument error.
    let refusals = [
        json!({"op": "init"}),
        json!({"op": "start"}),
        json!({"op": "block"}),
        json!({"op": "unblock"}),
        json!({"label": "first task"}),
        json!({"op": "start"}),
        json!({"op": "finish"}),
        json!({"op": "start"}),
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
/// Calls a later decision answers anew (a stale view merges, a set keeps a closed row open): the
/// fixtures keep main's recorded refusal, replay skips them as that refusal did, their own tests pin them.
const DECIDED: [(&str, usize); 3] = [
    ("state-refusals", 8),
    ("set-cannot-close", 2),
    ("set-cannot-close-twice", 1),
];

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

/// A fresh session whose list is the seed's numbered lines, with the session's landed calls before `upto`
/// replayed; a refused call moved nothing, so this is the list the model saw. The scratch dir
/// comes back with the tool: the session writes into it for as long as the tool is used.
fn replayed_to(
    sequence: &Sequence,
    upto: usize,
    tag: &str,
) -> Result<(Scratch, TodoTool), Box<dyn Error>> {
    let (root, session) = session(&format!("{}-{upto}-{tag}", sequence.name))?;
    let store = store_for(&session);
    if let Some(seed) = &sequence.seed {
        let items: Vec<Todo> = seed
            .lines()
            .filter_map(|line| line.split_once(". ").map(|(_, text)| text))
            .filter_map(|text| Todo::from_text(text).ok())
            .fold(Vec::new(), |mut items, item| {
                if !items.iter().any(|seen: &Todo| seen.label == item.label) {
                    items.push(item);
                }
                items
            });
        store.apply(
            Op::Init {
                phases: vec![(PhaseName::new("Tasks")?, items)],
            },
            None,
        )?;
    }
    let tool = TodoTool::new(store);
    for row in sequence.calls.iter().take(upto) {
        if row["isError"] == false {
            replay(&tool, &row["args"]);
        }
    }
    Ok((root, tool))
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
        let (_root, tool) = replayed_to(&sequence, 0, "same")?;
        for (index, row) in sequence.calls.iter().enumerate() {
            if DECIDED.contains(&(sequence.name.as_str(), index)) {
                continue;
            }
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
    assert_eq!(calls, 86);
    Ok(())
}

#[test]
fn a_call_refused_for_its_shape_lands_as_it_meant() -> TestResult {
    let (mut landed, mut refused, mut wrong) = (0, 0, Vec::new());
    for sequence in sequences(SHAPES)? {
        for (index, row) in sequence.calls.iter().enumerate() {
            if row["isError"] == false || DECIDED.contains(&(sequence.name.as_str(), index)) {
                continue;
            }
            let (_root, tool) = replayed_to(&sequence, index, "sent")?;
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
            let (_meant_root, meant_tool) = replayed_to(&sequence, index, "meant")?;
            let (_, meant) = replay(&meant_tool, means);
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
    assert_eq!((landed, refused), (17, 7), "of the 24 refused calls left");
    Ok(())
}

fn todo_fixture(name: &str) -> Result<String, Box<dyn Error>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/todo");
    Ok(std::fs::read_to_string(dir.join(name))?)
}

/// Synthetic calls in both shapes GLM's leaked markup took in 17 refused `done` calls of the
/// 2026-09-11 v4 sweep, run through the adapters' decode and the tool after an `init`.
#[test]
fn a_done_call_with_leaked_glm_markup_lands_as_done() -> TestResult {
    let fixture: Value = serde_json::from_str(&todo_fixture("glm-leaked-calls.json")?)?;
    let (_root, session) = session("leaked")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let decode =
        |args: &Value| Value::Object(yi_ai::json_salvage::parse_streaming_json(&args.to_string()));
    for args in fixture["refused"].as_array().ok_or("refused")? {
        call(&tool, json!({"op": "init", "items": ["a standing item"]}));
        let (is_error, text) = call(&tool, decode(args));
        assert!(is_error && store.progress().open == 1, "{args}: {text}");
    }
    let lands = fixture["lands"].as_array().ok_or("lands")?;
    let mut refused = Vec::new();
    for case in lands {
        let args = &case["arguments"];
        call(&tool, json!({"op": "init", "items": [args["label"]]}));
        let (is_error, text) = call(&tool, decode(args));
        let list = store.list();
        let item = list.items().next().ok_or("the item")?;
        let landed = TodoStateName::of(&item.state) == TodoStateName::Done
            && item.evidence.as_deref() == case["evidence"].as_str()
            && text.starts_with("(op inferred: done)\n");
        if is_error || !landed {
            refused.push(format!("{args}: {text}"));
        }
    }
    assert!(
        refused.is_empty(),
        "{} of {} leaked calls did not land as done:\n{}",
        refused.len(),
        lands.len(),
        refused.join("\n")
    );
    Ok(())
}

/// Synthetic markup-free calls across every op and argument form, refusals and odd values
/// included, come back from `unleak` byte for byte, as the sweep's 2,383 such calls did.
#[test]
fn unleak_leaves_every_call_without_the_markup_byte_identical() -> TestResult {
    let mut calls = 0;
    for line in todo_fixture("well-formed-calls.jsonl")?.lines() {
        assert!(!line.contains("arg_"), "no markup: {line}");
        let args: Map<String, Value> = serde_json::from_str(line)?;
        let after = serde_json::to_string(&*unleak(&args))?;
        assert_eq!(after, serde_json::to_string(&args)?, "{line}");
        calls += 1;
    }
    assert_eq!(calls, 59);
    Ok(())
}

/// The HUD header ended mid-phrase on a long running label; the cut says it is one.
#[test]
fn the_header_marks_a_cut_running_label() -> TestResult {
    let long = "read the record (architecture, design doc, git log, changelog, gate recipe, last merges, guardrail script)";
    let mut running = Todo::from_text(long)?;
    running.state = TodoState::Running {
        by: AgentId::owner(),
    };
    assert!(running.is_cut(), "the label is cut to the max");
    let list = TodoList {
        phases: vec![yi_types::todo::TodoPhase {
            name: PhaseName::new("Tasks")?,
            items: vec![running],
            extra: Map::new(),
        }],
        ..TodoList::default()
    };
    let header = text::header(&list);
    assert!(header.ends_with('…'), "{header}");
    assert!(
        header.starts_with("Todos 0/1 · running: read the record"),
        "{header}"
    );
    Ok(())
}

/// Incident: 11 of 22 F0e `todo` refusals were a label the renderer had cut and marked with
/// an ellipsis, or the same label with its backticks dropped; the refusal then echoed the
/// stored label cut to 80 chars, which read as identical to what was sent (#474).
#[test]
fn a_label_the_renderer_cut_names_its_item_back() -> TestResult {
    let (_root, session) = session("cut-label")?;
    let tool = TodoTool::new(store_for(&session));
    let long =
        "`gateway`: the root failure behind an alert in an interleaved multi-process log file";
    let (is_error, text) = call(
        &tool,
        json!({"op": "init", "items": [long, "`billing`: the ledger"]}),
    );
    assert!(!is_error, "{text}");
    let rendered = text
        .lines()
        .find(|line| line.contains("t1"))
        .ok_or("no rendered row")?
        .to_owned();
    assert!(rendered.contains('…'), "the row is cut: {rendered}");
    // What the model copies out of the render, ellipsis and all.
    let copied = rendered
        .split_once("t1 ")
        .map(|(_, tail)| tail.to_owned())
        .ok_or("no label in the row")?;
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "label": copied, "evidence": "`python3 check.py gateway` -> ok"}),
    );
    assert!(!is_error, "{text}");

    let (is_error, text) = call(
        &tool,
        json!({"op": "start", "label": "billing: the ledger"}),
    );
    assert!(!is_error, "backticks dropped still names it: {text}");
    // Three confirmation calls sent the id and the request's whole line the label was cut from.
    let (is_error, text) = call(&tool, json!({"op": "init", "items": [long]}));
    assert!(!is_error, "{text}");
    let (is_error, text) = call(&tool, json!({"op": "start", "label": format!("t1 {long}")}));
    assert!(!is_error, "the id and the uncut line name it: {text}");
    Ok(())
}

/// Incident: four F0e `done` calls put the op in the key with the id as its value; that has
/// one reading, while `op is required` cost the whole composed evidence string (#474).
#[test]
fn an_op_passed_as_a_key_lands_as_that_op() -> TestResult {
    let (_root, session) = session("op-key")?;
    let tool = TodoTool::new(store_for(&session));
    let (is_error, text) = call(&tool, json!({"op": "init", "items": ["one", "two"]}));
    assert!(!is_error, "{text}");
    let (is_error, text) = call(
        &tool,
        json!({"done": "t1", "evidence": "`python3 check.py cronnext` -> ok 9 of 9 pass"}),
    );
    assert!(!is_error, "{text}");
    let list = latest_record(&session).ok_or("no record")?.list;
    let first = list.items().next().ok_or("no items")?;
    assert_eq!(TodoStateName::of(&first.state), TodoStateName::Done);
    // A set's own argument is not an id, so the repair stays off the ops that take a list.
    let (is_error, text) = call(&tool, json!({"set": "- [ ] rebuilt"}));
    assert!(
        is_error,
        "an op with no id argument is still refused: {text}"
    );
    Ok(())
}

/// Dies with a done that names no item closing every open item, or refused for evidence
/// in the name of t1 that was already done, instead of landing on the one running item.
#[test]
fn a_done_naming_no_item_lands_on_the_running_one() -> TestResult {
    let (_root, session) = session("done-running")?;
    let store = store_for(&session);
    let tool = TodoTool::new(store.clone());
    store.apply(
        Op::Set {
            list: "- [x] a\n- [>] b\n- [ ] c\n".to_owned(),
        },
        None,
    )?;
    let (is_error, text) = call(&tool, json!({"op": "done"}));
    assert!(is_error && text.contains("proves \"b\""), "{text}");
    let (is_error, text) = call(&tool, json!({"op": "done", "evidence": "`make` ok"}));
    assert!(!is_error, "{text}");
    let list = store.list();
    let done = |name: &str| {
        list.items().any(|item| {
            item.label.as_str() == name && TodoStateName::of(&item.state) == TodoStateName::Done
        })
    };
    assert!(done("b") && !done("c"), "{:?}", states(&list));
    let mut two = list.clone();
    two.for_each_mut(|item| {
        item.state = TodoState::Running {
            by: AgentId::owner(),
        }
    });
    store.replace_with(|_| Some(two), "engine");
    let (is_error, text) = call(&tool, json!({"op": "done", "evidence": "`make` ok"}));
    assert!(
        is_error && text.contains("t1") && text.contains("t3"),
        "{text}"
    );
    Ok(())
}

/// A repeated done for an item already done lands as a view with a note, not the
/// TodoError::Illegal the store alone returns.
#[test]
fn a_done_sent_again_for_a_done_item_is_a_view_with_a_note() -> TestResult {
    let (_root, session) = session("done-again")?;
    let store = store_for(&session);
    let tool = TodoTool::new(store.clone());
    call(&tool, json!({"op": "init", "items": ["one"]}));
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "label": "t1", "evidence": "`make` ok"}),
    );
    assert!(!is_error, "{text}");
    let (is_error, text) = call(
        &tool,
        json!({"op": "done", "label": "t1", "evidence": "`make` ok"}),
    );
    assert!(
        !is_error && text.contains("it was already done, so nothing changed"),
        "{text}"
    );
    Ok(())
}

/// Dies with the session list, the common case with no plan open, unblocking an ask nobody has
/// answered, or leaving the user's pick unrecorded once they reply.
#[test]
fn a_session_todos_ask_waits_for_the_reply_and_records_the_pick() -> TestResult {
    let (_root, session) = session("asked")?;
    let typed = |text: &str| {
        yi_types::message::AgentMessage::user_input(
            yi_types::message::UserContent::Text(text.to_owned()),
            0,
        )
    };
    yi_session::lock_session(&session).append_message("main", typed("name the crate"))?;
    let tool = TodoTool::new(store_for(&session));
    call(&tool, json!({"op": "init", "items": ["pick a name"]}));
    let option = |id: &str| json!({"id": id, "label": format!("name {id}")});
    let options = vec![option("a"), option("b"), option("c")];
    let block =
        json!({"op": "block", "id": "t1", "on": "user", "note": "which?", "options": options});
    let (is_error, text) = call(&tool, block);
    assert!(!is_error, "{text}");
    let unblock = json!({"op": "unblock", "id": "t1"});
    let (is_error, text) = call(&tool, unblock.clone());
    assert!(
        is_error && text.contains("waits on the user's pick"),
        "{text}"
    );
    yi_session::lock_session(&session).append_message("main", typed("2"))?;
    let (is_error, text) = call(&tool, unblock);
    assert!(!is_error, "{text}");
    let list = latest_record(&session).ok_or("no record")?.list;
    let item = list.items().next().ok_or("no item")?;
    let answer = item.ask.as_ref().and_then(|ask| ask.answer.as_ref());
    assert_eq!(
        answer.map(|answer| (answer.address.to_string(), answer.option.clone())),
        Some(("user://2".to_owned(), Some("b".to_owned().try_into()?)))
    );
    assert!(
        item.cites
            .intent
            .iter()
            .any(|url| url.to_string() == "user://2")
    );
    Ok(())
}

/// The tool refuses an address nothing can serve, naming why, and keeps one it can.
#[test]
fn a_block_on_a_clock_address_is_checked_before_it_waits() -> TestResult {
    let (_root, session) = session("clock-wait")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    call(&tool, json!({"op": "set", "list": "- [ ] ship at nine"}));

    let (refused, text) = call(
        &tool,
        json!({"op": "block", "id": "t1", "on": "clock://at tomorrow", "note": "wait"}),
    );
    assert!(refused, "a clock address with no time was taken: {text}");
    assert!(text.contains("Invalid one-shot schedule"), "{text}");
    let (refused, text) = call(
        &tool,
        json!({"op": "block", "id": "t1", "on": "ci://apex/main", "note": "wait"}),
    );
    assert!(
        refused && text.contains("no adapter for ci://"),
        "a channel nothing feeds was taken: {text}"
    );

    let (refused, text) = call(
        &tool,
        json!({"op": "block", "id": "t1", "on": "clock://at 2030-01-01T09:00Z", "note": "wait"}),
    );
    assert!(!refused, "{text}");
    let list = store.list();
    let blocked = list.items().next().map(|todo| todo.state.clone());
    assert_eq!(
        blocked,
        Some(TodoState::Blocked {
            on: BlockedOn::Channel {
                address: "clock://at 2030-01-01T09:00Z".to_owned(),
                filter: None,
            },
            note: "wait".to_owned(),
        })
    );
    Ok(())
}

/// Dies with a todo refusal that names no class: the tool-failure census counts misreads and
/// stale views toward zero by `details.errorKind`, so an untagged refusal is invisible to it.
#[test]
fn every_todo_refusal_names_its_class() -> Result<(), Box<dyn Error>> {
    let (_scratch, session) = session("todo-kinds")?;
    let tool = TodoTool::new(store_for(&session));
    let kind = |args: Value| crate::support::refusal_kind(&tool, args);
    assert_eq!(kind(json!({"op": "frobnicate"})), "invalid_args");
    assert_eq!(
        kind(json!({"op": "done", "label": "ghost", "evidence": "x"})),
        "stale"
    );
    let set = tool.execute(
        json!({"op": "set", "list": "- [ ] first\n- [ ] second"})
            .as_object()
            .cloned()
            .unwrap_or_default(),
        &ToolContext::new(std::env::temp_dir()),
    );
    assert!(!set.is_error, "{:?}", set.result.content);
    assert_eq!(kind(json!({"op": "append", "items": ["first"]})), "verdict");
    assert_eq!(
        kind(json!({"op": "append", "items": []})),
        "invalid_args",
        "an empty append is an argument shape, not state gone stale"
    );
    assert_eq!(
        kind(json!({"op": "done", "label": "first"})),
        "verdict",
        "done without evidence is the rule saying no, as on the plan tool"
    );
    assert_eq!(
        kind(json!({"op": "set", "list": format!("# {}\n- [ ] a", "p".repeat(100))})),
        "verdict",
        "a phase name past its cap is a cap saying no, as the plan tool's label cap is"
    );
    Ok(())
}

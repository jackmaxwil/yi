//! A todo row carries a check that `done` runs: a red keeps the row open, a green closes it with
//! the command as its evidence, and only `drop` with a reason releases a red row.

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, SharedSession};
use yi_runtime::todo::tool::TodoTool;
use yi_runtime::todo::{TodoStore, latest_record};
use yi_tools::{Tool, ToolContext};
use yi_types::todo::TodoList;

type TestResult = Result<(), Box<dyn Error>>;

fn session(name: &str) -> Result<(Scratch, SharedSession), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-check-{name}"))?;
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

fn call(tool: &TodoTool, cwd: &Path, args: Value) -> (bool, String) {
    let input = args.as_object().cloned().unwrap_or_default();
    let output = tool.execute(input, &ToolContext::new(cwd.to_path_buf()));
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    (output.is_error, text)
}

fn armed(tool: &TodoTool, args: Value) -> Vec<String> {
    tool.arms(&args.as_object().cloned().unwrap_or_default())
}

fn row(session: &SharedSession, needle: &str) -> Result<yi_types::plan::doc::Todo, Box<dyn Error>> {
    let list: TodoList = latest_record(session).ok_or("no record")?.list;
    let item = list
        .items()
        .find(|item| item.label.as_str() == needle)
        .ok_or("no such row")?;
    Ok(item.clone())
}

#[test]
fn a_red_check_keeps_the_row_running_and_a_passing_one_closes_it() -> TestResult {
    let (root, session) = session("red-green")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    let (error, text) = call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    assert!(!error, "{text}");
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "test -s out.txt"}),
    );
    assert!(!error, "{text}");
    let (error, text) = call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    assert!(!error, "{text}");
    assert!(text.contains("has not passed"), "{text}");
    assert!(text.contains("[>] t1 ship it"), "{text}");
    assert_eq!(
        yi_types::plan::doc::TodoStateName::of(&row(&session, "ship it")?.state),
        yi_types::plan::doc::TodoStateName::Running
    );
    std::fs::write(cwd.join("out.txt"), b"built")?;
    let (error, text) = call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    assert!(!error, "{text}");
    let closed = row(&session, "ship it")?;
    assert_eq!(
        yi_types::plan::doc::TodoStateName::of(&closed.state),
        yi_types::plan::doc::TodoStateName::Done
    );
    assert_eq!(
        closed.evidence.map(|evidence| evidence.to_string()),
        Some("check `test -s out.txt` passed".to_owned())
    );
    Ok(())
}

#[test]
fn drop_of_a_red_row_says_it_was_dropped_while_red() -> TestResult {
    let (root, session) = session("drop-red")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "test -s out.txt"}),
    );
    call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "drop", "label": "ship it", "reason": "out of scope"}),
    );
    assert!(!error, "{text}");
    assert!(
        text.contains("dropped while red: test -s out.txt"),
        "{text}"
    );
    Ok(())
}

#[test]
fn rm_and_a_set_omitting_a_red_row_are_refused_naming_drop() -> TestResult {
    let (root, session) = session("refuse-remove")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "test -s out.txt"}),
    );
    call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    let (error, text) = call(&tool, &cwd, json!({"op": "rm", "label": "ship it"}));
    assert!(error, "{text}");
    assert!(text.contains("`drop ship it`"), "{text}");
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "set", "list": "- [ ] something else"}),
    );
    assert!(error, "{text}");
    assert!(text.contains("`drop ship it`"), "{text}");
    assert!(row(&session, "ship it").is_ok(), "the red row survived");
    Ok(())
}

#[test]
fn a_set_that_keeps_the_row_keeps_its_check() -> TestResult {
    let (root, session) = session("keep-check")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "test -s out.txt"}),
    );
    assert!(!error, "{text}");
    let (error, text) = call(&tool, &cwd, json!({"op": "set", "list": "- [>] ship it"}));
    assert!(!error, "{text}");
    let check = yi_types::todo::RowCheck::of(&row(&session, "ship it")?).ok_or("the check went")?;
    assert_eq!(check.command, "test -s out.txt");
    Ok(())
}

#[test]
fn a_start_refuses_a_check_whose_program_is_missing_before_it_writes() -> TestResult {
    let (root, session) = session("probe")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "touch m && nosuchprog"}),
    );
    assert!(error, "{text}");
    assert!(text.contains("nosuchprog"), "{text}");
    assert!(!cwd.join("m").exists(), "the refused check wrote its file");
    Ok(())
}

#[test]
fn arms_carry_the_command_a_start_and_a_done_will_run() -> TestResult {
    let (root, session) = session("arms")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    assert_eq!(
        armed(
            &tool,
            json!({"op": "start", "label": "ship it", "check": "make check"})
        ),
        ["make check"]
    );
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "make check"}),
    );
    assert_eq!(
        armed(&tool, json!({"op": "done", "label": "ship it"})),
        ["make check"]
    );
    Ok(())
}

#[test]
fn a_unchanged_red_keeps_the_fingerprint_and_a_moved_one_changes_it() -> TestResult {
    let (root, session) = session("fingerprint")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "cat tally && false"}),
    );
    std::fs::write(cwd.join("tally"), b"1")?;
    call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    let first: TodoList = latest_record(&session).ok_or("no record")?.list;
    let before: yi_types::todo::CheckLast = yi_types::todo::RowCheck::of(
        first
            .items()
            .find(|item| item.label.as_str() == "ship it")
            .ok_or("no row")?,
    )
    .ok_or("no check")?
    .last;
    assert!(before.digest().is_some(), "{}", before.as_str());
    call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    let same: TodoList = latest_record(&session).ok_or("no record")?.list;
    assert_eq!(first.fingerprint(), same.fingerprint());
    std::fs::write(cwd.join("tally"), b"2")?;
    call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    let moved: TodoList = latest_record(&session).ok_or("no record")?.list;
    assert_ne!(first.fingerprint(), moved.fingerprint());
    Ok(())
}

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

#[test]
fn a_start_whose_check_is_not_a_string_is_refused_not_dropped() -> TestResult {
    let (root, session) = session("blank")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    for bad in [json!(5), json!(["true"]), json!({"cmd": "true"})] {
        let (error, text) = call(
            &tool,
            &cwd,
            json!({"op": "start", "label": "ship it", "check": bad}),
        );
        assert!(error, "check {bad} was accepted: {text}");
        assert!(text.contains("check"), "{text}");
        assert!(
            yi_types::todo::RowCheck::of(&row(&session, "ship it")?).is_none(),
            "{bad}"
        );
    }
    Ok(())
}

#[test]
fn a_container_check_is_not_probed_on_the_host() -> TestResult {
    let (root, session) = session("container-probe")?;
    let checker = yi_runtime::goal::Checker::new(
        None,
        None,
        yi_runtime::wall::Wall {
            container: Some("yi-image".to_owned()),
            ..yi_runtime::wall::Wall::default()
        },
    );
    let tool = TodoTool::new(store_for(&session)).with_checker(checker);
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "yidog-image-only --check"}),
    );
    assert!(!error, "{text}");
    let check = yi_types::todo::RowCheck::of(&row(&session, "ship it")?).ok_or("no check")?;
    assert_eq!(check.command, "yidog-image-only --check");
    Ok(())
}

fn drop_red(tool: &TodoTool, cwd: &Path) {
    call(tool, cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        tool,
        cwd,
        json!({"op": "start", "label": "ship it", "check": "false"}),
    );
    call(tool, cwd, json!({"op": "done", "label": "ship it"}));
    call(
        tool,
        cwd,
        json!({"op": "drop", "label": "ship it", "reason": "out of scope"}),
    );
}

#[test]
fn a_dropped_red_row_can_be_removed() -> TestResult {
    let (root, session) = session("drop-rm")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    drop_red(&tool, &cwd);
    let (error, text) = call(&tool, &cwd, json!({"op": "rm", "label": "ship it"}));
    assert!(!error, "{text}");
    assert!(row(&session, "ship it").is_err());
    Ok(())
}

#[test]
fn a_dropped_red_row_can_be_omitted_and_is_not_reported_as_open() -> TestResult {
    let (root, session) = session("drop-set")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    drop_red(&tool, &cwd);
    let list = latest_record(&session).ok_or("no record")?.list;
    let stopped = yi_runtime::todo::coupling::intercept_text(1, &list);
    assert!(!stopped.contains("still red"), "{stopped}");
    let (error, text) = call(&tool, &cwd, json!({"op": "set", "list": "- [ ] fresh"}));
    assert!(!error, "{text}");
    assert!(row(&session, "ship it").is_err());
    Ok(())
}

#[test]
fn a_done_scope_does_not_rerun_a_terminal_rows_check() -> TestResult {
    let (root, session) = session("terminal-check")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(
        &tool,
        &cwd,
        json!({"op": "set", "list": "- [ ] checked\n- [ ] plain"}),
    );
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "checked", "check": "test -s out.txt"}),
    );
    std::fs::write(cwd.join("out.txt"), b"built")?;
    call(&tool, &cwd, json!({"op": "done", "label": "checked"}));
    std::fs::remove_file(cwd.join("out.txt"))?;
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "done", "phase": "Tasks", "evidence": "`true` exit 0"}),
    );
    assert!(!error, "{text}");
    assert!(!text.contains("has not passed"), "{text}");
    let checked = row(&session, "checked")?;
    let last = yi_types::todo::RowCheck::of(&checked)
        .ok_or("no check")?
        .last;
    assert!(last.is_green(), "{}", last.as_str());
    assert_eq!(
        checked.evidence.map(|evidence| evidence.to_string()),
        Some("check `test -s out.txt` passed".to_owned())
    );
    assert_eq!(
        yi_types::plan::doc::TodoStateName::of(&row(&session, "plain")?.state),
        yi_types::plan::doc::TodoStateName::Done
    );
    Ok(())
}

#[test]
fn done_evidence_names_the_command_that_ran() -> TestResult {
    let (root, session) = session("evidence-race")?;
    let store = store_for(&session);
    let tool = TodoTool::new(Arc::clone(&store));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "true"}),
    );
    let pending = Arc::new(std::sync::Mutex::new(None));
    let next = Arc::clone(&pending);
    store.set_resync(Arc::new(move |_| {
        next.lock().ok().and_then(|mut list| list.take())
    }));
    let next = Arc::clone(&pending);
    store.on_change(Arc::new(move |list| {
        let green = list.items().any(|item| {
            item.label.as_str() == "ship it"
                && !item.state.is_terminal()
                && yi_types::todo::RowCheck::of(item).is_some_and(|check| check.last.is_green())
        });
        if !green {
            return;
        }
        let mut changed = list.clone();
        changed.for_each_mut(|item| {
            if item.label.as_str() == "ship it" {
                yi_types::todo::RowCheck {
                    command: "printf forged".to_owned(),
                    last: yi_types::todo::CheckLast::green(),
                }
                .put(item);
            }
        });
        if let Ok(mut slot) = next.lock() {
            *slot = Some(changed);
        }
    }));
    let (error, text) = call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    assert!(!error, "{text}");
    assert_eq!(
        row(&session, "ship it")?
            .evidence
            .map(|evidence| evidence.to_string()),
        Some("check `true` passed".to_owned())
    );
    Ok(())
}

#[test]
fn a_done_the_list_refuses_runs_no_check() -> TestResult {
    let (root, session) = session("refused-done")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "set", "list": "- [ ] ship it\n  - [ ] write it"}),
    );
    assert!(!error, "{text}");
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "touch ran.txt"}),
    );
    assert!(!error, "{text}");
    let (error, text) = call(&tool, &cwd, json!({"op": "done", "label": "ship it"}));
    assert!(error, "{text}");
    assert!(text.contains("write it"), "{text}");
    assert!(
        !cwd.join("ran.txt").exists(),
        "the refused done ran its check"
    );
    Ok(())
}

#[test]
fn a_second_start_naming_another_check_is_refused_and_keeps_the_first() -> TestResult {
    let (root, session) = session("rewrite-start")?;
    let tool = TodoTool::new(store_for(&session));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd)?;
    call(&tool, &cwd, json!({"op": "set", "list": "- [ ] ship it"}));
    call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "test -s out.txt"}),
    );
    let (error, text) = call(
        &tool,
        &cwd,
        json!({"op": "start", "label": "ship it", "check": "true"}),
    );
    assert!(error, "{text}");
    assert!(text.contains("test -s out.txt"), "{text}");
    let check = yi_types::todo::RowCheck::of(&row(&session, "ship it")?).ok_or("the check went")?;
    assert_eq!(check.command, "test -s out.txt");
    Ok(())
}

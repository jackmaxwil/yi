//! The three bounds on a console-run kernel cell, each asserted where deleting the
//! control fails the test: the size cap, the queue cap, and what a close takes down.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use yi_acp::cells::{MAX_CELL_BYTES, MAX_QUEUED_CELLS, UserCell, UserCells, cell_code};

type TestResult = Result<(), Box<dyn Error>>;

/// The handler's `(code, message)` pair is not an `Error`; a test that meant to succeed
/// says so by naming the call that failed.
fn ok<T>(what: &str, got: Result<T, (i64, String)>) -> Result<T, String> {
    got.map_err(|(code, message)| format!("{what}: {code} {message}"))
}

/// A task that never finishes on its own, so the queue sees it as still running and only
/// an abort can end it.
fn pending_cell(call_id: &str) -> (UserCell, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(std::future::pending::<()>());
    (
        UserCell {
            call_id: call_id.to_owned(),
            cancelled: Arc::clone(&cancelled),
            task,
        },
        cancelled,
    )
}

#[test]
fn a_cell_past_the_size_cap_is_refused_before_anything_spawns() -> TestResult {
    let at_cap = "x".repeat(MAX_CELL_BYTES);
    assert_eq!(
        ok("a cell at the cap", cell_code(&json!({ "code": at_cap })))?.len(),
        MAX_CELL_BYTES
    );
    let over = "x".repeat(MAX_CELL_BYTES.saturating_add(1));
    let (code, message) = cell_code(&json!({ "code": over }))
        .err()
        .ok_or("a cell one byte over the cap must be refused")?;
    assert_eq!(code, -32602);
    assert!(message.contains("64 KB"), "{message}");
    assert_eq!(
        ok("a missing code", cell_code(&json!({})))?,
        "",
        "a missing code is an empty cell"
    );
    Ok(())
}

#[tokio::test]
async fn the_queue_refuses_a_ninth_cell_and_a_finished_one_makes_room() -> TestResult {
    let mut cells = UserCells::default();
    for n in 1..=MAX_QUEUED_CELLS {
        let id = ok("admit", cells.admit("s1"))?;
        assert_eq!(id, format!("user-{n}"), "call ids count up");
        let (cell, _) = pending_cell(&id);
        cells.track("s1", cell);
    }
    let (code, message) = cells
        .admit("s1")
        .err()
        .ok_or("the ninth cell must be refused")?;
    assert_eq!(code, -32000);
    assert!(message.contains("kernel busy"), "{message}");

    // A second session has its own queue: one busy kernel never blocks another.
    assert_eq!(
        ok("a second session", cells.admit("s2"))?,
        format!("user-{}", MAX_QUEUED_CELLS.saturating_add(1))
    );

    // Retiring a finished cell is what makes room, so admit must look before it counts.
    let done = tokio::spawn(async {});
    cells.track(
        "s3",
        UserCell {
            call_id: "user-done".to_owned(),
            cancelled: Arc::new(AtomicBool::new(false)),
            task: done,
        },
    );
    tokio::task::yield_now().await;
    ok("admit after a cell finished", cells.admit("s3"))?;
    assert_eq!(cells.queued("s3"), 0, "the finished cell was retired");
    Ok(())
}

#[tokio::test]
async fn a_cancel_reaches_only_the_named_cell() -> TestResult {
    let mut cells = UserCells::default();
    let (first, first_flag) = pending_cell("user-1");
    let (second, second_flag) = pending_cell("user-2");
    cells.track("s1", first);
    cells.track("s1", second);

    assert!(cells.cancel("s1", "user-1"));
    assert!(first_flag.load(Ordering::SeqCst));
    assert!(!second_flag.load(Ordering::SeqCst), "its neighbour runs on");
    assert!(!cells.cancel("s1", "user-9"), "an unknown cell is not ours");
    assert!(!cells.cancel("s9", "user-1"), "nor is another session's");
    cells.close("s1");
    Ok(())
}

#[tokio::test]
async fn closing_a_session_cancels_and_aborts_every_cell_it_owns() -> TestResult {
    let mut cells = UserCells::default();
    let (mine, my_flag) = pending_cell("user-1");
    let (theirs, their_flag) = pending_cell("user-2");
    let my_task = mine.task.abort_handle();
    cells.track("s1", mine);
    cells.track("s2", theirs);

    cells.close("s1");
    tokio::task::yield_now().await;
    assert!(my_flag.load(Ordering::SeqCst), "the flag is set first");
    assert!(
        my_task.is_finished(),
        "and the handle is never left running"
    );
    assert_eq!(cells.queued("s1"), 0);
    assert!(
        !their_flag.load(Ordering::SeqCst),
        "another session is spared"
    );
    assert_eq!(cells.queued("s2"), 1);
    cells.close("s2");
    Ok(())
}

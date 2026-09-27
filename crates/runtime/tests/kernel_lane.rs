//! One kernel, two writers. The agent's tool and a console-run cell share a session's
//! kernel, and `KernelManager::execute` holds `execution_queue` across the whole cell —
//! that lock, not anything above it, is why the second caller waits rather than landing
//! in the busy-reuse path and interrupting work that is still running.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
use yi_tools::CancelFlag;

type TestResult = Result<(), Box<dyn Error>>;

fn service() -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }))
}

fn never_cancelled() -> CancelFlag {
    Arc::new(|| false)
}

/// The cell's own stdout, which is where the ordering is legible.
fn stdout_of(output: &yi_tools::ToolOutput) -> String {
    output
        .result
        .details
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_second_cell_queues_behind_the_first_instead_of_interrupting_it() -> TestResult {
    let service = service();
    // Boot the kernel first: the wait for a cold start would otherwise land inside the
    // window this test measures.
    let warm = service
        .execute_user_cell("print('warm')", &never_cancelled())
        .await;
    assert!(!warm.is_error, "the kernel never booted: {warm:?}");

    let slow = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let out = service
                .execute_user_cell(
                    "import time\ntime.sleep(1)\nprint('slow')",
                    &never_cancelled(),
                )
                .await;
            (Instant::now(), out)
        })
    };
    // Long enough that the slow cell holds the kernel, short enough to stay inside its sleep.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let quick = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let out = service
                .execute_user_cell("print('quick')", &never_cancelled())
                .await;
            (Instant::now(), out)
        })
    };

    let (slow_at, slow_out) = slow.await?;
    let (quick_at, quick_out) = quick.await?;
    service.dispose().await;

    assert!(
        !slow_out.is_error,
        "the first cell ran to completion rather than being interrupted: {:?}",
        slow_out.result
    );
    assert!(
        !quick_out.is_error,
        "the second cell ran: {:?}",
        quick_out.result
    );
    assert!(
        stdout_of(&slow_out).contains("slow"),
        "the first cell printed: {:?}",
        stdout_of(&slow_out)
    );
    assert!(
        stdout_of(&quick_out).contains("quick"),
        "the second cell printed: {:?}",
        stdout_of(&quick_out)
    );
    assert!(
        slow_at <= quick_at,
        "the queue is first in, first out: the second cell finished first"
    );
    Ok(())
}

fn cancelled_after(delay: Duration) -> CancelFlag {
    let at = Instant::now() + delay;
    Arc::new(move || Instant::now() >= at)
}

fn text_of(output: &yi_tools::ToolOutput) -> String {
    output
        .result
        .content
        .iter()
        .map(|content| match content {
            yi_types::message::Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

/// The field shape: a cell polling with `await asyncio.sleep(30)`, cancelled by its caller.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_cancelled_awaiting_cell_leaves_the_namespace_standing() -> TestResult {
    let service = service();
    let set = service
        .execute_user_cell("keep = 42", &never_cancelled())
        .await;
    assert!(!set.is_error, "{:?}", set.result);
    let poll = "import asyncio\nwhile True:\n    await asyncio.sleep(30)";
    let stopped = service
        .execute_user_cell(poll, &cancelled_after(Duration::from_secs(1)))
        .await;
    assert!(stopped.is_error, "{:?}", stopped.result);
    let after = service
        .execute_user_cell("print(keep)", &never_cancelled())
        .await;
    service.dispose().await;
    assert_eq!(stdout_of(&after), "42\n", "{}", text_of(&after));
    assert_eq!(after.result.details["kernelRestarted"], false);
    Ok(())
}

/// A cell deaf to SIGINT outlives the busy window, so the kernel has to go; the state says so
/// until the next cell.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_forced_restart_names_what_it_lost_in_the_result_and_the_state() -> TestResult {
    let service = service();
    let set = service
        .execute_user_cell("keep = 42\nalso = 7", &never_cancelled())
        .await;
    assert!(!set.is_error, "{:?}", set.result);
    let deaf = "import signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\ntime.sleep(60)";
    let _ = service
        .execute_user_cell(deaf, &cancelled_after(Duration::from_secs(1)))
        .await;
    let after = service
        .execute_user_cell("print('fresh')", &never_cancelled())
        .await;
    let state = service.state();
    let _ = service.execute_user_cell("1", &never_cancelled()).await;
    let settled = service.state();
    service.dispose().await;
    let text = text_of(&after);
    assert_eq!(after.result.details["kernelRestarted"], true, "{text}");
    assert!(
        text.contains("[IPython kernel was restarted; 2 names lost: also, keep."),
        "{text}"
    );
    assert!(state.contains("restarted; 2 names lost"), "{state}");
    assert!(
        !settled.contains("restarted"),
        "a later cell left the note: {settled}"
    );
    Ok(())
}

/// The names the state line offers are checked against the live modules, not a copy of them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "tier-2 journey: `just journeys`"]
async fn the_first_cell_shows_the_rlm_table_once_and_the_state_names_the_surface() -> TestResult {
    let service = service();
    let before = service.state();
    assert!(
        before.contains("help(rlm) and help(yi) are exact"),
        "{before}"
    );
    let offered = |module: &str| -> Vec<String> {
        before
            .split(&format!("{module}: "))
            .nth(1)
            .and_then(|rest| rest.split(" · ").next())
            .map(|names| names.split(", ").map(|name| format!("{name:?}")).collect())
            .unwrap_or_default()
    };
    let (rlm, yi) = (offered("rlm"), offered("yi"));
    assert!(!rlm.is_empty() && !yi.is_empty(), "{before}");
    let check = format!(
        "import yi\nprint(all(hasattr(rlm, n) for n in [{}]) and all(hasattr(yi, n) for n in [{}]))",
        rlm.join(", "),
        yi.join(", ")
    );
    let first = service.execute_user_cell(&check, &never_cancelled()).await;
    let second = service.execute_user_cell("1", &never_cancelled()).await;
    let after = service.state();
    service.dispose().await;
    assert_eq!(stdout_of(&first), "True\n", "{}", text_of(&first));
    let table = text_of(&first);
    assert!(table.contains("[rlm, shown once per session;"), "{table}");
    assert!(table.contains("await rlm.send(target, message,"), "{table}");
    assert!(table.contains("\nrlm.bash(command)"), "{table}");
    assert!(
        !text_of(&second).contains("shown once per session"),
        "{}",
        text_of(&second)
    );
    assert!(!after.contains("help(rlm)"), "{after}");
    Ok(())
}

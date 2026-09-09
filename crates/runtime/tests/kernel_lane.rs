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
        sandbox: None,
        snapshot_key: None,
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

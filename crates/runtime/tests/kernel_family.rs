//! A child reading `kernel://main/<var>` while the parent runs a cell is refused, and the
//! refusal names the route that works: the parent puts the value, the child gets it.
use crate::kernel_across_sessions::{bare_service, printed, read_main, root_session};
use crate::scratch::Scratch;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult = Result<(), Box<dyn Error>>;

type Cell = tokio::task::JoinHandle<Result<yi_tools::KernelCellOutcome, String>>;

fn run_cell(
    service: &Arc<yi_runtime::KernelService>,
    stop: &Arc<AtomicBool>,
    code: &'static str,
) -> Cell {
    let (service, stop) = (Arc::clone(service), Arc::clone(stop));
    tokio::task::spawn_blocking(move || {
        let cancelled: yi_tools::CancelFlag = Arc::new(move || stop.load(Ordering::SeqCst));
        yi_tools::KernelBridge::execute_cell(service.as_ref(), code, "", &cancelled, None)
    })
}

async fn wait_for_file(path: &std::path::Path) {
    for _ in 0..600 {
        if path.is_file() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

const SLEEP: &str = "import time\nopen('sleeping', 'w').close()\ntime.sleep(20)";

#[tokio::test]
async fn a_busy_refusal_names_the_put_and_get_route() -> TestResult {
    let root = Scratch::new("yi-kernel-busy-names-put")?;
    let service = bare_service(&root)?;
    let stop = Arc::new(AtomicBool::new(false));
    run_cell(&service, &stop, "x = 1").await??;
    let sleeper = run_cell(&service, &stop, SLEEP);
    wait_for_file(&root.join("sleeping")).await;
    let read = read_main(&root, &service, "x")?.await?;
    stop.store(true, Ordering::SeqCst);
    let _interrupted = sleeper.await?;
    service.dispose().await;
    let error = read
        .err()
        .ok_or("a read of a kernel mid-cell returned a value")?;
    let message = error.to_string();
    assert!(
        message.contains("rlm.put") && message.contains("rlm.get"),
        "the refusal did not name the remedy: {message}"
    );
    Ok(())
}

#[tokio::test]
async fn a_value_put_before_the_parent_awaits_is_read_by_the_child() -> TestResult {
    let root = Scratch::new("yi-kernel-put-then-wait")?;
    let (_parent_session, parent) = root_session(&root, "rlm-1", "family")?;
    let (_child_session, child) = root_session(&root, "rlm-2", "family")?;
    printed(&parent, "rlm.put('x', 'from the parent')").await?;
    let stop = Arc::new(AtomicBool::new(false));
    let sleeper = run_cell(&parent, &stop, SLEEP);
    wait_for_file(&root.join("sleeping")).await;
    let seen = printed(&child, "print(rlm.get('x'))").await?;
    stop.store(true, Ordering::SeqCst);
    let _interrupted = sleeper.await?;
    for kernel in [parent, child] {
        kernel.dispose().await;
    }
    assert!(
        seen.contains("from the parent"),
        "the child could not read the put value: {seen}"
    );
    Ok(())
}

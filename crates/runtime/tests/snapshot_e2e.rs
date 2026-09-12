#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions, restore_notice_text};
use yi_tools::{CancelFlag, KernelBridge};

type TestResult = Result<(), Box<dyn Error>>;

async fn cell(
    service: &Arc<KernelService>,
    code: &'static str,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn service(session_dir: &std::path::Path, notices: &Arc<Mutex<Vec<String>>>) -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    let notices = Arc::clone(notices);
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: Some(session_dir.to_path_buf()),
        family_dir: None,
        host: Arc::new(registry),
        on_restore: Some(Arc::new(move |restore| {
            if let Ok(mut queue) = notices.lock() {
                queue.push(restore_notice_text(restore));
            }
        })),
        sandbox: None,
        snapshot_key: None,
    }))
}

#[tokio::test]
async fn session_dir_snapshot_revives_through_the_service() -> TestResult {
    let dir = Scratch::new("yi-snap-svc")?;
    let notices = Arc::new(Mutex::new(Vec::new()));

    let first = service(&dir, &notices);
    let outcome = cell(&first, "answer = 42")
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(outcome.result.status, yi_types::kernel::ExecuteStatus::Ok);
    first.dispose().await;
    assert!(
        notices
            .lock()
            .map(|queue| queue.is_empty())
            .unwrap_or(false),
        "no snapshot existed, so the first boot must not announce a restore"
    );
    assert!(
        dir.join("kernel-state.dill").is_file(),
        "dispose must flush a final snapshot to the session dir"
    );

    let second = service(&dir, &notices);
    let outcome = cell(&second, "print(answer)")
        .await
        .map_err(|e| e.to_string())?;
    assert!(
        outcome.result.stdout.contains("42"),
        "a fresh kernel must revive the prior namespace: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    let announced = notices
        .lock()
        .map(|queue| queue.join("\n"))
        .unwrap_or_default();
    assert!(
        announced.contains("<ipython_state_restored>") && announced.contains("answer"),
        "the model must be told which names were revived, only after bootstrap: {announced}"
    );
    second.dispose().await;
    Ok(())
}

#[tokio::test]
async fn post_compaction_sync_prunes_and_reports_names() -> TestResult {
    let dir = Scratch::new("yi-sync-svc")?;
    let notices = Arc::new(Mutex::new(Vec::new()));
    let service = service(&dir, &notices);

    assert!(
        service.sync_after_compaction().await.is_none(),
        "sync must be a peek, never a boot: no kernel, no notice"
    );

    let outcome = cell(&service, "kept = 1")
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(outcome.result.status, yi_types::kernel::ExecuteStatus::Ok);

    let notice = service.sync_after_compaction().await.ok_or("sync notice")?;
    // A listing that times out or errors says so in the kernel's stderr tail,
    // which reaches a caller only through the next cell. Without it a failure
    // here reports that the names are missing and never why.
    let diagnostics = cell(&service, "pass")
        .await
        .map(|probe| probe.result.stderr)
        .unwrap_or_default();
    assert!(
        notice.contains("<ipython_state>") && notice.contains("kept"),
        "the notice must list surviving names: {notice}\nkernel diagnostics: {diagnostics}"
    );
    service.dispose().await;
    Ok(())
}

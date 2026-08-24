use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_kernel::bootstrap::{
    BootstrapOptions, default_runtime_source_dir, default_skills_source_dir, ensure_kernel_python,
};
use yi_kernel::client::{
    AbortFlag, ExecuteOptions, HostFuture, HostHandlers, KernelManager, KernelOptions,
    KernelSnapshotConfig,
};
use yi_kernel::snapshot::{manifest_path_in, snapshot_path_in};
use yi_types::kernel::ExecuteStatus;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct EchoHost;

impl HostHandlers for EchoHost {
    fn dispatch(&self, request_type: &str, payload: Map<String, Value>) -> Option<HostFuture> {
        if request_type != "test.echo" {
            return None;
        }
        Some(Box::pin(async move {
            let mut reply = Map::new();
            reply.insert("echoed".to_owned(), Value::Object(payload));
            Ok(reply)
        }))
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn manager() -> Result<KernelManager, String> {
    manager_with_snapshot(None)
}

fn manager_with_snapshot(snapshot: Option<KernelSnapshotConfig>) -> Result<KernelManager, String> {
    let python = ensure_kernel_python(&BootstrapOptions {
        on_progress: Some(Box::new(|message| eprintln!("{message}"))),
        home: home(),
        runtime_source_dir: default_runtime_source_dir(),
        skills_source_dir: default_skills_source_dir(),
    })?;
    KernelManager::new(KernelOptions {
        python: Some(python),
        cwd: None,
        env: Vec::new(),
        username: "yi".to_owned(),
        home: home(),
        runtime_source_dir: default_runtime_source_dir(),
        host: Some(Arc::new(EchoHost)),
        on_progress: None,
        snapshot,
    })
}

#[tokio::test]
async fn cells_stream_error_host_request_interrupt_and_shutdown() -> TestResult {
    let kernel = manager()?;

    let result = kernel.execute("1 + 1", ExecuteOptions::default()).await?;
    assert_eq!(result.status, ExecuteStatus::Ok);
    assert_eq!(result.result.as_deref(), Some("2"));

    let result = kernel
        .execute("print('over'); print('wire')", ExecuteOptions::default())
        .await?;
    assert_eq!(result.stdout, "over\nwire\n");

    let result = kernel.execute("1 / 0", ExecuteOptions::default()).await?;
    assert_eq!(result.status, ExecuteStatus::Error);
    assert_eq!(
        result.error.as_ref().map(|error| error.ename.as_str()),
        Some("ZeroDivisionError")
    );

    let state_survives = kernel
        .execute("x = 41; x + 1", ExecuteOptions::default())
        .await?;
    assert_eq!(
        state_survives.result.as_deref(),
        Some("42"),
        "namespace must persist across cells"
    );

    let echoed = kernel
        .execute(
            "import rlm\nreply = await rlm.host_request('test.echo', {'value': 7})\nprint(reply['echoed']['value'])",
            ExecuteOptions::default(),
        )
        .await?;
    assert_eq!(
        echoed.status,
        ExecuteStatus::Ok,
        "host.request round trip failed: {} {}",
        echoed.stderr,
        echoed
            .error
            .as_ref()
            .map(|error| error.evalue.clone())
            .unwrap_or_default()
    );
    assert_eq!(echoed.stdout.trim(), "7");

    let unregistered = kernel
        .execute(
            "import rlm\ntry:\n    await rlm.host_request('mcp.refresh', {})\nexcept RuntimeError as e:\n    print(f'refused: {e}')",
            ExecuteOptions::default(),
        )
        .await?;
    assert!(
        unregistered
            .stdout
            .contains("host request type \"mcp.refresh\" is not available in this session"),
        "unregistered types must error prime's way: {}",
        unregistered.stdout
    );

    let abort = AbortFlag::default();
    let abort_remote = abort.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        abort_remote.fire();
    });
    let interrupted = kernel
        .execute(
            "import time\ntime.sleep(30)",
            ExecuteOptions {
                abort: Some(abort),
                ..ExecuteOptions::default()
            },
        )
        .await?;
    assert_eq!(
        interrupted.status,
        ExecuteStatus::Aborted,
        "an aborted sleep must come back aborted, not hang"
    );

    let after = kernel.execute("'alive'", ExecuteOptions::default()).await?;
    assert_eq!(
        after.result.as_deref(),
        Some("'alive'"),
        "the kernel must accept work again after an interrupt"
    );

    assert!(
        kernel.shutdown().await,
        "this caller should perform the cleanup"
    );
    let dead = kernel.execute("1", ExecuteOptions::default()).await;
    assert!(dead.is_err(), "a shut-down kernel must refuse new cells");
    Ok(())
}

#[tokio::test]
async fn namespace_snapshot_revives_across_kernels() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-snap-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = KernelSnapshotConfig {
        path: snapshot_path_in(&dir),
        manifest_path: manifest_path_in(&dir),
        max_bytes: None,
        max_variable_bytes: None,
        debounce_ms: None,
    };

    let first = manager_with_snapshot(Some(config.clone()))?;
    let restore = first.restore_state().await.ok_or("restore result")?;
    assert!(
        restore.restored.is_empty() && restore.failed.is_empty(),
        "a missing snapshot file must report an empty restore, not an error"
    );
    let cell = first
        .execute(
            "x = 41\nwords = ['a', 'b']\nunpicklable = (i for i in [1])",
            ExecuteOptions::default(),
        )
        .await?;
    assert_eq!(cell.status, ExecuteStatus::Ok);
    let snapshot = first.snapshot_state().await.ok_or("snapshot result")?;
    assert!(
        snapshot.saved.contains(&"x".to_owned()) && snapshot.saved.contains(&"words".to_owned()),
        "user variables must be saved: {:?}",
        snapshot.saved
    );
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|skip| skip.name == "unpicklable"),
        "an unpicklable variable must be skipped, not fatal: {:?}",
        snapshot.skipped
    );
    assert!(
        !snapshot
            .saved
            .iter()
            .any(|name| name == "In" || name == "Out"),
        "IPython-injected names must never be snapshotted"
    );
    assert!(snapshot.bytes > 0 && config.path.is_file());
    first.dispose().await;

    let second = manager_with_snapshot(Some(config.clone()))?;
    let restore = second.restore_state().await.ok_or("restore result")?;
    assert!(
        restore.restored.contains(&"x".to_owned()),
        "a fresh kernel must revive snapshotted names: {restore:?}"
    );
    let cell = second
        .execute("print(x + 1, words)", ExecuteOptions::default())
        .await?;
    assert!(
        cell.stdout.contains("42 ['a', 'b']"),
        "revived values must be usable: {} {}",
        cell.stdout,
        cell.stderr
    );
    second.dispose().await;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn prune_removes_oversized_variables_and_list_names_reports() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-prune-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let kernel = manager_with_snapshot(Some(KernelSnapshotConfig {
        path: snapshot_path_in(&dir),
        manifest_path: manifest_path_in(&dir),
        max_bytes: None,
        // Tiny cap so the test does not have to allocate 16 MiB.
        max_variable_bytes: Some(1_024),
        debounce_ms: None,
    }))?;

    let cell = kernel
        .execute("small = 7\nbig = 'x' * 100_000", ExecuteOptions::default())
        .await?;
    assert_eq!(cell.status, ExecuteStatus::Ok);

    let names = kernel
        .list_namespace_names()
        .await
        .ok_or("list_namespace_names")?;
    assert!(
        names.contains(&"big".to_owned()) && names.contains(&"small".to_owned()),
        "listing must report user names: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name == "In" || name == "Out"),
        "listing must filter IPython-injected names"
    );

    let pruned = kernel
        .prune_oversized_variables()
        .await
        .ok_or("prune result")?;
    assert_eq!(
        pruned.pruned,
        vec!["big".to_owned()],
        "the over-cap variable must be pruned"
    );
    assert!(
        pruned.saved.contains(&"small".to_owned()),
        "under-cap variables must survive a prune"
    );

    let gone = kernel.execute("big", ExecuteOptions::default()).await?;
    assert_eq!(
        gone.error.as_ref().map(|error| error.ename.as_str()),
        Some("NameError"),
        "a pruned variable must be deleted from the live namespace"
    );
    let kept = kernel.execute("small", ExecuteOptions::default()).await?;
    assert_eq!(kept.result.as_deref(), Some("7"));

    kernel.dispose().await;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_kernel::bootstrap::{
    BootstrapOptions, default_runtime_source_dir, default_skills_source_dir, ensure_kernel_python,
};
use yi_kernel::client::{
    AbortFlag, ExecuteOptions, HostFuture, HostHandlers, KernelManager, KernelOptions,
};
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

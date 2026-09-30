use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_kernel::ATTACHMENT_DISPLAY_MIME;
use yi_kernel::bootstrap::{
    BootstrapOptions, Toolchain, default_runtime_source_dir, default_skills_source_dir,
    ensure_kernel_python, find_system_python, has_runtime,
};
use yi_kernel::client::{
    AbortFlag, ExecuteOptions, HostFuture, HostHandlers, KernelManager, KernelOptions,
    KernelSnapshotConfig,
};
use yi_kernel::snapshot::{manifest_path_in, snapshot_path_in};
use yi_types::kernel::ExecuteStatus;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct EchoHost;

/// Set when a `test.hold` request's host future is dropped rather than left running.
static HOLD_DROPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        HOLD_DROPPED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl HostHandlers for EchoHost {
    fn dispatch(&self, request_type: &str, payload: Map<String, Value>) -> Option<HostFuture> {
        if request_type == "test.hold" {
            return Some(Box::pin(async move {
                let _held = Held;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(Map::new())
            }));
        }
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
        toolchain: None,
        venv_dir: None,
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
        wrap: None,
    })
}

/// Under Rosetta debugpy's import forked a child that never exited and `kernel_info` timed out;
/// the kernel's own `PYTHONPATH` shadows it, ahead of the path the caller gave, which it keeps.
#[tokio::test]
async fn a_kernel_cannot_import_debugpy_and_keeps_the_given_python_path() -> TestResult {
    let python = ensure_kernel_python(&BootstrapOptions {
        on_progress: None,
        home: home(),
        runtime_source_dir: default_runtime_source_dir(),
        skills_source_dir: default_skills_source_dir(),
        toolchain: None,
        venv_dir: None,
    })?;
    let kernel = KernelManager::new(KernelOptions {
        python: Some(python),
        cwd: None,
        env: vec![("PYTHONPATH".to_owned(), "/already".to_owned())],
        username: "yi".to_owned(),
        home: home(),
        runtime_source_dir: default_runtime_source_dir(),
        host: Some(Arc::new(EchoHost)),
        on_progress: None,
        snapshot: None,
        wrap: None,
    })?;
    let shadowed = kernel
        .execute("import debugpy", ExecuteOptions::default())
        .await?;
    let said = shadowed.error.as_ref().map(|error| error.evalue.as_str());
    assert_eq!(
        said,
        Some("yi kernel does not load debugpy"),
        "{shadowed:?}"
    );
    let path = kernel
        .execute(
            "import os; os.environ['PYTHONPATH'].split(os.pathsep)[1:]",
            ExecuteOptions::default(),
        )
        .await?;
    assert_eq!(path.result.as_deref(), Some("['/already']"), "{path:?}");
    kernel.shutdown().await;
    Ok(())
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
        "unregistered types must error rather than reply: {}",
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

/// Dies with the host task outliving its comm: an `rlm.receive` the cell stopped awaiting
/// kept polling for 300 s, marked the mail it took as read and replied to a closed comm.
#[tokio::test]
async fn an_abandoned_host_request_stops_its_host_task() -> TestResult {
    let kernel = manager()?;
    let cell = "import asyncio, rlm\ntry:\n    await asyncio.wait_for(rlm.host_request('test.hold', {}), 0.3)\nexcept asyncio.TimeoutError:\n    print('gave up')";
    let result = kernel.execute(cell, ExecuteOptions::default()).await?;
    assert_eq!(result.stdout.trim(), "gave up", "{}", result.stderr);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !HOLD_DROPPED.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the host task still runs after its comm closed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    kernel.dispose().await;
    Ok(())
}

/// Incident: the debounced snapshot's 5 s abort left its cell in the active slot, so the
/// user's next cell waited out the busy window and was refused (the 2026-09-10 audit, S7).
#[tokio::test]
async fn an_aborted_internal_cell_clears_the_active_slot() -> TestResult {
    let kernel = manager()?;
    let abort = AbortFlag::default();
    let deaf = abort.clone();
    // Deaf to the interrupt past the grace and the busy window, then failing on it, as a
    // snapshot inside one long C call does; the abort fires once the cell has gone deaf.
    let stuck = kernel
        .execute(
            "import signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\nprint('deaf', flush=True)\ntime.sleep(10)\nraise KeyboardInterrupt",
            ExecuteOptions {
                abort: Some(abort),
                on_stream: Some(Box::new(move |_, _| deaf.fire())),
                internal: true,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    assert_eq!(stuck.status, ExecuteStatus::Aborted);

    let after = kernel.execute("'alive'", ExecuteOptions::default()).await?;
    assert_eq!(
        after.result.as_deref(),
        Some("'alive'"),
        "the user's next cell must run once Yi's own cell is abandoned"
    );
    kernel.dispose().await;
    Ok(())
}

/// Incident: nothing remembered a skip, so a variable over the per-variable cap was
/// serialized up to the cap, and thrown away, on every checkpoint.
#[tokio::test]
async fn an_over_cap_variable_is_serialized_once_not_every_checkpoint() -> TestResult {
    let dir = Scratch::new("yi-snap-overcap")?;
    let kernel = manager_with_snapshot(Some(KernelSnapshotConfig {
        path: snapshot_path_in(&dir),
        manifest_path: manifest_path_in(&dir),
        max_bytes: None,
        max_variable_bytes: Some(1 << 20),
        debounce_ms: None,
    }))?;
    kernel
        .execute(
            "class Big:\n    reduced = 0\n    def __reduce__(self):\n        Big.reduced += 1\n        return (bytes, (b'x' * (2 << 20),))\nbig = Big()",
            ExecuteOptions::default(),
        )
        .await?;
    for _ in 0..2 {
        let snapshot = kernel.snapshot_state().await.ok_or("snapshot result")?;
        assert!(
            snapshot.skipped.iter().any(|skip| skip.name == "big"),
            "{snapshot:?}"
        );
    }
    let count = kernel
        .execute("print(Big.reduced)", ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    assert_eq!(count.stdout.trim(), "1", "{}", count.stderr);
    Ok(())
}

/// Incident: the over-cap memo matched a list shrunk in place (same id, type and size), so
/// the post-compaction prune deleted a variable that no longer crossed the cap.
#[tokio::test]
async fn a_prune_keeps_an_over_cap_variable_that_shrank_in_place() -> TestResult {
    let dir = Scratch::new("yi-snap-shrunk")?;
    let kernel = manager_with_snapshot(Some(KernelSnapshotConfig {
        path: snapshot_path_in(&dir),
        manifest_path: manifest_path_in(&dir),
        max_bytes: None,
        max_variable_bytes: Some(1 << 20),
        debounce_ms: None,
    }))?;
    kernel
        .execute("rows = [b'x' * (2 << 20)]", ExecuteOptions::default())
        .await?;
    let first = kernel.snapshot_state().await.ok_or("snapshot result")?;
    assert!(
        first.skipped.iter().any(|skip| skip.name == "rows"),
        "{first:?}"
    );
    kernel
        .execute("rows[0] = b'small'", ExecuteOptions::default())
        .await?;
    let prune = kernel
        .prune_oversized_variables()
        .await
        .ok_or("prune result")?;
    let alive = kernel
        .execute("print('rows' in globals())", ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    assert!(prune.pruned.is_empty(), "{prune:?}");
    assert_eq!(alive.stdout.trim(), "True", "{}", alive.stderr);
    Ok(())
}

/// Incident: every cell, Yi's own included, was written to the user's
/// `~/.ipython/profile_default/history.sqlite`, one sqlite lock shared by every kernel.
#[tokio::test]
async fn the_kernel_keeps_no_ipython_history() -> TestResult {
    let kernel = manager()?;
    let probe = kernel
        .execute(
            "print(get_ipython().history_manager.enabled)",
            ExecuteOptions::default(),
        )
        .await?;
    kernel.dispose().await;
    assert_eq!(probe.stdout.trim(), "False", "{}", probe.stderr);
    Ok(())
}

/// Incident (#817): kernel code displayed an "image" whose data ended the kitty escape the
/// console writes it into and made the terminal write the clipboard; the provider then
/// refused it on every later request. A Pillow PNG through the same display still attaches.
#[tokio::test]
async fn an_image_whose_data_is_not_base64_attaches_nothing() -> TestResult {
    let kernel = manager()?;
    let attach = |data: &str| {
        format!(
            "from IPython.display import display\ndisplay({{'{ATTACHMENT_DISPLAY_MIME}': {{'mime_type': 'image/png', 'data': '{data}'}}}}, raw=True)"
        )
    };
    let injected = kernel
        .execute(
            &attach(r"AAAA\x1b\\\x1b]52;c;ZWNobyBwd25lZA==\x07"),
            ExecuteOptions::default(),
        )
        .await?;
    let png = kernel
        .execute(&attach(PILLOW_PNG), ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    assert!(
        injected.attachments.is_empty(),
        "{:?}",
        injected.attachments
    );
    assert_eq!(injected.status, ExecuteStatus::Error);
    assert!(
        injected.stderr.contains("attachment dropped"),
        "{}",
        injected.stderr
    );
    assert_eq!(png.attachments.len(), 1, "{}", png.stderr);
    assert_eq!(png.status, ExecuteStatus::Ok);
    Ok(())
}

/// Pillow's encode of a 1x1 RGB image, chosen so its base64 carries `+` and `/`.
const PILLOW_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";

#[tokio::test]
async fn namespace_snapshot_revives_across_kernels() -> TestResult {
    let dir = Scratch::new("yi-snap-e2e")?;
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
    let held = std::fs::File::open(&config.manifest_path)?;
    let before = std::fs::read(&config.manifest_path)?;
    first.execute("more = 1", ExecuteOptions::default()).await?;
    first.snapshot_state().await.ok_or("second snapshot")?;
    assert_ne!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&config.manifest_path)?),
        std::os::unix::fs::MetadataExt::ino(&held.metadata()?),
        "a reader holding the manifest must keep the old file, not see it rewritten in place"
    );
    let mut kept = Vec::new();
    std::io::Read::read_to_end(&mut &held, &mut kept)?;
    assert_eq!(kept, before);
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
    Ok(())
}

/// A dispose re-serialized the whole namespace though the last checkpoint held every cell;
/// a cell run after the checkpoint must still reach the disk.
#[tokio::test]
async fn dispose_flushes_only_what_the_last_checkpoint_missed() -> TestResult {
    let dir = Scratch::new("yi-snap-settled")?;
    let config = KernelSnapshotConfig {
        path: snapshot_path_in(&dir),
        manifest_path: manifest_path_in(&dir),
        max_bytes: None,
        max_variable_bytes: None,
        debounce_ms: Some(600_000),
    };
    let inode =
        || std::fs::metadata(&config.path).map(|meta| std::os::unix::fs::MetadataExt::ino(&meta));
    let first = manager_with_snapshot(Some(config.clone()))?;
    first.execute("x = 1", ExecuteOptions::default()).await?;
    first.snapshot_state().await.ok_or("snapshot result")?;
    let checkpoint = inode()?;
    first.dispose().await;
    assert_eq!(
        inode()?,
        checkpoint,
        "dispose rewrote a checkpoint that held every cell"
    );

    let second = manager_with_snapshot(Some(config.clone()))?;
    second.execute("y = 2", ExecuteOptions::default()).await?;
    second.dispose().await;
    let manifest = std::fs::read_to_string(&config.manifest_path)?;
    assert!(
        manifest.contains("\"y\""),
        "the cell after the checkpoint was lost: {manifest}"
    );
    Ok(())
}

#[tokio::test]
async fn prune_removes_oversized_variables_and_list_names_reports() -> TestResult {
    let dir = Scratch::new("yi-prune-e2e")?;
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
    Ok(())
}

/// The v4 trial images ship python3 and no uv; the venv must build from python3 alone and the
/// kernel it boots must import `rlm`, or delegation is unreachable for the whole hour.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn the_kernel_boots_on_system_python_when_uv_is_absent() -> TestResult {
    let Some(python3) = find_system_python() else {
        return Ok(());
    };
    let scratch = Scratch::new("yi-system-venv")?;
    let built = tokio::task::spawn_blocking({
        let venv = scratch.join("venv");
        move || {
            ensure_kernel_python(&BootstrapOptions {
                on_progress: Some(Box::new(|message| eprintln!("{message}"))),
                home: home(),
                runtime_source_dir: default_runtime_source_dir(),
                skills_source_dir: default_skills_source_dir(),
                toolchain: Some(Toolchain::System(python3)),
                venv_dir: Some(venv),
            })
        }
    })
    .await?;
    // A python whose ensurepip cannot seed a venv (the gate's runner) is the ladder's
    // problem to name, not this test's to prove; uv is the toolchain there.
    let python = match built {
        Ok(python) => python,
        Err(error) if error.contains("ensurepip") => {
            eprintln!("skipped: {error}");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    assert!(has_runtime(&python), "{}", python.display());
    let kernel = KernelManager::new(KernelOptions {
        python: Some(python),
        cwd: None,
        env: Vec::new(),
        username: "yi".to_owned(),
        home: home(),
        runtime_source_dir: default_runtime_source_dir(),
        host: Some(Arc::new(EchoHost)),
        on_progress: None,
        snapshot: None,
        wrap: None,
    })?;
    let result = kernel
        .execute(
            "import rlm; print(callable(rlm.run))",
            ExecuteOptions::default(),
        )
        .await?;
    kernel.dispose().await;
    assert_eq!(result.status, ExecuteStatus::Ok, "{result:?}");
    assert_eq!(result.stdout.trim(), "True");
    Ok(())
}

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
use yi_kernel::snapshot::snapshot_path_in;
use yi_types::image::{ImageDefect, image_defect};
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
        if request_type == "model.info" {
            let mut reply = Map::new();
            reply.insert("input".to_owned(), serde_json::json!(["text", "image"]));
            return Some(Box::pin(async move { Ok(reply) }));
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
        connection_dir: None,
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
        connection_dir: None,
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
            "import rlm\nreply = await rlm.host_request('test.echo', {'value': 7})\nprint(reply['echoed']['value'], sorted(reply['echoed']))",
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
    assert_eq!(
        echoed.stdout.trim(),
        "7 ['type', 'value']",
        "the handler must receive the request as the cell sent it, with no key added"
    );

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

/// #906: yi's provider keys reached the kernel and everything a cell starts, wherever no
/// sandbox stood between them (a host with no Seatbelt).
#[tokio::test]
async fn the_kernel_inherits_no_provider_key() -> TestResult {
    for name in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "YI_906_ORDINARY"] {
        // SAFETY: nextest runs each test in a process of its own, so no thread reads the env.
        unsafe { std::env::set_var(name, format!("ENV-906-{name}")) };
    }
    let kernel = manager()?;
    let probe = kernel
        .execute(
            "import os; print(sorted(k for k, v in os.environ.items() if v.startswith('ENV-906-')))",
            ExecuteOptions::default(),
        )
        .await?;
    kernel.dispose().await;
    assert_eq!(
        probe.stdout.trim(),
        "['YI_906_ORDINARY']",
        "{}",
        probe.stderr
    );
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

/// A refused image names a remedy (#860); each one is run here as the model would run it, on
/// a source the defect describes, and must attach an image the provider takes.
#[tokio::test]
async fn the_remedy_an_image_refusal_names_attaches_the_image() -> TestResult {
    let dir = Scratch::new("yi-image-remedy")?;
    let kernel = manager()?;
    let sources = [
        (
            ImageDefect::Type,
            "x.bmp",
            "Image.new('RGB', (4, 4)).save(path, 'BMP')",
        ),
        (
            ImageDefect::TooWide {
                width: 9000,
                height: 2,
            },
            "wide.png",
            "Image.new('RGB', (9000, 2)).save(path)",
        ),
    ];
    let mut outcomes = Vec::new();
    for (defect, name, make) in sources {
        let remedy = defect.remedy();
        let call = remedy.split('`').nth(1).ok_or("no call in the remedy")?;
        let cell = format!(
            "from PIL import Image\nfrom attach_image import run as attach_image\npath = {:?}\n{make}\n{call}",
            dir.join(name).display().to_string()
        );
        outcomes.push((
            defect,
            kernel.execute(&cell, ExecuteOptions::default()).await?,
        ));
    }
    kernel.dispose().await;
    for (defect, outcome) in outcomes {
        let [attachment] = outcome.attachments.as_slice() else {
            return Err(format!("{defect:?}: {} {:?}", outcome.stderr, outcome.error).into());
        };
        assert_eq!(
            image_defect(&attachment.mime_type, &attachment.data),
            None,
            "{defect:?}"
        );
    }
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
    let held = std::fs::File::open(&config.path)?;
    let before = std::fs::read(&config.path)?;
    first.execute("more = 1", ExecuteOptions::default()).await?;
    first.snapshot_state().await.ok_or("second snapshot")?;
    assert_ne!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&config.path)?),
        std::os::unix::fs::MetadataExt::ino(&held.metadata()?),
        "a reader holding the index must keep the old file, not see it rewritten in place"
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

    let third = manager_with_snapshot(Some(config.clone()))?;
    let restore = third.restore_state().await.ok_or("restore result")?;
    third.dispose().await;
    assert!(
        restore.restored.contains(&"y".to_owned()),
        "the cell after the checkpoint was lost: {restore:?}"
    );
    Ok(())
}

async fn settle(kernel: &KernelManager) -> TestResult {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while kernel.unsaved() {
        if std::time::Instant::now() > deadline {
            return Err("the debounced checkpoint never caught up".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

fn debounced(dir: &Scratch) -> Option<KernelSnapshotConfig> {
    Some(KernelSnapshotConfig {
        path: snapshot_path_in(dir),
        max_bytes: None,
        max_variable_bytes: None,
        debounce_ms: Some(0),
    })
}

/// Incident: every clean cell re-dilled every variable, 471 ms for 60,000 grep hits, paid by a
/// read-only cell in the kernel's one queue.
#[tokio::test]
async fn a_read_only_cell_leaves_untouched_variables_undilled() -> TestResult {
    let dir = Scratch::new("yi-snap-lean")?;
    let kernel = manager_with_snapshot(debounced(&dir))?;
    kernel
        .execute(
            "class Counted:\n    dumps = 0\n    def __reduce__(self):\n        Counted.dumps += 1\n        return (Counted, ())\nbig = Counted()",
            ExecuteOptions::default(),
        )
        .await?;
    settle(&kernel).await?;
    for _ in 0..3 {
        kernel
            .execute("print(len('read only'))", ExecuteOptions::default())
            .await?;
        settle(&kernel).await?;
    }
    let count = kernel
        .execute("print(Counted.dumps)", ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    assert_eq!(count.stdout.trim(), "1", "{}", count.stderr);
    Ok(())
}

/// A cell that fails to compile ran nothing, so it must not force the next pass to re-dill
/// every variable.
#[tokio::test]
async fn a_syntax_error_cell_keeps_the_next_pass_incremental() -> TestResult {
    let dir = Scratch::new("yi-snap-syntax")?;
    let kernel = manager_with_snapshot(debounced(&dir))?;
    kernel
        .execute(
            "class Counted:\n    dumps = 0\n    def __reduce__(self):\n        Counted.dumps += 1\n        return (Counted, ())\nbig = Counted()",
            ExecuteOptions::default(),
        )
        .await?;
    settle(&kernel).await?;
    kernel.execute("x = (", ExecuteOptions::default()).await?;
    kernel
        .execute("print(1)", ExecuteOptions::default())
        .await?;
    settle(&kernel).await?;
    let count = kernel
        .execute("print(Counted.dumps)", ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    assert_eq!(count.stdout.trim(), "1", "{}", count.stderr);
    Ok(())
}

/// A full pass that outruns the debounce bound must not make every later pass full: the cell
/// after it still checkpoints.
#[tokio::test]
async fn a_timed_out_full_pass_leaves_later_cells_checkpointed() -> TestResult {
    let dir = Scratch::new("yi-snap-slow")?;
    let marker = snapshot_path_in(&dir).with_extension("slow");
    let kernel = manager_with_snapshot(debounced(&dir))?;
    let define = format!(
        "import time\nSLOW = False\nclass Slow:\n    def __reduce__(self):\n        if SLOW:\n            open({:?}, 'w').close()\n            time.sleep(8)\n        return (Slow, ())\nslow = Slow()",
        marker.to_string_lossy()
    );
    kernel.execute(&define, ExecuteOptions::default()).await?;
    settle(&kernel).await?;
    kernel
        .execute("SLOW = True", ExecuteOptions::default())
        .await?;
    settle(&kernel).await?;
    for _ in 2..19 {
        kernel
            .execute("print(1)", ExecuteOptions::default())
            .await?;
        settle(&kernel).await?;
    }
    kernel
        .execute("print(1)", ExecuteOptions::default())
        .await?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !marker.exists() {
        if std::time::Instant::now() > deadline {
            return Err("the twentieth cell never started a full pass".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    kernel.execute("z = 5", ExecuteOptions::default()).await?;
    let caught_up = settle(&kernel).await;
    kernel
        .execute("SLOW = False", ExecuteOptions::default())
        .await?;
    kernel.dispose().await;
    caught_up
}

/// A mutation reached only through a nested reference keeps the outer name's id, type and size,
/// so an incremental pass reuses its blob; the dispose flush is full and saves it.
#[tokio::test]
async fn dispose_heals_a_nested_mutation_no_cell_named() -> TestResult {
    let dir = Scratch::new("yi-snap-nested")?;
    let first = manager_with_snapshot(debounced(&dir))?;
    for code in ["a = [[1]]", "b = a[0]", "b.append(2)"] {
        first.execute(code, ExecuteOptions::default()).await?;
        settle(&first).await?;
    }
    first.dispose().await;
    let second = manager_with_snapshot(debounced(&dir))?;
    second.restore_state().await.ok_or("restore result")?;
    let probe = second
        .execute("print(a)", ExecuteOptions::default())
        .await?;
    second.dispose().await;
    assert_eq!(probe.stdout.trim(), "[[1, 2]]", "{}", probe.stderr);
    Ok(())
}

/// A name bound to the object a cell mutated through an alias is saved with the mutation, even
/// when the mutation leaves the object's size unchanged.
#[tokio::test]
async fn a_mutation_through_an_alias_survives_a_restart() -> TestResult {
    let dir = Scratch::new("yi-snap-alias")?;
    let first = manager_with_snapshot(debounced(&dir))?;
    for code in ["x = [0]", "y = x", "y[0] = 1"] {
        first.execute(code, ExecuteOptions::default()).await?;
        settle(&first).await?;
    }
    first.dispose().await;
    let second = manager_with_snapshot(debounced(&dir))?;
    second.restore_state().await.ok_or("restore result")?;
    let probe = second
        .execute("print(x)", ExecuteOptions::default())
        .await?;
    second.dispose().await;
    assert_eq!(probe.stdout.trim(), "[1]", "{}", probe.stderr);
    Ok(())
}

/// Incident: IPython's `Out` cache kept a displayed value alive after `del`, so its memory
/// was never returned.
#[tokio::test]
async fn a_displayed_value_is_freed_by_del() -> TestResult {
    let kernel = manager()?;
    let options = ExecuteOptions::default;
    kernel
        .execute(
            "import weakref, gc\nclass Held: pass\nheld = Held()\nref = weakref.ref(held)",
            options(),
        )
        .await?;
    kernel.execute("held", options()).await?;
    kernel.execute("del held", options()).await?;
    let probe = kernel
        .execute("gc.collect()\nprint(ref() is None)", options())
        .await?;
    kernel.dispose().await;
    assert_eq!(probe.stdout.trim(), "True", "{}", probe.stderr);
    Ok(())
}

/// Yi's own cells (snapshots, listings) stay out of `In`, so the model's cell numbers are its own.
#[tokio::test]
async fn only_model_cells_grow_the_input_history() -> TestResult {
    let kernel = manager()?;
    let length = "print(len(In))";
    let before = kernel.execute(length, ExecuteOptions::default()).await?;
    kernel.list_namespace_names().await.ok_or("listing")?;
    kernel.list_namespace_names().await.ok_or("listing")?;
    let after = kernel.execute(length, ExecuteOptions::default()).await?;
    kernel.dispose().await;
    let count = |text: &str| text.trim().parse::<usize>();
    assert_eq!(count(&after.stdout)?, count(&before.stdout)? + 1);
    Ok(())
}

/// Incident: a listing issued while another cell held the kernel answered "no names" inside its
/// own budget, reddening the gate on a namespace query the change never touched.
#[tokio::test]
#[ignore = "tier-2 journey: `just journeys`"]
async fn a_namespace_listing_answers_after_a_contended_kernel() -> TestResult {
    let kernel = manager()?;
    let cell = kernel
        .execute("kept = 41", ExecuteOptions::default())
        .await?;
    assert_eq!(cell.status, ExecuteStatus::Ok);

    let (slept, listed) = tokio::join!(
        kernel.execute("import time; time.sleep(6)", ExecuteOptions::default()),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            kernel.list_namespace_names().await
        }
    );
    let names = listed.ok_or("list_namespace_names")?;
    assert!(
        names.contains(&"kept".to_owned()),
        "a listing behind a busy cell must still report the namespace: {names:?}"
    );
    let _interrupted = slept;
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
        connection_dir: None,
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

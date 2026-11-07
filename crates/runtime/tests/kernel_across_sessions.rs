//! `kernel://<child>/var` is a parent reading a namespace that is not its own,
//! so the proof is one map shared by two sessions the wiring built.
use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::faux_model::faux_model;
use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::fetch::{FetchError, KernelServiceMap, Resolver};
use yi_runtime::{
    AgentSession, KernelService, ProviderStream, RuntimeWiring, SessionConfig, SubagentHost, Wall,
    attach_runtime,
};
use yi_types::message::StopReason;

type TestResult = Result<(), Box<dyn Error>>;

const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 50;

async fn wait_for_status(host: &Arc<SubagentHost>, name: &str, status: &str) -> bool {
    for _ in 0..POLL_ATTEMPTS {
        let listed = host.list();
        if let Some(Value::Array(children)) = listed.get("subagents")
            && children.iter().any(|child| {
                child.get("session_name").and_then(Value::as_str) == Some(name)
                    && child.get("status").and_then(Value::as_str) == Some(status)
            })
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    false
}

async fn child_kernel_answer(
    role: &str,
    tools: Option<&str>,
    code: &str,
    expected: &str,
    extra: Map<String, Value>,
) -> TestResult {
    let root = Scratch::new("yi-kernel-across")?;
    std::fs::write(root.join("walled.txt"), "42")?;
    let provider = Arc::new(ProviderStream::new(None));
    let mut call = Map::new();
    call.insert("code".to_owned(), Value::String(code.to_owned()));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", call)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
    ]);
    let mut parent = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(128_000),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let kernels = KernelServiceMap::new();
    let host = attach_runtime(
        &mut parent,
        RuntimeWiring {
            provider: Arc::clone(&provider),
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            lane_slots: 1,
            deadline: None,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: Wall::default(),
            auto_background: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: Arc::clone(&kernels),
        },
    );
    let parent_kernel = parent
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    printed(&parent_kernel, "parent_answer = 42").await?;
    let contained = extra.contains_key("deny_read");

    let mut kwargs = extra;
    kwargs.insert("name".to_owned(), Value::String("helper".to_owned()));
    kwargs.insert("role".to_owned(), Value::String(role.to_owned()));
    if let Some(tools) = tools {
        kwargs.insert(
            "tools".to_owned(),
            Value::Array(vec![Value::String(tools.to_owned())]),
        );
    }
    host.spawn("bind the answer".to_owned(), kwargs)?;
    assert!(
        wait_for_status(&host, "helper", "completed").await,
        "the child never finished its cell: {:?}",
        host.list()
    );
    if contained && !yi_tools::Sandbox::available() {
        let store = host
            .transcript("helper")
            .ok_or("the worker has no transcript")?;
        let transcript = serde_json::to_string(&yi_runtime::family::recent_entries(&store))?;
        assert!(
            transcript.contains("ipython is unavailable to a walled session here"),
            "{transcript}"
        );
        return Ok(());
    }

    let family = root.join("family");
    let resolver = Arc::new(
        Resolver::new(root.to_path_buf(), Wall::default())
            .with_kernel_variables(kernels)
            .with_family_dir(family.clone()),
    );
    let url: yi_types::url::Url = "kernel://helper/answer".parse()?;
    let reader = Arc::clone(&resolver);
    let fetched = tokio::task::spawn_blocking(move || reader.fetch(&url)).await??;
    assert_eq!(fetched.text, expected);
    assert_eq!(fetched.served_by, "kernel-namespace helper");
    if contained {
        return Ok(());
    }
    // D164: the same variable as an object, dilled by the child's own kernel.
    let object: yi_types::url::Url = "kernel://helper/answer".parse()?;
    let dumper = Arc::clone(&resolver);
    let dumped = tokio::task::spawn_blocking(move || dumper.dump_kernel(&object)).await??;
    let Some((path, bytes)) = dumped else {
        return Err("the helper may write the board".into());
    };
    assert_eq!(path, family.join("helper.answer.dill"));
    assert!(bytes > 0 && path.is_file(), "{path:?} {bytes}");
    Ok(())
}

#[tokio::test]
async fn a_parent_reads_a_variable_out_of_its_childs_kernel() -> TestResult {
    child_kernel_answer("root", None, "answer = 42", "42", Map::new()).await
}

#[tokio::test]
async fn a_worker_naming_ipython_holds_its_own_kernel() -> TestResult {
    child_kernel_answer("worker", Some("ipython"), "answer = 42", "42", Map::new()).await
}

#[tokio::test]
async fn a_worker_fetches_a_parent_kernel_value() -> TestResult {
    child_kernel_answer(
        "worker",
        Some("ipython"),
        "answer = await rlm.fetch('kernel://main/parent_answer')",
        "42",
        Map::new(),
    )
    .await
}

#[tokio::test]
async fn a_walled_worker_naming_ipython_keeps_the_kernel_boundary() -> TestResult {
    let code = concat!(
        "from pathlib import Path\n",
        "try:\n",
        "    Path('walled.txt').read_text()\n",
        "except OSError:\n",
        "    answer = True\n",
        "else:\n",
        "    answer = False",
    );
    let mut wall = Map::new();
    wall.insert(
        "deny_read".to_owned(),
        Value::Array(vec!["walled.txt".into()]),
    );
    child_kernel_answer("worker", Some("ipython"), code, "True", wall).await
}

/// One root session wired as `build_session` wires it once the session id is known.
pub(crate) fn root_session(
    root: &std::path::Path,
    rlm: &str,
    family: &str,
) -> Result<(AgentSession, Arc<yi_runtime::KernelService>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(128_000),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    attach_runtime(
        &mut session,
        RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            lane_slots: 1,
            deadline: None,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join(rlm),
            family_dir: Some(root.join("family").join(family)),
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: Wall::default(),
            auto_background: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: KernelServiceMap::new(),
        },
    );
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    Ok((session, kernel))
}

async fn printed(
    kernel: &Arc<yi_runtime::KernelService>,
    code: &'static str,
) -> Result<String, Box<dyn Error>> {
    let outcome = run_cell(kernel, &Arc::new(AtomicBool::new(false)), code).await??;
    Ok(format!(
        "{}{}",
        outcome.result.stdout, outcome.result.stderr
    ))
}

/// Incident: the board was `rlm-<pid>/family`, so two sessions one `yi acp` worker hosts
/// read each other's entries, and a `--continue` in a new process found an empty board.
#[tokio::test]
async fn the_family_board_belongs_to_the_root_session() -> TestResult {
    let root = Scratch::new("yi-family-by-session")?;
    let (_a, first) = root_session(&root, "rlm-1", "session-a")?;
    printed(&first, "rlm.put('k', 'from a')").await?;
    let (_b, neighbour) = root_session(&root, "rlm-1", "session-b")?;
    let seen = printed(&neighbour, "print([entry['name'] for entry in rlm.ls()])").await?;
    let (_c, resumed) = root_session(&root, "rlm-2", "session-a")?;
    let kept = printed(&resumed, "print(rlm.get('k'))").await?;
    for kernel in [first, neighbour, resumed] {
        kernel.dispose().await;
    }
    assert!(
        seen.contains("[]"),
        "another session in the process read the board: {seen}"
    );
    assert!(
        kept.contains("from a"),
        "the continued session lost its board: {kept}"
    );
    Ok(())
}

fn bare_service(root: &Scratch) -> Result<Arc<KernelService>, Box<dyn Error>> {
    let registry = yi_runtime::HostRegistry::default();
    Ok(Arc::new(yi_runtime::KernelService::new(
        yi_runtime::KernelServiceOptions {
            cwd: root.to_path_buf(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or("HOME is unset")?,
            session_dir: None,
            family_dir: None,
            host: Arc::new(registry),
            on_restore: None,
            on_boot: None,
            sandbox: None,
            snapshot_key: None,
            per_session_state: false,
            cell_ceiling: None,
        },
    )))
}

type Read = tokio::task::JoinHandle<Result<yi_runtime::fetch::Fetched, FetchError>>;

fn read_main(
    root: &Scratch,
    service: &Arc<KernelService>,
    name: &str,
) -> Result<Read, Box<dyn Error>> {
    let kernels = KernelServiceMap::new();
    kernels.insert("main", service);
    let resolver =
        Resolver::new(root.to_path_buf(), Wall::default()).with_kernel_variables(kernels);
    let url: yi_types::url::Url = format!("kernel://main/{name}").parse()?;
    Ok(tokio::task::spawn_blocking(move || resolver.fetch(&url)))
}

type Cell = tokio::task::JoinHandle<Result<yi_tools::KernelCellOutcome, String>>;

const SLEEP: &str = "import time\nopen('sleeping', 'w').close()\ntime.sleep(20)";

fn run_cell(service: &Arc<KernelService>, stop: &Arc<AtomicBool>, code: &'static str) -> Cell {
    let (service, stop) = (Arc::clone(service), Arc::clone(stop));
    tokio::task::spawn_blocking(move || {
        let cancelled: yi_tools::CancelFlag = Arc::new(move || stop.load(Ordering::SeqCst));
        yi_tools::KernelBridge::execute_cell(service.as_ref(), code, "", &cancelled, None)
    })
}

/// Incident: a read of an agent mid-cell waited for the whole cell, up to the 600 s ceiling,
/// then failed with an empty detail; its 5 s timer never raced the execution queue.
#[tokio::test]
async fn a_read_of_a_busy_kernel_gives_up_within_its_deadline() -> TestResult {
    let root = Scratch::new("yi-kernel-busy-read")?;
    let service = bare_service(&root)?;
    let stop = Arc::new(AtomicBool::new(false));
    run_cell(&service, &stop, "x = 1").await??;
    let sleeper = run_cell(&service, &stop, SLEEP);
    crate::support::wait_for_path(&root.join("sleeping"), "the sleeper cell never started").await?;

    let started = std::time::Instant::now();
    let read = read_main(&root, &service, "x")?.await?;
    let waited = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    let _interrupted = sleeper.await?;
    service.dispose().await;

    let error = read
        .err()
        .ok_or("a read of a kernel mid-cell returned a value")?;
    assert!(
        waited < std::time::Duration::from_secs(8),
        "the read waited {waited:?} for the running cell"
    );
    let message = error.to_string();
    assert!(
        message.contains("running a cell"),
        "the read failed without saying why: {message}"
    );
    assert!(
        message.contains("rlm.put") && message.contains("rlm.get"),
        "the refusal did not name the remedy: {message}"
    );
    assert!(
        message.contains("context_keys") && !message.contains("read family://"),
        "the refusal sent a kernel-less reader to the sidecar: {message}"
    );
    Ok(())
}

/// Incident: a read whose own repr outlived the 5 s deadline on an idle kernel was reported
/// as the agent running a cell, so the model was told to wait for a cell that did not exist.
#[tokio::test]
async fn a_slow_read_of_an_idle_kernel_is_not_called_busy() -> TestResult {
    let root = Scratch::new("yi-kernel-slow-read")?;
    let service = bare_service(&root)?;
    let define = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
        yi_tools::KernelBridge::execute_cell(
            define.as_ref(),
            "import time\nclass Slow:\n    def __repr__(self):\n        end = time.monotonic() + 7\n        while time.monotonic() < end:\n            try:\n                time.sleep(0.1)\n            except KeyboardInterrupt:\n                pass\n        return 'slow'\nslow = Slow()",
            "",
            &cancelled,
            None,
        )
    })
    .await??;
    let read = read_main(&root, &service, "slow")?.await?;
    service.dispose().await;
    let error = read.err().ok_or("a 7 s repr fit a 5 s read")?;
    assert!(
        !error.to_string().contains("running a cell"),
        "an idle kernel's slow read was called busy: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_put_readback_lands_while_the_parent_cell_still_runs() -> TestResult {
    let root = Scratch::new("yi-kernel-put-mid-cell")?;
    let (_parent_session, parent) = root_session(&root, "rlm-1", "family")?;
    let (_child_session, child) = root_session(&root, "rlm-2", "family")?;
    printed(&parent, "rlm.put('x', 'from the parent')").await?;
    let stop = Arc::new(AtomicBool::new(false));
    let sleeper = run_cell(&parent, &stop, SLEEP);
    crate::support::wait_for_path(&root.join("sleeping"), "the sleeper cell never started").await?;
    let seen = printed(&child, "print(rlm.get('x'))").await?;
    let mid_cell = root.join("sleeping").is_file();
    stop.store(true, Ordering::SeqCst);
    let _interrupted = sleeper.await?;
    for kernel in [parent, child] {
        kernel.dispose().await;
    }
    assert!(
        mid_cell,
        "the readback was not taken while the parent cell was still running"
    );
    assert!(
        seen.contains("from the parent"),
        "the child could not read the put value: {seen}"
    );
    Ok(())
}

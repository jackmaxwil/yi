//! `kernel://<child>/var` is a parent reading a namespace that is not its own,
//! so the proof is one map shared by two sessions the wiring built.
use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::fetch::{FetchError, KernelServiceMap, Resolver};
use yi_runtime::{
    AgentSession, KernelService, ProviderStream, RuntimeWiring, SessionConfig, SubagentHost, Wall,
    attach_runtime,
};
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

const POLL_ATTEMPTS: usize = 400;
const POLL_INTERVAL_MS: u64 = 50;

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

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

#[tokio::test]
async fn a_parent_reads_a_variable_out_of_its_childs_kernel() -> TestResult {
    let root = Scratch::new("yi-kernel-across")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut code = Map::new();
    code.insert("code".to_owned(), Value::String("answer = 42".to_owned()));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "ipython", code)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("bound it")], StopReason::Stop),
    ]);
    let mut parent = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
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

    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String("helper".to_owned()));
    host.spawn("bind the answer".to_owned(), kwargs)?;
    assert!(
        wait_for_status(&host, "helper", "completed").await,
        "the child never finished its cell: {:?}",
        host.list()
    );

    let family = root.join("family");
    let resolver = Arc::new(
        Resolver::new(root.to_path_buf(), Wall::default())
            .with_kernel_variables(kernels)
            .with_family_dir(family.clone()),
    );
    let url: yi_types::url::Url = "kernel://helper/answer".parse()?;
    let reader = Arc::clone(&resolver);
    let fetched = tokio::task::spawn_blocking(move || reader.fetch(&url)).await??;
    assert_eq!(fetched.text, "42");
    assert_eq!(fetched.served_by, "kernel-namespace helper");
    // D164: the same variable as an object, dilled by the child's own kernel.
    let object: yi_types::url::Url = "kernel://helper/answer".parse()?;
    let dumper = Arc::clone(&resolver);
    let (path, bytes) = tokio::task::spawn_blocking(move || dumper.dump_kernel(&object)).await??;
    assert_eq!(path, family.join("helper.answer.dill"));
    assert!(bytes > 0 && path.is_file(), "{path:?} {bytes}");
    Ok(())
}

/// One root session wired as `build_session` wires it once the session id is known.
fn root_session(
    root: &std::path::Path,
    rlm: &str,
    family: &str,
) -> Result<(AgentSession, Arc<yi_runtime::KernelService>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
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
    let service = Arc::clone(kernel);
    let outcome = tokio::task::spawn_blocking(move || {
        let cancelled: yi_tools::CancelFlag = Arc::new(|| false);
        yi_tools::KernelBridge::execute_cell(service.as_ref(), code, &cancelled)
    })
    .await??;
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
    let mut registry = yi_runtime::HostRegistry::default();
    registry.register_mcp_stubs();
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

/// Incident: a read of an agent mid-cell waited for the whole cell, up to the 600 s ceiling,
/// then failed with an empty detail; its 5 s timer never raced the execution queue.
#[tokio::test]
async fn a_read_of_a_busy_kernel_gives_up_within_its_deadline() -> TestResult {
    let root = Scratch::new("yi-kernel-busy-read")?;
    let service = bare_service(&root)?;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let run = |code: &'static str| {
        let (service, stop) = (Arc::clone(&service), Arc::clone(&stop));
        tokio::task::spawn_blocking(move || {
            let cancelled: yi_tools::CancelFlag =
                Arc::new(move || stop.load(std::sync::atomic::Ordering::SeqCst));
            yi_tools::KernelBridge::execute_cell(service.as_ref(), code, &cancelled)
        })
    };
    run("x = 1").await??;
    let sleeper = run("import time\nopen('sleeping', 'w').close()\ntime.sleep(20)");
    for _ in 0..600 {
        if root.join("sleeping").is_file() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        root.join("sleeping").is_file(),
        "the sleeper cell never started"
    );

    let started = std::time::Instant::now();
    let read = read_main(&root, &service, "x")?.await?;
    let waited = started.elapsed();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _interrupted = sleeper.await?;
    service.dispose().await;

    let error = read
        .err()
        .ok_or("a read of a kernel mid-cell returned a value")?;
    assert!(
        waited < std::time::Duration::from_secs(8),
        "the read waited {waited:?} for the running cell"
    );
    assert!(
        error.to_string().contains("running a cell"),
        "the read failed without saying why: {error}"
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
            &cancelled,
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

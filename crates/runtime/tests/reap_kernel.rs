use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use crate::faux_model::faux_model;
use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, KernelService, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions,
};
use yi_tools::{CancelFlag, KernelBridge};
use yi_types::message::StopReason;
use yi_types::model::Effort;

type TestResult = Result<(), Box<dyn Error>>;

const POLL_ATTEMPTS: usize = 600;
const POLL_INTERVAL_MS: u64 = 50;

fn process_alive(pid: u32) -> bool {
    let mut command = yi_tools::command("ps");
    command.args(["-p", &pid.to_string()]);
    let cancelled: CancelFlag = Arc::new(|| false);
    yi_tools::run_captured(command, None, &cancelled, 5_000)
        .ok()
        .and_then(|capture| capture.exit_code)
        == Some(0)
}

async fn run_cell(service: &Arc<KernelService>, code: &'static str) -> Result<String, String> {
    let service = Arc::clone(service);
    let outcome = tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), code, "", &cancelled, None)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(outcome.result.stdout)
}

/// The proof is a process count, not a code read: boot an IPython kernel in a
/// child, reap the child, and the kernel's python process must be gone.
#[tokio::test]
async fn a_reaped_childs_booted_kernel_process_is_gone() -> TestResult {
    let root = Scratch::new("yi-reap-kernel")?;
    let kernel_slot: Arc<Mutex<Option<Arc<KernelService>>>> = Arc::new(Mutex::new(None));
    let factory_slot = Arc::clone(&kernel_slot);
    let (events, _keep) = tokio::sync::broadcast::channel(64);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: root.to_path_buf(),
        cwd: std::env::temp_dir(),
        home: std::env::temp_dir(),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(128_000), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            provider.queue_faux(vec![faux_assistant_message(
                vec![faux_text("done")],
                StopReason::Stop,
            )]);
            let child = AgentSession::new(
                SessionConfig {
                    system_prompt: String::new(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            );
            let service = crate::kernel_lane::service();
            child.set_kernel_service(Arc::clone(&service));
            if let Ok(mut slot) = factory_slot.lock() {
                *slot = Some(service);
            }
            Ok(child)
        }),
        notice: Arc::new(|_, _| {}),
        events,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_, _| {}),
        attribute: Arc::new(|_| {}),
        store: Arc::new(|| None),
        plans_dir: std::env::temp_dir().join(".yi/plans"),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));

    let mut kwargs = Map::new();
    kwargs.insert("name".to_owned(), Value::String("kernel-child".to_owned()));
    host.spawn("hold a kernel".to_owned(), kwargs)?;
    let service = {
        let mut found = None;
        for _ in 0..POLL_ATTEMPTS {
            if let Some(service) = kernel_slot.lock().ok().and_then(|slot| slot.clone()) {
                found = Some(service);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
        found.ok_or("the factory never built the child")?
    };

    let stdout = run_cell(&service, "import os\nprint(os.getpid())").await?;
    let pid: u32 = stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().parse().ok())
        .ok_or_else(|| format!("no pid in kernel stdout: {stdout:?}"))?;
    assert!(
        process_alive(pid),
        "the booted kernel {pid} must be running"
    );

    host.reap("kernel-child")?;
    let mut gone = false;
    for _ in 0..POLL_ATTEMPTS {
        if !process_alive(pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    assert!(
        gone,
        "the reaped child's IPython process {pid} is still alive"
    );
    Ok(())
}

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, RuntimeWiring,
    SessionConfig, Wall, attach_runtime, restore_notice_text,
};
use yi_tools::{CancelFlag, KernelBridge};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

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

/// One `yi` process resuming session `id`: the root wired as `main.rs` wires it, with
/// its per-pid `rlm-<pid>` directory under the shared sessions directory.
fn process(
    root: &Path,
    pid: u32,
    id: &str,
) -> Result<(AgentSession, Arc<KernelService>), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let sessions = root.join("sessions");
    attach_runtime(
        &mut session,
        RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: sessions.join(format!("rlm-{pid}")),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: Some(sessions),
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session.attach_store(store(id))?;
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    Ok((session, kernel))
}

fn store(id: &str) -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: id.to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

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
        cell_ceiling: None,
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

/// Incident: the root kernel's directory was the process's `rlm-<pid>`, so `--continue`
/// looked for the snapshot where the new process had never written one, and the replayed
/// transcript's names raised NameError.
#[tokio::test]
async fn the_snapshot_dir_is_the_sessions_dir_not_the_process_dir() -> TestResult {
    const ID: &str = "snap-continue";
    let root = Scratch::new("yi-snap-dir")?;

    let (_first, kernel) = process(&root, 4711, ID)?;
    let outcome = cell(&kernel, "answer = 42").await?;
    assert_eq!(outcome.result.status, yi_types::kernel::ExecuteStatus::Ok);
    kernel.dispose().await;
    let (snapshot, _) = yi_runtime::kernel::snapshot_paths(&root.join("sessions"), Some(ID));
    assert!(
        snapshot.is_file(),
        "the root's snapshot sits flat in the sessions dir, not under rlm-<pid>: {}",
        snapshot.display()
    );

    let (_second, kernel) = process(&root, 4890, ID)?;
    let outcome = cell(&kernel, "print(answer)").await?;
    assert!(
        outcome.result.stdout.contains("42"),
        "the next process must revive the namespace: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    kernel.dispose().await;
    Ok(())
}

/// Incident: `/new`, `switch_session` and `fork` swap the store under a live kernel, which
/// kept snapshotting under the first session's id, so the second session's names overwrote
/// the first's file in the shared sessions dir and the second had none.
#[tokio::test]
async fn a_store_switch_rekeys_the_live_kernel() -> TestResult {
    let root = Scratch::new("yi-snap-switch")?;

    let (session, kernel) = process(&root, 4711, "snap-a")?;
    cell(&kernel, "answer = 42").await?;
    session.reset();
    session.attach_store(store("snap-b"))?;
    let outcome = cell(&kernel, "other = 'answer' in dir()\nprint(other)").await?;
    assert!(
        outcome.result.stdout.contains("False"),
        "the new session's kernel starts from its own namespace: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    kernel.dispose().await;
    let (snapshot, _) = yi_runtime::kernel::snapshot_paths(&root.join("sessions"), Some("snap-b"));
    assert!(
        snapshot.is_file(),
        "keyed by the new id: {}",
        snapshot.display()
    );

    let (_next, kernel) = process(&root, 4890, "snap-a")?;
    let outcome = cell(&kernel, "print(answer, 'other' in dir())").await?;
    assert!(
        outcome.result.stdout.contains("42 False"),
        "the first session's snapshot holds only its own names: {} {}",
        outcome.result.stdout,
        outcome.result.stderr
    );
    kernel.dispose().await;
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

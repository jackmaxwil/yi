#![cfg(target_os = "macos")]

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use yi_runtime::{HostRegistry, KernelService, KernelServiceOptions};
use yi_tools::{CancelFlag, KernelBridge, Sandbox};

type TestResult = Result<(), Box<dyn Error>>;

async fn cell(
    service: &Arc<KernelService>,
    code: String,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), &code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn workspace(tag: &str) -> Result<(Scratch, PathBuf, PathBuf, PathBuf), Box<dyn Error>> {
    let root = Scratch::new(&format!("yi-kernel-sbx-{tag}"))?;
    let project = root.join("project");
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let session = root.join("session");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&session)?;
    Ok((root, project, home, session))
}

fn service(
    cwd: PathBuf,
    home: PathBuf,
    sandbox: Sandbox,
    family_dir: Option<PathBuf>,
) -> Arc<KernelService> {
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd,
        home,
        session_dir: None,
        family_dir,
        host: Arc::new(registry),
        on_restore: None,
        on_boot: None,
        sandbox: Some(sandbox),
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }))
}

/// A directory this process can write that no root of `sandbox` covers: HOME, or the shared
/// user directory when a run put HOME under tmp, which the sandbox grants whole.
fn uncovered(sandbox: &Sandbox, home: &std::path::Path) -> Option<PathBuf> {
    let resolve =
        |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let roots: Vec<PathBuf> = sandbox.writable.iter().map(|root| resolve(root)).collect();
    [home.to_path_buf(), PathBuf::from("/Users/Shared")]
        .into_iter()
        .find(|dir| dir.is_dir() && !roots.iter().any(|root| resolve(dir).starts_with(root)))
}

#[tokio::test]
async fn an_ipython_cell_cannot_write_outside_the_confined_roots() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("write")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let kernel = service(project.clone(), home.clone(), sandbox.clone(), None);
    let inside = cell(
        &kernel,
        "open('inside.txt','w').write('in')\nprint('ok')".to_owned(),
    )
    .await?;
    assert_eq!(inside.result.status, yi_types::kernel::ExecuteStatus::Ok);
    assert_eq!(std::fs::read_to_string(project.join("inside.txt"))?, "in");

    let probe = uncovered(&sandbox, &home).ok_or("no writable directory outside the sandbox")?;
    let escape = probe.join(format!("yi-p7-escape-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&escape);
    let path = escape.display().to_string();
    let outside = cell(&kernel, format!("open(r'{path}','w').write('out')")).await?;
    assert_eq!(
        outside.result.status,
        yi_types::kernel::ExecuteStatus::Error
    );
    let blob = format!(
        "{}{}",
        outside.result.stderr,
        outside
            .result
            .error
            .as_ref()
            .map(|error| error.traceback.join("\n"))
            .unwrap_or_default()
    )
    .to_lowercase();
    assert!(
        blob.contains("permission") || blob.contains("not permitted") || blob.contains("errno"),
        "outside write must fail: {blob}"
    );
    assert!(
        !escape.is_file(),
        "the file must not exist after a contained write"
    );

    // ~/.yi itself is read-only; the harness store is written by the host (#583). A HOME
    // under tmp sits inside a granted root, where no profile can make ~/.yi read-only.
    let yi = home.join(".yi");
    if probe == home {
        let config = yi.join(format!("yi-p7-config-{}.txt", std::process::id()));
        let config_path = config.display().to_string();
        let denied = cell(&kernel, format!("open(r'{config_path}','w').write('x')")).await?;
        assert_eq!(denied.result.status, yi_types::kernel::ExecuteStatus::Error);
        assert!(
            !config.is_file(),
            "a cell must not write under ~/.yi itself"
        );
        let harness = yi.join("harness");
        std::fs::create_dir_all(&harness)?;
        let store = harness.join(format!("yi-p7-store-{}.txt", std::process::id()));
        let store_path = store.display().to_string();
        let denied = cell(&kernel, format!("open(r'{store_path}','w').write('x')")).await?;
        assert_eq!(denied.result.status, yi_types::kernel::ExecuteStatus::Error);
        assert!(
            !store.is_file(),
            "a cell must not write the global harness store"
        );
    }
    kernel.dispose().await;
    let _ = std::fs::remove_file(&escape);
    Ok(())
}

#[tokio::test]
async fn a_profile_change_restarts_the_kernel() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("restart")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let kernel = service(project.clone(), home.clone(), sandbox.clone(), None);
    let first = cell(&kernel, "marker = 1\nprint(marker)".to_owned()).await?;
    assert_eq!(first.result.status, yi_types::kernel::ExecuteStatus::Ok);

    let extra = home.join(format!("yi-p7-extra-{}", std::process::id()));
    std::fs::create_dir_all(&extra)?;
    let mut next = sandbox;
    next.writable.push(extra.clone());
    kernel.set_sandbox(Some(next)).await;

    let second = cell(&kernel, "print(marker)".to_owned()).await?;
    assert_eq!(second.result.status, yi_types::kernel::ExecuteStatus::Error);
    let name = second
        .result
        .error
        .as_ref()
        .map(|error| error.ename.as_str())
        .unwrap_or("");
    assert_eq!(name, "NameError", "{name}");
    kernel.dispose().await;
    let _ = std::fs::remove_dir_all(&extra);
    Ok(())
}

/// A root session wired the way `yi` wires one, with `sessions_dir` as its session corpus.
fn root_session(
    project: &std::path::Path,
    home: &std::path::Path,
    rlm_dir: &std::path::Path,
    sessions_dir: Option<PathBuf>,
    broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
) -> yi_runtime::AgentSession {
    let provider = Arc::new(yi_runtime::ProviderStream::new(None, None));
    let mut session = yi_runtime::AgentSession::new(
        yi_runtime::SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_loop::ExecutionMode::Sequential,
            cwd: project.to_path_buf(),
            home: home.to_path_buf(),
            lane_slots: 1,
            broker,
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: rlm_dir.to_path_buf(),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(rlm_dir.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read,
            sessions_dir,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session
}

/// Incident (#580): the root kernel's writable root was the whole session corpus, where every
/// session's dill snapshot sits flat and is loaded, code and all, at that session's next boot;
/// a cell or its `bash()` could plant one for another session.
#[tokio::test]
async fn a_root_kernel_cannot_write_another_sessions_state() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, home, _session) = workspace("corpus")?;
    let bare = Sandbox::for_workspace(&project, &home, None);
    // Under a tmp root the corpus would be writable anyway and the test would prove nothing.
    let probe = uncovered(&bare, &home).ok_or("no directory outside the sandbox")?;
    let corpus = probe.join(format!("yi-corpus-{}", std::process::id()));
    std::fs::create_dir_all(&corpus)?;
    let session = root_session(
        &project,
        &home,
        &root.join("rlm"),
        Some(corpus.clone()),
        None,
        None,
    );
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let planted = corpus.join("01other.kernel-state.dill");
    let by_job = corpus.join("01other.by-bash.txt");
    let code = format!(
        "try:\n    open(r'{}','wb').write(b'x')\nexcept OSError as e:\n    print(e)\nprint(await bash(\"touch '{}'\"))",
        planted.display(),
        by_job.display()
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let (cell_wrote, job_wrote) = (planted.is_file(), by_job.is_file());
    let _ = std::fs::remove_dir_all(&corpus);
    let ran = ran?;
    assert!(
        !cell_wrote && !job_wrote,
        "the root kernel wrote into the session corpus (cell {cell_wrote}, bash {job_wrote}): {}",
        ran.result.stdout
    );
    Ok(())
}

/// Incident (#583): a global harness save went straight to disk from the kernel, so a cell in
/// one session stored notes every later session reads, with no ask. It now asks the broker.
#[tokio::test]
async fn a_global_harness_save_asks_the_permission_broker() -> TestResult {
    let (root, project, home, _session) = workspace("harness")?;
    let asks = Arc::new(std::sync::Mutex::new(0_u32));
    let counted = Arc::clone(&asks);
    let asker: yi_runtime::Asker = Arc::new(move |_ask| {
        if let Ok(mut count) = counted.lock() {
            *count += 1;
        }
        yi_runtime::AskOutcome::Reject
    });
    let broker = Arc::new(yi_runtime::permission::PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    ));
    let session = root_session(&project, &home, &root.join("rlm"), None, Some(broker), None);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let title = format!("yi-583-probe-{}", std::process::id());
    let code = format!(
        "try:\n    rlm.harness.create_memory('{title}', 'x', global_=True)\n    print('saved')\nexcept Exception as e:\n    print('refused:', e)"
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let stdout = ran?.result.stdout;
    let asked = asks.lock().map(|count| *count).unwrap_or_default();
    assert!(
        asked == 1 && stdout.contains("refused"),
        "a global harness save must ask and honor the answer (asked {asked}): {stdout}"
    );
    Ok(())
}

fn faux_model() -> yi_types::model::Model {
    let zero = || serde_json::Number::from(0u64);
    yi_types::model::Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: yi_types::model::ModelCost {
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

/// Incident: `exec.spawn`, the host half of the kernel's `bash()`, ran with no sandbox, so a
/// contained kernel wrote anywhere by shelling out; then under a profile without the kernel's
/// loopback grant, so a local server or test suite in it could not bind 127.0.0.1.
#[tokio::test]
async fn the_kernels_bash_is_contained_like_the_kernel() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, home, _session) = workspace("bash")?;
    let session = root_session(&project, &home, &root.join("rlm"), None, None, None);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&root.join("rlm")));
    let probe = uncovered(&sandbox, &home).ok_or("no directory outside the sandbox")?;
    let escape = probe.join(format!("yi-bash-escape-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&escape);
    let code = format!("print(await bash(\"touch '{}'\"))", escape.display());
    let ran = cell(&kernel, code).await;
    let bind = r#"import sys
bound = sys.executable + ''' -c "import socket; socket.socket().bind(('127.0.0.1', 0)); print('bound-' + 'ok')"'''
print(await bash(bound))"#;
    let bound = cell(&kernel, bind.to_owned()).await;
    kernel.dispose().await;
    let escaped = escape.is_file();
    let _ = std::fs::remove_file(&escape);
    let ran = ran?;
    assert!(
        !escaped,
        "the kernel's bash() wrote outside the sandbox: {}",
        ran.result.stdout
    );
    let bound = bound?.result.stdout;
    assert!(
        bound.contains("bound-ok"),
        "a loopback bind in bash() failed: {bound}"
    );
    Ok(())
}

/// Incident: a child kernel's writable roots stopped at its own `sub-*` directory, so its
/// `rlm.put` to the family board it shares with its parent failed with EPERM.
#[tokio::test]
async fn a_contained_kernel_can_write_its_family_board() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("family")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    // Under a tmp root the board would be writable anyway and the test would prove nothing.
    let probe = uncovered(&sandbox, &home).ok_or("no directory outside the sandbox")?;
    let family = probe.join(format!("yi-family-{}", std::process::id()));
    std::fs::create_dir_all(&family)?;
    let kernel = service(project, home, sandbox, Some(family.clone()));
    let put = cell(
        &kernel,
        "print(rlm.put('shard', [1, 2])['name'])".to_owned(),
    )
    .await;
    kernel.dispose().await;
    let written = family.join("shard.dill").is_file();
    let _ = std::fs::remove_dir_all(&family);
    let put = put?;
    assert!(
        written,
        "the board refused the put: {} {:?}",
        put.result.stderr, put.result.error
    );
    Ok(())
}

/// Incident (#584): `~/.yi/mcp` was a kernel writable root, and the host ran whatever
/// `sessions.json` named on a `fetch("mcp://…")`, a read Auto mode never asks about. The store
/// is host-only now (D296), and the token files under it are hidden from a cell.
#[tokio::test]
async fn a_root_kernel_cannot_plant_an_mcp_session_or_read_its_tokens() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, home, _session) = workspace("mcp")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&root.join("rlm")));
    // Under a tmp HOME the store sits in a granted root and the test would prove nothing.
    if uncovered(&sandbox, &home).as_deref() != Some(home.as_path()) {
        return Ok(());
    }
    let mcp = home.join(".yi").join("mcp");
    let store = mcp.join("sessions.json");
    let token = mcp
        .join("tokens")
        .join(format!("yi-584-{}.json", std::process::id()));
    std::fs::create_dir_all(mcp.join("tokens"))?;
    std::fs::write(&token, r#"{"accessToken":"yi-584-secret"}"#)?;
    let before = std::fs::read_to_string(&store).ok();
    let session = root_session(&project, &home, &root.join("rlm"), None, None, None);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let code = format!(
        "for path in (r'{store}', r'{token}'):\n    try:\n        open(path, 'a').write('')\n        print('wrote', path)\n    except OSError as e:\n        print('write', e.errno)\ntry:\n    print('read', open(r'{token}').read())\nexcept OSError as e:\n    print('read', e.errno)\nprint(await bash(\"echo x >> '{store}'\"))",
        store = store.display(),
        token = token.display(),
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let after = std::fs::read_to_string(&store).ok();
    let _ = std::fs::remove_file(&token);
    let _ = std::fs::remove_dir(mcp.join("tokens"));
    if before.is_none() {
        let _ = std::fs::remove_file(&store);
    }
    let stdout = ran?.result.stdout;
    assert!(
        stdout.matches("write 1\n").count() == 2 && stdout.contains("read 1\n"),
        "a cell must get EPERM on the store and on a token file: {stdout}"
    );
    assert_eq!(after, before, "the store must be untouched: {stdout}");
    Ok(())
}

struct RecordingMcp(Arc<std::sync::Mutex<Vec<(PathBuf, String, String)>>>);

impl yi_runtime::fetch::McpResourceRead for RecordingMcp {
    fn read(&self, _server: &str, _resource: &str) -> Result<String, String> {
        Err("no read in this test".to_owned())
    }

    fn connect(
        &self,
        config: &std::path::Path,
        entry: &str,
        session: &str,
    ) -> Result<String, String> {
        self.0.lock().map_err(|error| error.to_string())?.push((
            config.to_path_buf(),
            entry.to_owned(),
            session.to_owned(),
        ));
        Ok(
            r#"{"session":"@krnl-fixture","state":"live","server":{"tools":[{"name":"echo"}]}}"#
                .to_owned(),
        )
    }
}

/// Incident (#584): a kernel-side `connect` ran `yi mcp connect` inside the sandbox and wrote
/// the session store from there. It now asks the host, which resolves the name in
/// `~/.yi/mcp.json` alone: a path, a URL or a workspace file a cell could have written is refused.
#[tokio::test]
async fn a_kernel_connect_runs_on_the_host_from_the_home_config() -> TestResult {
    let (root, project, home, _session) = workspace("connect")?;
    let config = home.join(".yi").join("mcp.json");
    let created = !config.is_file();
    if created {
        std::fs::create_dir_all(home.join(".yi"))?;
        std::fs::write(&config, "{}")?;
    }
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let host: Arc<dyn yi_runtime::fetch::McpResourceRead> =
        Arc::new(RecordingMcp(Arc::clone(&calls)));
    let session = root_session(&project, &home, &root.join("rlm"), None, None, Some(host));
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let code = "import rlm.mcp as mcp\nprint(await mcp.list_tools('fixture'))\nfor bad in ('../evil.json', 'a:b', '/tmp/x.json'):\n    try:\n        await mcp.list_tools(bad)\n    except Exception as e:\n        print('refused:', bad)";
    let ran = cell(&kernel, code.to_owned()).await;
    kernel.dispose().await;
    if created {
        let _ = std::fs::remove_file(&config);
    }
    let stdout = ran?.result.stdout;
    let seen = calls.lock().map(|calls| calls.clone()).unwrap_or_default();
    assert_eq!(
        seen,
        vec![(config, "fixture".to_owned(), "krnl-fixture".to_owned())],
        "the host connects the named entry once: {stdout}"
    );
    assert!(
        stdout.contains("[{'name': 'echo'}]") && stdout.matches("refused:").count() == 3,
        "tools come back and every non-name is refused: {stdout}"
    );
    Ok(())
}

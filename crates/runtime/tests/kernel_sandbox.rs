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
        KernelBridge::execute_cell(service.as_ref(), &code, &cancelled, None)
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
pub(crate) fn uncovered(sandbox: &Sandbox, home: &std::path::Path) -> Option<PathBuf> {
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

/// #599: every contained spawn reaches loopback, so a connection file's key would let one
/// sandbox run code in another's kernel. Every profile hides them; each kernel reads its own.
#[tokio::test]
async fn a_kernel_reaches_loopback_and_no_other_kernels_key() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("keys")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let first = service(project.clone(), home.clone(), sandbox.clone(), None);
    let second = service(project.clone(), home.clone(), sandbox.clone(), None);
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = std::io::Write::write_all(&mut &stream, b"HOST");
        }
    });
    let own = cell(
        &first,
        "import ipykernel\nprint(ipykernel.get_connection_file())".to_owned(),
    )
    .await?;
    let file = own.result.stdout.trim().to_owned();
    let text = std::fs::read_to_string(&file)?;
    let key = serde_json::from_str::<serde_json::Value>(&text)?["key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or("the connection file names no key")?
        .to_owned();
    let code = format!(
        "import socket\nfor attempt in (lambda: open(r'{file}').read(), lambda: open(r'{file}', 'a').write(' ')):\n    try:\n        print(attempt())\n    except OSError as e:\n        print('denied', e.errno)\nprint(socket.create_connection(('127.0.0.1', {port}), 3).recv(4))"
    );
    let other = cell(&second, code).await;
    let context = yi_tools::ToolContext::new(project.clone());
    let timeout = std::time::Duration::from_secs(60);
    let command = format!("cat '{file}'");
    let catted =
        yi_tools::run_or_background(&command, &context, None, timeout, Some(&sandbox), None);
    first.dispose().await;
    second.dispose().await;
    let other = other?.result;
    let catted = match catted? {
        yi_tools::Run::Finished(capture) => format!("{}{}", capture.stdout, capture.stderr),
        _ => return Err("cat did not finish".into()),
    };
    let seen = (
        other.stdout.contains(&key) || catted.contains(&key),
        other.stdout.matches("denied 1\n").count(),
        other.stdout.contains("b'HOST'"),
    );
    assert_eq!(
        seen,
        (false, 2, true),
        "(key seen, denials, loopback reached): cell {other:?}, cat {catted}"
    );
    Ok(())
}

/// Review of #925 (F1): the kernel's re-allow was bound to its directory as resolved on disk,
/// where its own profile writes; a process outliving it planted a link, and the next boot
/// re-allowed the link's target.
#[tokio::test]
async fn a_link_planted_at_the_kernels_own_directory_grants_nothing() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("own-link")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    let probe = uncovered(&sandbox, &home).ok_or("no directory outside the sandbox")?;
    let target = probe.join(format!("yi-own-link-{}", std::process::id()));
    std::fs::create_dir_all(&target)?;
    let kernel = service(project.clone(), home.clone(), sandbox, None);
    let own = cell(
        &kernel,
        "import ipykernel, os\nprint(os.path.dirname(ipykernel.get_connection_file()))".to_owned(),
    )
    .await?
    .result
    .stdout
    .trim()
    .to_owned();
    let plant = format!(
        "import subprocess\nsubprocess.Popen(['/bin/sh', '-c', 'for i in $(seq 400); do [ -e \"$0/connection.json\" ] || {{ ln -s \"$1\" \"$0\"; exit; }}; sleep 0.05; done', r'{own}', r'{}'], start_new_session=True)",
        target.display()
    );
    cell(&kernel, plant).await?;
    kernel.kill().await;
    let own = PathBuf::from(own);
    for _ in 0..100 {
        if own.is_symlink() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let planted = own.is_symlink();
    let escape = target.join("escape.txt");
    let code = format!(
        "try:\n    open(r'{}', 'w').write('x')\n    print('wrote')\nexcept OSError as e:\n    print('denied', e.errno)",
        escape.display()
    );
    let next = cell(&kernel, code).await;
    kernel.dispose().await;
    let escaped = escape.exists();
    let _ = std::fs::remove_dir_all(&target);
    let next = next?.result.stdout;
    assert!(
        planted && !escaped && next.contains("denied 1"),
        "planted {planted}, escaped {escaped}: {next}"
    );
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
    let wall = yi_runtime::Wall::default();
    walled_session(project, home, rlm_dir, sessions_dir, broker, mcp_read, wall)
}

fn walled_session(
    project: &std::path::Path,
    home: &std::path::Path,
    rlm_dir: &std::path::Path,
    sessions_dir: Option<PathBuf>,
    broker: Option<Arc<yi_runtime::permission::PermissionBroker>>,
    mcp_read: Option<Arc<dyn yi_runtime::fetch::McpResourceRead>>,
    wall: yi_runtime::Wall,
) -> yi_runtime::AgentSession {
    let provider = Arc::new(yi_runtime::ProviderStream::new(None));
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
            wall,
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

/// #598, #889: a walled juror's kernel wrote the tree and read walled files, since the wall was
/// a check at the tool seam and in no profile; and a cell inherited `*_API_KEY`. The stores
/// themselves are proven in `tools/tests/sandbox.rs` with a scratch HOME, not the runner's.
#[tokio::test]
async fn a_walled_kernel_and_its_bash_keep_to_the_wall_and_inherit_no_key() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    // SAFETY: nextest runs each test in a process of its own, so no thread reads the env.
    unsafe { std::env::set_var("YI_598_PROBE_API_KEY", "ENV-598") };
    let (root, project, home, _session) = workspace("walled")?;
    std::fs::write(project.join("walled.txt"), "WALLED-598")?;
    std::os::unix::fs::symlink(&project, root.join("via"))?;
    let wall = yi_runtime::Wall {
        deny_write: vec![project.clone()],
        deny_read: vec![project.join("walled.txt")],
        ..yi_runtime::Wall::default()
    };
    let rlm = root.join("rlm");
    let session = walled_session(&project, &home, &rlm, None, None, None, wall);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let via = root.join("via");
    let (tree, via) = (project.display(), via.display());
    let code = format!(
        "import os\nfor path in (r'{tree}/walled.txt', r'{tree}/WALLED.TXT', r'{via}/walled.txt'):\n    try:\n        print(open(path).read())\n    except OSError as e:\n        print('denied', e.errno)\ntry:\n    open(r'{tree}/by-cell.txt', 'w').write('x')\nexcept OSError as e:\n    print('denied', e.errno)\nprint(os.environ.get('YI_598_PROBE_API_KEY'))\nprint(await bash(\"cat '{tree}/walled.txt'; touch '{tree}/by-job.txt'; env\"))",
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let wrote = ["by-cell.txt", "by-job.txt"].map(|name| project.join(name).exists());
    let stdout = ran?.result.stdout;
    let read = ["WALLED-598", "ENV-598"].map(|secret| stdout.contains(secret));
    // Three reads and a write refused with EPERM, and the job ran: proof the cell got that far.
    let ran = (
        stdout.matches("denied 1\n").count(),
        stdout.contains("PATH="),
    );
    assert!(
        wrote == [false, false] && read == [false, false] && ran == (4, true),
        "wrote (cell, job) {wrote:?}, read (walled, env) {read:?}, (denials, job ran) {ran:?}"
    );
    Ok(())
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

/// #600 stage 2c: a `bash()` job ran under the profile captured when the session was wired, so a
/// grant kept later, here replayed from the ledger on `--continue`, never reached the kernel's jobs.
#[tokio::test]
async fn a_kernel_bash_job_takes_a_grant_kept_after_wiring() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, home, _session) = workspace("grant")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let probe = uncovered(&sandbox, &home).ok_or("no directory outside the sandbox")?;
    let granted = probe.join(format!("yi-job-grant-{}", std::process::id()));
    std::fs::create_dir_all(&granted)?;
    let (events, _keep) = tokio::sync::broadcast::channel(16);
    let broker = Arc::new(
        yi_runtime::permission::PermissionBroker::new(
            yi_runtime::PermissionMode::Auto,
            project.clone(),
            Vec::new(),
            None,
            events,
        )
        .with_sandbox(Some(sandbox)),
    );
    let session = root_session(&project, &home, &root.join("rlm"), None, Some(broker), None);
    let store = crate::support::memory_store("job-grant");
    let grant = yi_permission::write_grant(&granted);
    let rule = serde_json::json!({
        "id": 1, "kind": "command", "canonical": grant.canonical,
        "displayIdentity": grant.label, "decision": "allow", "generation": 1
    });
    yi_session::lock_session(&store).append_custom("main", "permission_rule", Some(rule))?;
    session.attach_store(store)?;
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let made = granted.join("made");
    let code = format!(
        "print(await bash(\"touch '{}' && echo made\"))",
        made.display()
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let wrote = made.is_file();
    let _ = std::fs::remove_dir_all(&granted);
    let ran = ran?;
    assert!(
        wrote,
        "the job's profile lacks the kept grant: {}",
        ran.result.stdout
    );
    Ok(())
}

/// Incident: a child kernel's writable roots stopped at its own `sub-*` directory, so its
/// `rlm.put` to the family board it shares with its parent failed with EPERM. Then (#757) the
/// profile granted `family/<id>` while nothing made `family/`, so the kernel's mkdir was denied.
#[tokio::test]
async fn a_contained_kernel_can_write_its_family_board() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, session) = workspace("family")?;
    let sandbox = Sandbox::for_workspace(&project, &home, Some(&session));
    // Under a tmp root the board would be writable anyway and the test would prove nothing.
    let probe = uncovered(&sandbox, &home).ok_or("no directory outside the sandbox")?;
    let sessions = probe.join(format!("yi-family-{}", std::process::id()));
    let family = sessions.join("family").join("01test");
    let kernel = service(project, home, sandbox, Some(family.clone()));
    let put = cell(
        &kernel,
        "print(rlm.put('shard', [1, 2])['name'])".to_owned(),
    )
    .await;
    kernel.dispose().await;
    let written = family.join("shard.dill").is_file();
    let _ = std::fs::remove_dir_all(&sessions);
    let put = put?;
    assert!(
        written,
        "the board refused the put: {} {:?}",
        put.result.stderr, put.result.error
    );
    Ok(())
}

/// Incident (#757): `rlm.put` from the root kernel raised PermissionError on
/// `~/.yi/sessions/family`, and a `bash()` job could not write the board either.
#[tokio::test]
async fn a_root_kernels_board_takes_puts_and_bash_jobs() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, _session) = workspace("board")?;
    let bare = Sandbox::for_workspace(&project, &home, None);
    // Under a tmp root the corpus would be writable anyway and the test would prove nothing.
    let probe = uncovered(&bare, &home).ok_or("no directory outside the sandbox")?;
    let corpus = probe.join(format!("yi-board-{}", std::process::id()));
    std::fs::create_dir_all(&corpus)?;
    let rlm_dir = corpus.join("rlm-1");
    let session = root_session(&project, &home, &rlm_dir, Some(corpus.clone()), None, None);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let board = rlm_dir.join("family");
    let code = format!(
        "try:\n    rlm.put('shard', [1])\nexcept OSError as e:\n    print(e)\nprint(await bash(\"echo x > '{}' && echo ok\"))",
        board.join("by-job.txt").display()
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let (put, job) = (
        board.join("shard.dill").is_file(),
        board.join("by-job.txt").is_file(),
    );
    let _ = std::fs::remove_dir_all(&corpus);
    let ran = ran?;
    assert!(
        put && job,
        "the root kernel's board refused a write (put {put}, bash {job}): {} {:?}",
        ran.result.stdout,
        ran.result.error
    );
    Ok(())
}

/// Incident (#855 review): a kernel whose board did not exist yet planted it as a link to
/// `~/.docker`, and the host's `family://` read followed it past the kernel's read denial.
/// `outside` stands in for the credential dir, which a test must never write.
#[tokio::test]
async fn a_link_planted_as_the_board_is_replaced_not_followed() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, home, _session) = workspace("board-link")?;
    let bare = Sandbox::for_workspace(&project, &home, None);
    let probe = uncovered(&bare, &home).ok_or("no directory outside the sandbox")?;
    let corpus = probe.join(format!("yi-board-link-{}", std::process::id()));
    let (rlm_dir, outside) = (corpus.join("rlm-1"), corpus.join("outside"));
    let board = rlm_dir.join("family");
    std::fs::create_dir_all(&rlm_dir)?;
    std::fs::create_dir_all(&outside)?;
    std::fs::write(outside.join("config.json"), "yi-855-probe-secret")?;
    // A board an older binary let a kernel plant stays a link until the host replaces it.
    std::os::unix::fs::symlink(&outside, &board)?;
    let session = root_session(&project, &home, &rlm_dir, Some(corpus.clone()), None, None);
    let kernel = session
        .kernel_service()
        .ok_or("the wiring installs a kernel")?;
    let (board_text, outside_text) = (board.display(), outside.display());
    let code = format!(
        "import os\ntry:\n    os.rmdir(r'{board_text}')\n    os.symlink(r'{outside_text}', r'{board_text}')\nexcept OSError as e:\n    print(e)\ntry:\n    print(await fetch('family://config'))\nexcept Exception as e:\n    print(e)"
    );
    let ran = cell(&kernel, code).await;
    kernel.dispose().await;
    let real = std::fs::symlink_metadata(&board).is_ok_and(|seen| seen.is_dir());
    let kept = std::fs::read_to_string(outside.join("config.json")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&corpus);
    let stdout = ran?.result.stdout;
    assert!(
        real && kept == "yi-855-probe-secret" && !stdout.contains("yi-855-probe-secret"),
        "a board link was kept ({}) or followed: {stdout}",
        !real
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

    fn connect_server(
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

/// #889: a walled session's kernel read every other session's spills and transcripts, since
/// its profile carried the wall but not the walled roots the tool seam adds (D340, #971). Its
/// own spill, transcript, state and the family board still read; an unwalled kernel reads all.
#[tokio::test]
async fn a_walled_kernel_reads_its_own_spill_and_transcript_and_no_other() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, real, _session) = workspace("walled-roots")?;
    let bare = Sandbox::for_workspace(&project, &real, None);
    // Outside every granted root, so a write to the board or its own dir proves the spare.
    let probe = uncovered(&bare, &real).ok_or("no directory outside the sandbox")?;
    let home = probe.join(format!("yi-walled-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let venv = yi_kernel::bootstrap::kernel_venv_dir(&real);
    std::fs::create_dir_all(home.join(".yi"))?;
    if venv.is_dir() {
        std::os::unix::fs::symlink(&venv, yi_kernel::bootstrap::kernel_venv_dir(&home))?;
    }
    // SAFETY: nextest runs each test in a process of its own, so no thread reads the env.
    unsafe { std::env::set_var("HOME", &home) };
    let ran = walled_roots_probe(&root, &project, &home).await;
    let _ = std::fs::remove_dir_all(&home);
    let (outputs, [id, root_id]) = ran?;
    let [walled, named, root_run, open] =
        <[String; 4]>::try_from(outputs).map_err(|_| "four runs")?;
    // A walled root reads its own transcript and its `kernels/<id>` state, never the corpus.
    let root_own = (
        root_run.contains(&format!("\"id\":\"{root_id}\"")),
        root_run.contains("own state"),
        root_run.contains("WALLED"),
    );
    assert_eq!(
        root_own,
        (true, true, false),
        "(transcript, state, leak): {root_run}"
    );
    // #1000 review F1: a spare never reopens what the wall itself names, by read or write.
    assert!(
        named.matches("denied 1").count() == 5
            && !named.contains("NOTE")
            && !named.contains("SPILL")
            && !named.contains("WROTE"),
        "the wall's own deny_read lost to a spare: {named}"
    );
    for leak in ["WALLED TRANSCRIPT", "WALLED OUTPUT"] {
        assert!(
            !walled.contains(leak),
            "{leak} reached a walled kernel: {walled}"
        );
    }
    // Its own spill and state twice, by the cell and by its `bash()` job.
    let own = (
        walled.matches("OWN SPILL").count() + walled.matches("STATE NOTE").count(),
        walled.contains(&format!("\"id\":\"{id}\"")),
        walled.contains("board [1]"),
    );
    assert_eq!(
        own,
        (4, true, true),
        "(spill, transcript, state and board): {walled}"
    );
    assert!(
        open.contains("WALLED TRANSCRIPT") && open.contains("WALLED OUTPUT"),
        "an unwalled kernel reads every store, as its bash does: {open}"
    );
    Ok(())
}

async fn walled_roots_probe(
    root: &std::path::Path,
    project: &std::path::Path,
    home: &std::path::Path,
) -> Result<(Vec<String>, [String; 2]), Box<dyn Error>> {
    let planted = crate::wall_e2e::plant_transcripts(home, project)?;
    let (spills, sessions) = (home.join(".yi/spills"), &planted.sessions);
    let elsewhere = home.join("store/--elsewhere--/1_other.jsonl");
    for (path, text) in [
        (spills.join("author/0123.txt"), "WALLED OUTPUT"),
        (home.join(".yi/tool-output/4567.txt"), "WALLED OUTPUT"),
        (elsewhere.clone(), "WALLED TRANSCRIPT"),
    ] {
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(path, text)?;
    }
    std::os::unix::fs::symlink(sessions, project.join("alias"))?;
    let store = yi_session::create_flat_session(
        planted.own_dir.clone(),
        project.to_string_lossy(),
        Some("author".to_owned()),
    )?;
    let (id, transcript) = {
        let store = yi_session::lock_session(&store);
        (store.metadata().id.clone(), store.file_path().cloned())
    };
    let transcript = transcript.ok_or("no transcript file")?;
    let own_spill = spills.join(&id).join("0001.txt");
    std::fs::create_dir_all(own_spill.parent().ok_or("no parent")?)?;
    std::fs::write(&own_spill, "OWN SPILL")?;
    let author = planted.author.display().to_string();
    let name = planted.author.strip_prefix(sessions)?.display();
    let reads = [
        author.clone(),
        author.replace("/.yi/sessions/", "/.YI/SESSIONS/"),
        format!("{}/../../../100_author.jsonl", planted.own_dir.display()),
        format!("{}/alias/{name}", project.display()),
        planted.sibling.display().to_string(),
        elsewhere.display().to_string(),
        spills.join("author/0123.txt").display().to_string(),
        format!("{}/.YI/SPILLS/author/0123.txt", home.display()),
        home.join(".yi/tool-output/4567.txt").display().to_string(),
        own_spill.display().to_string(),
        transcript.display().to_string(),
    ];
    let code = format!(
        "import glob, os\nfor path in {reads:?}:\n    try:\n        print(open(path).read())\n    except OSError as e:\n        print('denied', e.errno)\nfor root in ({sessions:?}, {spills:?}):\n    for path in glob.glob(root + '/**/*', recursive=True):\n        try:\n            print(open(path).read())\n        except OSError as e:\n            pass\nstate = os.environ['RLM_SESSION_DIR'] + '/note.txt'\nopen(state, 'w').write('STATE NOTE')\nprint(open(state).read())\nrlm.put('shard', [1])\nprint('board', rlm.get('shard'))\nprint('job', await bash(\"cat '{author}' '{own}' '{state}'\"))",
        sessions = sessions.display().to_string(),
        spills = spills.display().to_string(),
        own = own_spill.display(),
        state = planted.own_dir.join("note.txt").display(),
    );
    let broker = yi_runtime::permission::PermissionBroker::new(
        yi_permission::PermissionMode::Auto,
        project.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    )
    .with_session_store(&home.join("store"));
    let wall = crate::wall_e2e::juror_wall(project);
    let broker = Some(Arc::new(broker.for_child(&wall, project)));
    // The parent's wall covers the board and the own dir, every spill, and a dir in the own dir.
    let (board, secret) = (
        planted.own_dir.with_file_name("family"),
        planted.own_dir.join("secret"),
    );
    for dir in [&board, &secret] {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("note.txt"), "NAMED NOTE")?;
    }
    let mut named = wall.clone();
    let children = planted.own_dir.parent().ok_or("no parent")?.to_path_buf();
    named.deny_read = vec![children, spills.clone(), secret.clone()];
    let probes = [
        board.join("note.txt"),
        secret.join("note.txt"),
        own_spill.clone(),
    ];
    let writes = [board.join("by-cell.txt"), secret.join("by-cell.txt")];
    let named_code = format!(
        "for path in {probes:?}:\n    try:\n        print(open(path).read())\n    except OSError as e:\n        print('denied', e.errno)\nfor path in {writes:?}:\n    try:\n        open(path, 'w').write('x')\n    except OSError as e:\n        print('denied', e.errno)",
    );
    // A walled root (depth 0) whose sessions dir is the corpus: its state is `kernels/<id>`.
    let root_store = yi_session::create_flat_session(
        planted.author.parent().ok_or("no parent")?.to_path_buf(),
        project.to_string_lossy(),
        None,
    )?;
    let (root_id, root_file) = {
        let store = yi_session::lock_session(&root_store);
        (store.metadata().id.clone(), store.file_path().cloned())
    };
    let root_file = root_file.ok_or("no root transcript")?.display().to_string();
    let root_code = format!(
        "import os\nstate = os.environ['RLM_SESSION_DIR'] + '/note.txt'\nopen(state, 'w').write('state')\nprint('own', open(state).read())\nfor path in ({root_file:?}, {author:?}):\n    try:\n        print(open(path).read())\n    except OSError as e:\n        print('denied', e.errno)",
    );
    let mut outputs = Vec::new();
    let own = (planted.own_dir.clone(), None, Some(store));
    let runs = [
        (wall.clone(), broker.clone(), code.clone(), own.clone()),
        (named, broker.clone(), named_code, own),
        (
            wall,
            broker,
            root_code,
            (
                root.join("rlm-root"),
                Some(sessions.clone()),
                Some(root_store),
            ),
        ),
        (
            yi_runtime::Wall::default(),
            None,
            code,
            (root.join("open"), None, None),
        ),
    ];
    for (wall, broker, code, (rlm, corpus, store)) in runs {
        let session = walled_session(project, home, &rlm, corpus, broker, None, wall);
        if let Some(store) = store {
            session.attach_store(store)?;
        }
        let kernel = session
            .kernel_service()
            .ok_or("the wiring installs a kernel")?;
        let ran = cell(&kernel, code).await;
        kernel.dispose().await;
        let ran = ran?;
        outputs.push(format!("{}{:?}", ran.result.stdout, ran.result.error));
    }
    if writes.iter().any(|path| path.exists()) {
        outputs[1].push_str("WROTE");
    }
    Ok((outputs, [id, root_id]))
}

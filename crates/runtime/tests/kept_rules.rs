#![cfg(target_os = "macos")]
//! #600 stage 2c: a kept rule is a `permission_rule` entry in the session JSONL, replayed on
//! `--continue` and re-checked as it is; a child starts with a copy of its parent's rules and
//! keeps its own, so nothing it keeps reaches the parent or the parent's ledger.

use crate::sandbox_seam::{Probe, always_once, bash_call, faux_model, results, workspace};
use crate::scratch::Scratch;
use crate::support;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::permission::Containment;
use yi_runtime::{
    AgentSession, AskOutcome, Asker, PermissionBroker, PermissionMode, ProviderStream,
    SessionConfig, Wall, builtin_tools,
};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_tools::Sandbox;
use yi_types::message::StopReason;

type TestResult = Result<(), Box<dyn Error>>;

const RULE_ENTRY: &str = "permission_rule";

/// Every question answered `answer`, counted.
fn answering(answer: AskOutcome) -> (Asker, Arc<AtomicUsize>) {
    let asks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asks);
    let asker: Asker = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        answer
    });
    (asker, asks)
}

fn new_session(provider: &Arc<ProviderStream>) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(provider),
    )
}

fn broker(
    session: &AgentSession,
    project: &Path,
    sandbox: Sandbox,
    asker: Asker,
) -> Arc<PermissionBroker> {
    Arc::new(
        PermissionBroker::new(
            PermissionMode::Auto,
            project.to_path_buf(),
            Vec::new(),
            Some(asker),
            session.events_sender(),
        )
        .with_sandbox(Some(sandbox)),
    )
}

/// A session with the builtin tools under a broker for `project`, run from `holder`.
fn held(
    project: &Path,
    holder: &Path,
    sandbox: Sandbox,
    asker: Asker,
) -> (AgentSession, Arc<ProviderStream>) {
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = new_session(&provider);
    let gate = broker(&session, project, sandbox, asker);
    session.use_tools(builtin_tools(), holder.to_path_buf(), Some(gate));
    (session, provider)
}

/// One turn of queued bash calls; the results of this turn only, as a resumed history holds
/// the earlier ones too.
async fn turn(
    session: &AgentSession,
    provider: &ProviderStream,
    commands: &[&str],
) -> Result<Vec<String>, Box<dyn Error>> {
    let before = results(session).len();
    provider.queue_faux(
        commands
            .iter()
            .enumerate()
            .map(|(index, command)| bash_call(&format!("call-{before}-{index}"), command))
            .collect(),
    );
    session.prompt("do the thing")?;
    session.wait_idle().await;
    let now = results(session);
    assert_eq!(now.len(), before + commands.len(), "{now:?}");
    Ok(now[before..].to_vec())
}

fn listed(session: &AgentSession) -> String {
    yi_runtime::slash::run(session, "permissions", "").unwrap_or_default()
}

fn kept_entries(store: &yi_session::SharedSession) -> usize {
    let query = yi_session::EntryQuery {
        custom_type: Some(RULE_ENTRY.to_owned()),
        ..yi_session::EntryQuery::default()
    };
    (yi_session::lock_session(store).find_entries(&query))
        .map(|entries| entries.len())
        .unwrap_or(0)
}

fn widen_of(broker: &PermissionBroker, command: &str) -> Vec<PathBuf> {
    let mut args = Map::new();
    args.insert("command".to_owned(), Value::from(command));
    let outcome = broker.decide_call(
        "bash",
        yi_tools::ToolKind::Exec,
        false,
        "probe",
        &args,
        None,
    );
    match outcome.containment {
        Containment::Contained { widen, .. } => widen,
        Containment::Uncontained => Vec::new(),
    }
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Plan red test 2: the kept grant lived only in the broker's memory, so `--continue` asked
/// again for a directory the user had already said "always" to.
#[tokio::test]
async fn a_kept_grant_still_applies_after_continue() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-kept-continue")?;
    let dir = Probe::new(&probe, "continue")?;
    let (first, second) = (dir.0.join("first"), dir.0.join("second"));
    let touch = |path: &Path| format!("touch {}", path.display());
    let mut repo = JsonlRepo::new(root.join("sessions"), project.to_string_lossy());
    let store = repo.create(CreateOptions {
        id: Some("kept".to_owned()),
        ..CreateOptions::default()
    })?;
    let (always, _) = answering(AskOutcome::AllowAlways(0));
    let (session, provider) = held(&project, &project, sandbox.clone(), always);
    session.attach_store(Arc::clone(&store))?;
    let ran = turn(&session, &provider, &[&touch(&first), &touch(&first)]).await?;
    assert!(first.exists(), "the approved retry writes: {}", ran[1]);
    drop((session, store));

    let (refuse, asks) = answering(AskOutcome::Reject);
    let (resumed, provider) = held(&project, &project, sandbox, refuse);
    resumed.attach_store(repo.open("kept")?)?;
    let ran = turn(&resumed, &provider, &[&touch(&second)]).await?;
    assert!(
        second.exists(),
        "after --continue the kept grant still applies: {}",
        ran[0]
    );
    assert_eq!(asks.load(Ordering::SeqCst), 0, "and nobody is asked again");
    let listing = listed(&resumed);
    assert!(
        listing.contains(&real(&dir.0).display().to_string()),
        "/permissions lists the kept grant: {listing}"
    );
    Ok(())
}

/// The session pass #933 added ("Always = session pass") is kept for the session, and a
/// resumed session is the same session; it stays the exact bytes and the directory it named.
#[tokio::test]
async fn a_kept_pass_survives_continue_byte_exact_and_bound_to_its_directory() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, _probe) = workspace("yi-kept-pass")?;
    let other = root.join("other");
    std::fs::create_dir_all(&other)?;
    let nest = "import subprocess\nsubprocess.run(['sandbox-exec', '-p', '(version 1)(allow default)', 'true'], check=True)\nprint('nested-ok')\n";
    for dir in [&project, &other] {
        std::fs::write(dir.join("nest.py"), nest)?;
    }
    let nested = "python3 nest.py";
    let mut repo = JsonlRepo::new(root.join("sessions"), project.to_string_lossy());
    let store = repo.create(CreateOptions {
        id: Some("pass".to_owned()),
        ..CreateOptions::default()
    })?;
    let (always, _) = answering(AskOutcome::AllowAlways(0));
    let (session, provider) = held(&project, &project, sandbox.clone(), always);
    session.attach_store(Arc::clone(&store))?;
    let ran = turn(&session, &provider, &[nested, nested]).await?;
    assert!(
        ran[1].contains("nested-ok"),
        "the approved retry ran: {}",
        ran[1]
    );
    drop((session, store));

    let (refuse, asks) = answering(AskOutcome::Reject);
    let (resumed, provider) = held(&project, &project, sandbox.clone(), refuse);
    resumed.attach_store(repo.open("pass")?)?;
    let ran = turn(&resumed, &provider, &[nested, nested, "python3  nest.py"]).await?;
    assert!(
        !ran[0].contains("nested-ok"),
        "the pass is no standing escalation: {}",
        ran[0]
    );
    assert!(
        ran[1].contains("nested-ok") && ran[1].contains("outside the sandbox"),
        "after --continue the kept pass still passes: {}",
        ran[1]
    );
    assert!(
        ran[2].contains("Permission denied") && !ran[2].contains("nested-ok"),
        "one byte more is another command: {}",
        ran[2]
    );
    assert_eq!(asks.load(Ordering::SeqCst), 1);

    let (refuse, asks) = answering(AskOutcome::Reject);
    let (elsewhere, provider) = held(&other, &other, sandbox, refuse);
    elsewhere.attach_store(repo.open("pass")?)?;
    let ran = turn(&elsewhere, &provider, &[nested, nested]).await?;
    assert!(
        !ran[1].contains("nested-ok"),
        "the pass is bound to the directory it was kept in: {}",
        ran[1]
    );
    assert_eq!(asks.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Replay re-checks each rule against today's credential list and wall: a grant on a path
/// that is protected now, directly or through a link, is dropped, whatever was kept then.
#[tokio::test]
async fn replay_drops_a_grant_on_a_path_protected_since() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-kept-replay")?;
    let (kept, walled) = (Probe::new(&probe, "kept")?, Probe::new(&probe, "walled")?);
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    // Kept as a plain directory, swapped since for a link into the wall.
    let linked = Probe::new(&probe, "linked")?;
    std::fs::remove_dir(&linked.0)?;
    std::os::unix::fs::symlink(&walled.0, &linked.0)?;
    let store = support::memory_store("replay");
    let dirs = [&kept.0, &walled.0, &linked.0, &home.join(".ssh/agent")];
    for dir in dirs {
        let grant = yi_permission::write_grant(dir);
        let rule = json!({
            "id": 1, "kind": "command", "canonical": grant.canonical,
            "displayIdentity": grant.label, "decision": "allow", "generation": 1
        });
        yi_session::lock_session(&store).append_custom("main", RULE_ENTRY, Some(rule))?;
    }
    let (refuse, _) = answering(AskOutcome::Reject);
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = new_session(&provider);
    let gate = broker(&session, &project, sandbox, refuse);
    session.set_wall(Wall {
        deny_write: vec![walled.0.clone()],
        ..Wall::default()
    });
    session.use_tools(builtin_tools(), project.clone(), Some(Arc::clone(&gate)));
    session.attach_store(store)?;
    assert_eq!(
        widen_of(&gate, "touch x"),
        vec![kept.0.clone()],
        "only the grant still unprotected is replayed: {}",
        listed(&session)
    );
    Ok(())
}

/// Plan red test 3, and the ledger half: every child shared its parent's broker, so a grant
/// the child kept widened the parent's next contained run.
#[tokio::test(flavor = "multi_thread")]
async fn a_childs_kept_grant_reaches_neither_its_parent_nor_the_parents_ledger() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-kept-child")?;
    let dir = Probe::new(&probe, "child")?;
    let made = dir.0.join("made");
    let (always, _) = answering(AskOutcome::AllowAlways(0));
    let (session, provider, gate, host) = family_root(&root, &project, sandbox, always)?;
    let store = support::memory_store("kept-parent");
    session.attach_store(Arc::clone(&store))?;
    let touch = format!("touch {}", made.display());
    provider.queue_faux(vec![
        bash_call("child-1", &touch),
        bash_call("child-2", &touch),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    host.spawn("keep a grant".to_owned(), root_child(&[]))?;
    until(|| made.exists()).await;
    assert!(made.exists(), "the child's approved retry wrote");
    assert!(
        !widen_of(&gate, &touch).contains(&real(&dir.0)),
        "the child's grant does not widen the parent: {}",
        listed(&session)
    );
    assert_eq!(kept_entries(&store), 0, "nor lands in the parent's ledger");
    Ok(())
}

/// Plan red test 5 with a kept rule: the parent's pass runs one command outside the sandbox,
/// where no wall holds, so a juror walled off the tree must not inherit it.
#[tokio::test(flavor = "multi_thread")]
async fn a_walled_child_does_not_inherit_the_parents_pass() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, _probe) = workspace("yi-kept-juror")?;
    // The wall is a subtree, not the cwd: a refusal text is read for paths under the cwd.
    let jury = project.join("jury");
    std::fs::create_dir_all(&jury)?;
    let victim = jury.join("victim");
    std::fs::write(
        project.join("nest.py"),
        format!(
            "import subprocess\nsubprocess.run(['sandbox-exec', '-p', '(version 1)(allow default)', 'true'], check=True)\nopen('{}', 'w').write('x')\n",
            victim.display()
        ),
    )?;
    let (asker, asks) = always_once();
    let (session, provider, _gate, host) = family_root(&root, &project, sandbox, asker)?;
    let nested = "python3 nest.py";
    let ran = turn(&session, &provider, &[nested, nested]).await?;
    assert!(
        victim.exists(),
        "the parent's approved retry ran: {}",
        ran[1]
    );
    std::fs::remove_file(&victim)?;
    provider.queue_faux(vec![
        bash_call("juror-1", nested),
        bash_call("juror-2", nested),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let wall = [("deny_write", json!([jury.display().to_string()]))];
    host.spawn("judge it".to_owned(), root_child(&wall))?;
    let finished = || Value::Object(host.status())["members"][0]["state"] == "finished";
    until(|| victim.exists() || finished()).await;
    assert!(finished(), "the juror ran: {:?}", host.status());
    assert!(
        !victim.exists(),
        "the juror's retry ran outside its wall on the parent's pass"
    );
    assert_eq!(asks.load(Ordering::SeqCst), 2, "the juror's retry asks");
    Ok(())
}

type Root = (
    AgentSession,
    Arc<ProviderStream>,
    Arc<PermissionBroker>,
    Arc<yi_runtime::SubagentHost>,
);

/// A root wired as `yi` wires one, whose children come from the real child factory.
fn family_root(
    root: &Scratch,
    project: &Path,
    sandbox: Sandbox,
    asker: Asker,
) -> Result<Root, Box<dyn Error>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = new_session(&provider);
    let gate = broker(&session, project, sandbox, asker);
    let host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider: Arc::clone(&provider),
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: project.to_path_buf(),
            home,
            lane_slots: 1,
            broker: Some(Arc::clone(&gate)),
            tools: Arc::new(builtin_tools),
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
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    Ok((session, provider, gate, host))
}

fn root_child(pairs: &[(&str, Value)]) -> Map<String, Value> {
    std::iter::once(("role", json!("root")))
        .chain(pairs.iter().cloned())
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Polls, never sleeps blind: returns once `done` holds, or after twenty seconds.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

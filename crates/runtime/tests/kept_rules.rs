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
    assert!(
        !victim.exists(),
        "the juror's retry ran outside its wall on the parent's pass"
    );
    assert!(finished(), "the juror ran: {:?}", host.status());
    // #1001: a walled retry of a pathless refusal never leaves, so nobody is asked.
    assert_eq!(
        asks.load(Ordering::SeqCst),
        1,
        "the juror's retry is refused unasked"
    );
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

/// One `permission_rule` entry in the record's own shape, as the broker journals it.
fn ledger(
    store: &yi_session::SharedSession,
    kind: &str,
    canonical: &str,
    label: &str,
) -> Result<String, Box<dyn Error>> {
    let rule = json!({
        "id": 1, "kind": kind, "canonical": canonical,
        "displayIdentity": label, "decision": "allow", "generation": 1
    });
    Ok(yi_session::lock_session(store).append_custom("main", RULE_ENTRY, Some(rule))?)
}

fn kept_of(session: &AgentSession) -> Vec<String> {
    (session.permission_broker()).map_or_else(Vec::new, |broker| broker.kept_rules())
}

/// Review of #938: an exact `write` call's identity began as a write grant's does, so the
/// call's arguments, file content and all, were read back as a directory and rode the
/// `sandbox-exec` argv of every later contained spawn, where `ps` shows them.
#[tokio::test]
async fn an_exact_write_call_is_no_write_grant_and_rides_no_argv() -> TestResult {
    let (_root, project, sandbox, probe) = workspace("yi-kept-exact-write")?;
    let outside = Probe::new(&probe, "env")?;
    let secret = "SECRET=hunter2";
    let mut args = Map::new();
    args.insert("path".to_owned(), json!(outside.0.join(".env")));
    args.insert("content".to_owned(), json!(secret));
    let provider = Arc::new(ProviderStream::new(None));
    let session = new_session(&provider);
    let (always, asks) = answering(AskOutcome::AllowAlways(0));
    let gate = broker(&session, &project, sandbox, always);
    let kind = yi_tools::ToolKind::Write;
    let outcome = gate.decide_call("write", kind, false, "write-1", &args, None);
    assert!(outcome.allowed, "{}", outcome.reason);
    assert_eq!(
        asks.load(Ordering::SeqCst),
        1,
        "the write asked and was kept"
    );
    assert_eq!(gate.kept_rules().len(), 1, "{:?}", gate.kept_rules());
    let widen = widen_of(&gate, "touch x");
    let profile = gate
        .sandbox_for(&project, &Wall::default(), &widen)
        .ok_or("no profile")?;
    let (_, argv) = profile.wrap("true", &[]);
    let kept = format!("{:?} {argv:?}", gate.kept_writes());
    assert!(
        !kept.contains("hunter2"),
        "a kept call rides the argv: {kept}"
    );
    Ok(())
}

/// A session with the builtin tools under a broker for `project` and a JSONL store `a` that
/// kept one pass after its first message, whose id is returned with the store's repo.
fn switching(tag: &str) -> Result<(Scratch, AgentSession, JsonlRepo, String), Box<dyn Error>> {
    let (root, project, sandbox, _probe) = workspace(tag)?;
    let mut repo = JsonlRepo::new(root.join("sessions"), project.to_string_lossy());
    let store = repo.create(CreateOptions {
        id: Some("a".to_owned()),
        ..CreateOptions::default()
    })?;
    let pass =
        yi_permission::canonical_command_identity("python3 nest.py", &project.to_string_lossy());
    let first =
        serde_json::from_value(json!({"role": "user", "content": "run it", "timestamp": 0}))?;
    let entry = yi_session::lock_session(&store).append_message("main", first)?;
    ledger(
        &store,
        "command",
        &pass,
        "this exact command: python3 nest.py",
    )?;
    let (refuse, _) = answering(AskOutcome::Reject);
    let (session, _provider) = held(&project, &project, sandbox, refuse);
    session.attach_store(store)?;
    assert_eq!(kept_of(&session).len(), 1, "session a replays its pass");
    Ok((root, session, repo, entry))
}

/// Review of #938: the TUI's `/new` resets the session and attaches a new store; the broker
/// kept session a's pass, so session b listed it and ran under it.
#[tokio::test]
async fn a_new_session_keeps_none_of_the_last_ones_rules() -> TestResult {
    let (_root, session, mut repo, _) = switching("yi-kept-new")?;
    session.reset();
    session.attach_store(repo.create(CreateOptions::default())?)?;
    assert_eq!(kept_of(&session), Vec::<String>::new());
    Ok(())
}

/// Rpc `switch_session`: session b's own rules and none of a's.
#[tokio::test]
async fn a_switched_to_session_keeps_only_its_own_rules() -> TestResult {
    let (root, session, mut repo, _) = switching("yi-kept-switch")?;
    let other = repo.create(CreateOptions {
        id: Some("b".to_owned()),
        ..CreateOptions::default()
    })?;
    let dir = root.join("granted");
    let grant = yi_permission::write_grant(&dir);
    ledger(&other, "command", &grant.canonical, &grant.label)?;
    let path = yi_session::lock_session(&other)
        .file_path()
        .cloned()
        .ok_or("session b has no file")?;
    drop(other);
    session.reset();
    let loaded = yi_session::load_session(&path)?;
    session.attach_store(Arc::new(std::sync::Mutex::new(loaded)))?;
    assert_eq!(kept_of(&session), vec![grant.label]);
    Ok(())
}

/// Rpc `fork`: a fork taken before the message the rule followed carries none of it.
#[tokio::test]
async fn a_fork_from_before_a_rule_keeps_none_of_it() -> TestResult {
    let (_root, session, mut repo, entry) = switching("yi-kept-fork")?;
    let scope = yi_session::ForkScope::Branch {
        entry_id: Some(entry),
        position: Some(yi_session::ForkPosition::Before),
    };
    let forked = repo.fork("a", &scope, CreateOptions::default())?;
    session.reset();
    session.attach_store(forked)?;
    assert_eq!(kept_of(&session), Vec::<String>::new());
    Ok(())
}

/// Review of #938: with yi started from `$HOME` (or `--session-dir` inside the tree) the
/// session's own JSONL sat under a writable root, so a contained bash call or an in-tree
/// `write` could append a forged pass for the next `--continue` to replay. Nextest runs each
/// test in its own process, so the HOME set here reaches no other test.
#[tokio::test]
async fn the_session_ledger_is_out_of_reach_of_a_contained_write_and_the_write_tool() -> TestResult
{
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, _project, _sandbox, probe) = workspace("yi-kept-ledger")?;
    let home = Probe::new(&probe, "home")?;
    unsafe { std::env::set_var("HOME", &home.0) };
    let mut repo = JsonlRepo::new(home.0.join(".yi/sessions"), home.0.to_string_lossy());
    let store = repo.create(CreateOptions::default())?;
    let file = yi_session::lock_session(&store)
        .file_path()
        .cloned()
        .ok_or("no session file")?;
    let sandbox = Sandbox::for_workspace(&home.0, &home.0, None);
    let (refuse, _) = answering(AskOutcome::Reject);
    let (session, provider) = held(&home.0, &home.0, sandbox, refuse);
    session.attach_store(store)?;
    let forged = |line: &str| {
        let ledger = std::fs::read_to_string(&file).unwrap_or_default();
        ledger.lines().any(|kept| kept == line)
    };
    let echo = format!("echo forged-by-bash >> '{}'", file.display());
    let ran = turn(&session, &provider, &[&echo]).await?;
    assert!(
        !forged("forged-by-bash"),
        "a contained bash call appended: {}",
        ran[0]
    );
    let mut args = Map::new();
    args.insert("path".to_owned(), json!(file));
    args.insert("content".to_owned(), json!("forged-by-write\n"));
    provider.queue_faux(vec![faux_assistant_message(
        vec![yi_ai::faux::faux_tool_call("forge-2", "write", args)],
        StopReason::ToolUse,
    )]);
    session.prompt("forge it")?;
    session.wait_idle().await;
    let ran = results(&session);
    assert!(
        !forged("forged-by-write"),
        "the write tool wrote it: {ran:?}"
    );
    Ok(())
}

/// Review of #938: a parent's grant reaching its child was untested; a copy of nothing passed.
#[tokio::test(flavor = "multi_thread")]
async fn a_parents_kept_grant_applies_in_its_child() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-kept-down")?;
    let dir = Probe::new(&probe, "down")?;
    let made = dir.0.join("made");
    let (asker, asks) = answering(AskOutcome::Reject);
    let (session, provider, _gate, host) = family_root(&root, &project, sandbox, asker)?;
    let store = support::memory_store("kept-down");
    let grant = yi_permission::write_grant(&real(&dir.0));
    ledger(&store, "command", &grant.canonical, &grant.label)?;
    session.attach_store(store)?;
    provider.queue_faux(vec![
        bash_call("child-1", &format!("touch {}", made.display())),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    host.spawn("use the grant".to_owned(), root_child(&[]))?;
    let finished = || Value::Object(host.status())["members"][0]["state"] == "finished";
    until(|| made.exists() || finished()).await;
    assert!(made.exists(), "the child runs under its parent's grant");
    assert_eq!(asks.load(Ordering::SeqCst), 0, "and asks nobody");
    Ok(())
}

/// Review of #938: a pass is bound to the directory it was kept in, and a child's broker kept
/// its parent's directory, so an isolated child in another worktree matched it there.
#[test]
fn a_child_elsewhere_does_not_inherit_the_pass() -> TestResult {
    let (root, project, sandbox, _probe) = workspace("yi-kept-elsewhere")?;
    let provider = Arc::new(ProviderStream::new(None));
    let session = new_session(&provider);
    let (refuse, _) = answering(AskOutcome::Reject);
    let gate = broker(&session, &project, sandbox, refuse);
    let pass =
        yi_permission::canonical_command_identity("python3 nest.py", &project.to_string_lossy());
    let rule: yi_types::permission::SessionPermissionRule = serde_json::from_value(json!({
        "id": 1, "kind": "command", "canonical": pass,
        "displayIdentity": "this exact command: python3 nest.py",
        "decision": "allow", "generation": 1
    }))?;
    gate.replay(vec![rule], &Wall::default());
    let here = gate.for_child(&Wall::default(), &project);
    let elsewhere = gate.for_child(&Wall::default(), &root.join("lane"));
    assert_eq!(here.kept_rules().len(), 1);
    assert_eq!(elsewhere.kept_rules(), Vec::<String>::new());
    Ok(())
}

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Status};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

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

#[tokio::test]
async fn prompt_runs_to_idle_with_events() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("hello from faux")],
        StopReason::Stop,
    )]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let mut events = session.subscribe();
    session.prompt("hi")?;
    assert!(session.prompt("again").is_err());
    session.wait_idle().await;
    assert_eq!(session.status(), Status::Idle);
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        kinds.push(match event {
            AgentEvent::AgentStart => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            _ => "other",
        });
    }
    assert_eq!(kinds.first(), Some(&"agent_start"));
    assert_eq!(kinds.last(), Some(&"agent_end"));
    assert_eq!(session.messages().len(), 2);
    session.prompt("second turn is admitted after idle")?;
    session.wait_idle().await;
    Ok(())
}

fn session_with_reply(text: &str) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text(text)],
        StopReason::Stop,
    )]);
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

#[tokio::test]
async fn executes_a_read_tool_call_through_the_adapter() -> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-tool")?;
    std::fs::write(dir.join("fact.txt"), "the answer is 42")?;

    let provider = Arc::new(ProviderStream::new(None));
    let mut call_args = serde_json::Map::new();
    call_args.insert("path".to_owned(), serde_json::json!("fact.txt"));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "read", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), None);
    let mut events = session.subscribe();
    session.prompt("read the fact")?;
    session.wait_idle().await;

    let mut tool_result_text = String::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            assert!(!is_error);
            for content in result.content {
                if let yi_types::message::Content::Text { text, .. } = content {
                    tool_result_text.push_str(&text);
                }
            }
        }
    }
    assert!(tool_result_text.contains("the answer is 42"));
    Ok(())
}

#[tokio::test]
async fn persists_a_turn_to_the_store_and_resumes_from_it() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-runtime-store")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-runtime-test");

    let store = repo.create(CreateOptions {
        id: Some("turn-one".to_owned()),
        ..CreateOptions::default()
    })?;
    let session = session_with_reply("persisted reply");
    assert_eq!(session.attach_store(Arc::clone(&store))?, 0);
    session.prompt("hi")?;
    session.wait_idle().await;
    assert_eq!(session.store_error(), None);
    drop(session);
    drop(store);

    let reopened = repo.open("turn-one")?;
    let resumed = session_with_reply("second reply");
    assert_eq!(resumed.attach_store(reopened)?, 2);
    resumed.prompt("again")?;
    resumed.wait_idle().await;
    assert_eq!(resumed.store_error(), None);
    assert_eq!(resumed.messages().len(), 4);

    let final_store = repo.open("turn-one")?;
    let entries = yi_session::lock_session(&final_store).find_entries(&yi_session::EntryQuery {
        order: yi_session::EntryOrder::OldestFirst,
        ..yi_session::EntryQuery::default()
    })?;
    assert_eq!(entries.len(), 4);
    Ok(())
}

fn tool_call_session(command: &str) -> AgentSession {
    let mut call_args = serde_json::Map::new();
    call_args.insert("command".to_owned(), serde_json::json!(command));
    one_call_session("bash", call_args)
}

fn one_call_session(
    tool: &str,
    call_args: serde_json::Map<String, serde_json::Value>,
) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", tool, call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

async fn run_gated(
    command: &str,
    mode: yi_runtime::PermissionMode,
) -> Result<(bool, String), Box<dyn Error>> {
    run_gated_session(tool_call_session(command), mode).await
}

async fn run_gated_session(
    mut session: AgentSession,
    mode: yi_runtime::PermissionMode,
) -> Result<(bool, String), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-perm")?;
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        mode,
        dir.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), Some(broker));
    let mut events = session.subscribe();
    session.prompt("run it")?;
    session.wait_idle().await;
    let mut outcome = (false, String::new());
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            let text: String = result
                .content
                .iter()
                .map(|content| match content {
                    yi_types::message::Content::Text { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect();
            outcome = (!is_error, text);
        }
    }
    Ok(outcome)
}

#[tokio::test]
async fn headless_ask_mode_denies_with_evidence() -> Result<(), Box<dyn Error>> {
    let (allowed, text) = run_gated("echo hello", yi_runtime::PermissionMode::Ask).await?;
    assert!(!allowed);
    assert!(text.contains("Permission denied"), "{text}");
    assert!(text.contains("no interactive surface"), "{text}");
    assert!(text.contains("echo hello"), "{text}");
    Ok(())
}

#[tokio::test]
async fn yolo_mode_runs_the_command() -> Result<(), Box<dyn Error>> {
    let (allowed, text) = run_gated("echo hello", yi_runtime::PermissionMode::Yolo).await?;
    assert!(allowed, "{text}");
    assert!(text.contains("hello"), "{text}");
    Ok(())
}

#[tokio::test]
async fn catastrophic_targets_are_denied_even_in_yolo() -> Result<(), Box<dyn Error>> {
    let (allowed, text) = run_gated("rm -rf ~/.ssh", yi_runtime::PermissionMode::Yolo).await?;
    assert!(!allowed);
    assert!(text.contains("protected path"), "{text}");
    assert!(text.contains("denied in every mode"), "{text}");
    Ok(())
}

/// Incident: a `read` of `~/.ssh/id_rsa` was judged as `<cwd>/~/.ssh/id_rsa`, passed, and
/// failed only because no such file exists. The name here is absent, so a regression prints
/// no key.
#[tokio::test]
async fn a_tilde_read_of_a_key_store_is_denied_even_in_yolo() -> Result<(), Box<dyn Error>> {
    let mut call_args = serde_json::Map::new();
    let absent = format!("~/.ssh/yi-absent-{}", std::process::id());
    call_args.insert("path".to_owned(), serde_json::json!(absent));
    let session = one_call_session("read", call_args);
    let (allowed, text) = run_gated_session(session, yi_runtime::PermissionMode::Yolo).await?;
    assert!(!allowed);
    assert!(text.contains("protected path"), "{text}");
    Ok(())
}

#[tokio::test]
async fn write_approval_carries_the_patch() -> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-diff")?;
    let target = dir.join("notes.txt");
    std::fs::write(&target, "alpha\nbravo\ncharlie\n")?;

    let provider = Arc::new(ProviderStream::new(None));
    let mut call_args = serde_json::Map::new();
    call_args.insert("path".to_owned(), serde_json::json!("notes.txt"));
    call_args.insert(
        "content".to_owned(),
        serde_json::json!("alpha\nBRAVO\ncharlie\n"),
    );
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "write", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Ask,
        dir.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), Some(broker));
    let mut events = session.subscribe();
    session.prompt("write it")?;
    session.wait_idle().await;

    let mut denial = String::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd { result, .. } = event {
            denial = result
                .content
                .iter()
                .map(|content| match content {
                    yi_types::message::Content::Text { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect();
        }
    }
    assert!(denial.contains("-bravo"), "{denial}");
    assert!(denial.contains("+BRAVO"), "{denial}");
    assert!(denial.contains(" alpha"), "{denial}");
    assert_eq!(std::fs::read_to_string(&target)?, "alpha\nbravo\ncharlie\n");
    Ok(())
}

/// The interrupt signal lives on the session and nothing ever cleared it, so
/// the first abort that landed left `fired` set — and every later turn aborted
/// at its first checkpoint. A session the user interrupted once was finished.
#[tokio::test]
async fn a_session_still_runs_turns_after_an_abort() -> Result<(), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );

    session.prompt("one")?;
    session.wait_idle().await;
    // The user interrupts after that turn settled, the way a stray Esc lands.
    session.abort();

    let mut events = session.subscribe();
    session.prompt("two")?;
    session.wait_idle().await;

    let mut reasons = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::MessageEnd {
            message: yi_types::message::AgentMessage::Assistant { stop_reason, .. },
        } = event
        {
            reasons.push(stop_reason);
        }
    }
    assert!(
        reasons.contains(&StopReason::Stop),
        "the turn after an abort must run normally, not inherit the stale \
         interrupt: {reasons:?}"
    );
    assert!(
        !reasons.contains(&StopReason::Aborted),
        "nothing was interrupted this turn: {reasons:?}"
    );
    Ok(())
}

/// Cancelling killed the `sh -c` process alone, so a grandchild it had spawned
/// kept the capture pipes open and the drain — with it, the whole turn — waited
/// out the command anyway. The abort was safe but never prompt: Esc during a
/// long build left the UI working until the build finished on its own.
#[tokio::test]
async fn an_abort_during_a_tool_call_kills_the_child() -> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-abort")?;
    let marker = dir.join("marker");
    let started = dir.join("started");
    // `sleep` is forked before `started` appears, so the grandchild holding the
    // capture pipes — the whole point of the test — is live when we interrupt.
    let mut session = tool_call_session("sleep 2 & echo up > started; wait; echo late > marker");
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Yolo,
        dir.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), Some(broker));
    session.prompt("run it")?;
    for _ in 0..200 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(started.exists(), "the shell never got as far as forking");
    let interrupted = std::time::Instant::now();
    session.abort();
    session.wait_idle().await;
    let settled = interrupted.elapsed();
    assert!(
        settled < std::time::Duration::from_secs(1),
        "an abort must end the turn, not wait out the command it interrupted: \
         {settled:?}"
    );
    // Outlive the sleep: a shell that survived would run its next command here.
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    assert!(
        !marker.exists(),
        "the interrupted shell must not run its next command"
    );
    Ok(())
}

/// `tool_call_session` wired the way `yi ask` is, `--deadline` included.
fn deadline_session(root: &std::path::Path, command: &str, total: Duration) -> AgentSession {
    let mut session = tool_call_session(command);
    attach_like_yi_ask(&mut session, root, Some(total));
    session
}

/// `session` wired the way `yi ask` is.
fn attach_like_yi_ask(
    session: &mut AgentSession,
    root: &std::path::Path,
    deadline: Option<Duration>,
) {
    let provider = Arc::clone(session.provider_arc());
    yi_runtime::attach_runtime(
        session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: "sys".to_owned(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: root.join("home"),
            lane_slots: 1,
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
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
}

fn scratch(name: &str) -> Result<Scratch, Box<dyn Error>> {
    Ok(Scratch::new(&format!("yi-runtime-{name}"))?)
}

/// Incident: `--deadline` only counted down in the environment block, so a command that
/// outlived the budget held its turn until harbor killed the container.
#[tokio::test]
async fn a_deadline_kills_a_running_bash_call() -> Result<(), Box<dyn Error>> {
    let root = scratch("deadline-kill")?;
    let session = deadline_session(&root, "sleep 30", Duration::from_secs(1));
    let started = std::time::Instant::now();
    session.prompt("run it")?;
    let settled = tokio::time::timeout(Duration::from_secs(5), session.wait_idle()).await;
    assert!(
        settled.is_ok(),
        "a one-second deadline must end the command, not wait it out: {:?}",
        started.elapsed()
    );
    Ok(())
}

/// The deadline ends the run between turns, never inside one: the call in flight runs to its
/// own end and no work turn follows. Dies too with a run that ends on that tool call with no
/// answer (`mbx-service`, `mbx-ask`): one last turn answers, and a tool it calls is not run.
#[tokio::test]
async fn a_deadline_ends_the_run_after_the_turn_in_flight() -> Result<(), Box<dyn Error>> {
    let root = scratch("deadline-stop")?;
    // Invariant: the call ends between the 4.5 s margin and the 4.875 s last word of a 6 s clock,
    // so it sleeps to an instant: a span also counted the work before it, and git init took 0.3 s.
    #[expect(
        clippy::disallowed_methods,
        reason = "the command sleeps to a wall-clock instant, so the fixture reads that clock"
    )]
    let ends = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs_f64()
        + 4.7;
    let command =
        format!("python3 -c \"import time; time.sleep(max(0, {ends:.3} - time.time()))\"");
    let session = deadline_session(&root, &command, Duration::from_secs(6));
    let mut events = session.subscribe();
    session.prompt("run it")?;
    session.wait_idle().await;
    let mut calls = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd { is_error, .. } = event {
            calls.push(is_error);
        }
    }
    assert_eq!(calls, [false], "the call in flight runs to its own end");
    let unsent = session
        .provider()
        .faux
        .lock()
        .map(|faux| faux.pending_response_count())
        .unwrap_or_default();
    assert_eq!(
        unsent, 0,
        "the last word is the one request inside the margin"
    );
    let messages = session.messages();
    let asked = serde_json::to_string(&messages)?;
    assert!(
        asked.contains("[deadline] Time is up: no more tool calls"),
        "{asked}"
    );
    let last = messages.iter().rev().find_map(|message| match message {
        AgentMessage::Assistant { content, .. } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(serde_json::to_value(last)?[0]["text"], "done", "{asked}");
    Ok(())
}

/// Dies with a follow-up only queued: an ACP or RPC follow-up that reached an idle session
/// waited for a turn nobody started.
#[tokio::test]
async fn a_follow_up_to_an_idle_session_starts_its_turn() -> Result<(), Box<dyn Error>> {
    let session = session_with_reply("taken");
    session.follow_up_message(yi_runtime::session::user_input("one more thing"));
    tokio::time::timeout(Duration::from_secs(5), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    assert!(said.contains("taken"), "{said}");
    Ok(())
}

type Log = Arc<std::sync::Mutex<Vec<&'static str>>>;

fn note(log: &Log, what: &'static str) {
    if let Ok(mut log) = log.lock() {
        log.push(what);
    }
}

/// A session whose start capture takes 300 ms and then writes `captured` into `fact.txt`;
/// the log records each request, the capture's end and the end capture.
fn capture_session(dir: &Scratch, replies: Vec<AgentMessage>) -> (AgentSession, Log) {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(replies);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), None);
    let log: Log = Arc::default();
    let (start, request, end) = (Arc::clone(&log), Arc::clone(&log), Arc::clone(&log));
    let fact = dir.join("fact.txt");
    session.set_turn_start_hook(Arc::new(move || {
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::fs::write(&fact, "captured");
        note(&start, "captured");
    }));
    session.set_environment(Arc::new(move || {
        note(&request, "request");
        None
    }));
    session.set_turn_end_hook(Arc::new(move || note(&end, "end")));
    (session, log)
}

async fn settled_log(session: &AgentSession, log: &Log, entries: usize) -> Vec<&'static str> {
    session.wait_idle().await;
    for _ in 0..100 {
        if log.lock().map(|log| log.len()).unwrap_or(0) >= entries {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    log.lock().map(|log| log.clone()).unwrap_or_default()
}

/// Incident: the turn-start snapshot (1.2 s on a large tree's first capture) ran before the
/// first request. It now runs beside it, and no tool runs before it ends.
#[tokio::test]
async fn the_start_capture_overlaps_the_request_and_gates_the_tools() -> Result<(), Box<dyn Error>>
{
    let dir = Scratch::new("yi-runtime-capture")?;
    std::fs::write(dir.join("fact.txt"), "before")?;
    let mut call_args = serde_json::Map::new();
    call_args.insert("path".to_owned(), serde_json::json!("fact.txt"));
    let (session, log) = capture_session(
        &dir,
        vec![
            faux_assistant_message(
                vec![faux_tool_call("call-1", "read", call_args)],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ],
    );
    let mut events = session.subscribe();
    session.prompt("read the fact")?;
    let seen = settled_log(&session, &log, 4).await;
    assert_eq!(seen, ["request", "captured", "request", "end"]);
    let mut read = String::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd { result, .. } = event {
            read.push_str(&serde_json::to_string(&result.content)?);
        }
    }
    assert!(
        read.contains("captured"),
        "the tool overtook the capture: {read}"
    );
    Ok(())
}

/// A first open of the shadow gitdir is a `git init` (40-200 ms) that sat on the start of
/// every first session in a project; the first capture, beside the turn, opens it now.
#[tokio::test]
async fn wiring_checkpoints_leaves_the_shadow_gitdir_to_the_first_capture()
-> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-shadow")?;
    let home = dir.join("home");
    let session = session_with_reply("x");
    yi_runtime::wire_turn_checkpoints(&session, &home, &dir.join("project"));
    assert!(!yi_runtime::checkpoint::checkpoint_root(&home).exists());
    Ok(())
}

/// A reply with no tool call ends the run before the start capture does; the end capture
/// still lands after it, or undo pairs the wrong trees.
#[tokio::test]
async fn the_end_capture_follows_a_slow_start_capture() -> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-capture-end")?;
    let reply = faux_assistant_message(vec![faux_text("quick")], StopReason::Stop);
    let (session, log) = capture_session(&dir, vec![reply]);
    let mut events = session.subscribe();
    session.prompt("answer")?;
    // Incident: a surface reads `AgentEnd` as idle; a capture still running then refused its
    // next prompt as busy, and the TUI drive waited for a turn that never started.
    while !matches!(events.recv().await?, AgentEvent::AgentEnd { .. }) {}
    let at_end = log.lock().map(|log| log.clone()).unwrap_or_default();
    assert!(at_end.contains(&"captured"), "{at_end:?}");
    let seen = settled_log(&session, &log, 3).await;
    assert_eq!(seen, ["request", "captured", "end"]);
    Ok(())
}

/// Every settled ask of a session run with `asker`, read back from its journal.
async fn journaled(
    root: &std::path::Path,
    asker: Option<yi_runtime::Asker>,
) -> Result<Vec<yi_types::permission::PermissionRecord>, Box<dyn Error>> {
    let mut repo = JsonlRepo::new(root.join("sessions"), "/tmp/yi-perm-journal");
    let store = repo.create(CreateOptions::default())?;
    let mut session = tool_call_session("echo journaled");
    session.attach_store(Arc::clone(&store))?;
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Ask,
        root.to_path_buf(),
        Vec::new(),
        asker,
        session.events_sender(),
    ));
    let provider = Arc::clone(session.provider_arc());
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: "sys".to_owned(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: root.join("home"),
            lane_slots: 1,
            broker: Some(broker),
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
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    session.prompt("run it")?;
    session.wait_idle().await;
    let entries = yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery {
        order: yi_session::EntryOrder::OldestFirst,
        ..yi_session::EntryQuery::default()
    })?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| match entry {
            yi_types::entry::Entry::Custom {
                custom_type, data, ..
            } if custom_type == yi_types::permission::PERMISSION_ENTRY => data,
            _ => None,
        })
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?)
}

/// Approvals are the labels a classifier fits its thresholds on, so each settled ask is journaled
/// in the session, not only broadcast. Dies with the record missing, or naming the wrong answerer.
#[tokio::test]
async fn a_settled_ask_is_journaled_in_the_session() -> Result<(), Box<dyn Error>> {
    use yi_types::permission::Answerer;
    let approve: yi_runtime::Asker = Arc::new(|_| yi_runtime::AskOutcome::AllowOnce);
    for (asker, want) in [
        (None, (false, Answerer::Nobody)),
        (Some(approve), (true, Answerer::User)),
    ] {
        let root = scratch("perm-journal")?;
        let records = journaled(&root, asker).await?;
        let [record] = records.as_slice() else {
            return Err(format!("one settled ask, one record: {records:?}").into());
        };
        assert_eq!((record.allowed, record.by.clone()), want, "{record:?}");
        assert!(record.description.contains("echo journaled"), "{record:?}");
    }
    Ok(())
}

/// The owner: approval "should be on by default". A session attached with a classifier, armed by
/// default or by the `approve` key that armed it before, mints the key under the session's HOME.
/// Headless, with nobody wired to answer, an auto-mode ask the classifier is sure of still fails
/// closed. Dies with an unattended classifier allow.
#[tokio::test]
async fn a_headless_session_with_no_asker_fails_closed() -> Result<(), Box<dyn Error>> {
    for (name, extra) in [
        ("approve-default", ""),
        ("approve-armed", r#", "approve": true"#),
    ] {
        let (port, _served) =
            crate::classifier_e2e::sidecar(vec![crate::classifier_e2e::safe(0.99)])?;
        let (broker, warnings) = brokered_with_classifier(name, None, false, extra, port)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut args = serde_json::Map::new();
        args.insert("command".to_owned(), serde_json::json!("make build"));
        let outcome =
            broker.decide_call("bash", yi_tools::ToolKind::Exec, false, "c1", &args, None);
        assert!(
            !outcome.allowed,
            "{name}: headless, nobody answers for the person, so the ask degrades to a denial: {}",
            outcome.reason
        );
    }
    Ok(())
}

/// The owner: approval "should be on by default". A config with no `approval` key arms
/// the classifier through attach, so an auto-mode ask the classifier is sure of is allowed
/// where the person asked would have said no. Dies with an unattended classifier allow.
#[tokio::test]
async fn a_classifier_with_no_mode_set_approves_by_default() -> Result<(), Box<dyn Error>> {
    let (port, _served) = crate::classifier_e2e::sidecar(vec![crate::classifier_e2e::safe(0.99)])?;
    let asker: yi_runtime::Asker = Arc::new(|_| yi_runtime::AskOutcome::Reject);
    let (broker, warnings) =
        brokered_with_classifier("approve-default-on", Some(asker), false, "", port)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut args = serde_json::Map::new();
    args.insert("command".to_owned(), serde_json::json!("make build"));
    let outcome = broker.decide_call("bash", yi_tools::ToolKind::Exec, false, "c1", &args, None);
    assert!(outcome.allowed, "default-on: {}", outcome.reason);
    assert!(outcome.reason.contains("classifier"), "{}", outcome.reason);
    Ok(())
}

/// `after-delay` answers only where prompts close on settle; every other surface gets
/// wait-for-user, and attach must say so at start, never silently.
#[tokio::test]
async fn after_delay_warns_where_prompts_never_close() -> Result<(), Box<dyn Error>> {
    let (port, _served) = crate::classifier_e2e::sidecar(vec![crate::classifier_e2e::safe(0.99)])?;
    let extra = r#", "approval": "after-delay""#;
    let asker: yi_runtime::Asker = Arc::new(|_| yi_runtime::AskOutcome::Reject);
    let (_, warnings) =
        brokered_with_classifier("after-delay-warn", Some(asker), false, extra, port)?;
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("never close on settle")),
        "{warnings:?}"
    );
    let asker: yi_runtime::Asker = Arc::new(|_| yi_runtime::AskOutcome::Reject);
    let (_, warnings) =
        brokered_with_classifier("after-delay-close", Some(asker), true, extra, port)?;
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.contains("never close on settle")),
        "{warnings:?}"
    );
    Ok(())
}

/// The session shape every approval surface builds before `classifier::attach` runs: an
/// auto-mode broker behind the runtime wiring, then a classifier config parsed from `raw`.
fn brokered_with_classifier(
    name: &str,
    asker: Option<yi_runtime::Asker>,
    close_on_settle: bool,
    extra: &str,
    port: u16,
) -> Result<(Arc<yi_runtime::PermissionBroker>, Vec<String>), Box<dyn Error>> {
    let root = scratch(name)?;
    let home = root.join("home");
    std::fs::create_dir_all(&home)?;
    let mut session = tool_call_session("echo unused");
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        root.to_path_buf(),
        Vec::new(),
        asker,
        session.events_sender(),
    ));
    if close_on_settle {
        broker.prompts_close_on_settle();
    }
    let provider = Arc::clone(session.provider_arc());
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: "sys".to_owned(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: home.clone(),
            lane_slots: 1,
            broker: Some(Arc::clone(&broker)),
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
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    let raw = format!(
        r#"{{"models": {{"classifier": "english"}}, "classifier": {{"url": "http://127.0.0.1:{port}"{extra}}}}}"#
    );
    let (config, _) = yi_types::config::parse(&raw)?;
    let warnings = yi_runtime::classifier::attach(&session, &root, &home, &config);
    Ok((broker, warnings))
}

/// What the model was sent, in order: user text, reminder text, or `assistant`.
fn transcript(session: &AgentSession) -> Vec<String> {
    use yi_types::message::UserContent;
    session
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::User {
                content: UserContent::Text(text),
                ..
            } => Some(format!("user: {text}")),
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "reminder" => Some(format!("reminder: {text}")),
            AgentMessage::Assistant { .. } => Some("assistant".to_owned()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_typed_message_s_skill_pointer_enters_right_behind_it() -> Result<(), Box<dyn Error>> {
    use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope};
    use yi_runtime::session::user_input;
    let skill = |name: &str, needle: &str| RuleDoc {
        name: name.to_owned(),
        body: format!("skill://{name}"),
        path: std::path::PathBuf::from(format!("/skills/{name}/SKILL.md")),
        needles: vec![needle.to_owned()],
        scope: RuleScope::Text,
        gap: RuleGap::AfterTurns(1),
        mode: RuleMode::Remind,
        paths: Vec::new(),
        after: 1,
    };
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(
        ["one", "two", "three"]
            .into_iter()
            .map(|text| faux_assistant_message(vec![faux_text(text)], StopReason::Stop))
            .collect(),
    );
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.set_rules_engine(Arc::new(RuleEngine::new(vec![
        skill("gate", "cargo nextest"),
        skill("review", "review the work"),
        skill("plan", "write a plan"),
    ])));
    session.prompt_message(user_input("run cargo nextest"))?;
    assert!(
        session.follow_up_message(user_input("then review the work")),
        "the run is going, so the follow-up waits for its answer"
    );
    session.wait_idle().await;
    // An idle session's follow-up starts the run itself.
    session.follow_up_message(user_input("write a plan"));
    session.wait_idle().await;
    assert_eq!(
        transcript(&session),
        [
            "user: run cargo nextest",
            "reminder: Relevant: skill://gate (matched \"cargo nextest\")",
            "assistant",
            "user: then review the work",
            "reminder: Relevant: skill://review (matched \"review the work\")",
            "assistant",
            "user: write a plan",
            "reminder: Relevant: skill://plan (matched \"write a plan\")",
            "assistant",
        ]
    );
    Ok(())
}

/// D310: the system prompt is constant for a conversation. An orchestrate signal on the third
/// turn leaves the bytes the first request sent, and the protocol rides the transcript once, as
/// a `fragment` message between the prompt that raised it and the reply.
/// The transcript's kinds in order; a custom entry shows its kind and its first clause.
fn shape(session: &AgentSession) -> Vec<String> {
    use yi_types::message::UserContent;
    session
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::User { .. } => Some("user".to_owned()),
            AgentMessage::Assistant { .. } => Some("assistant".to_owned()),
            AgentMessage::CompactionSummary { .. } => Some("summary".to_owned()),
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } => Some(format!(
                "{custom_type}: {}",
                text.split(['.', '\n']).next().unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_orchestrate_signal_on_turn_three_leaves_the_system_prompt_alone()
-> Result<(), Box<dyn Error>> {
    use yi_types::message::UserContent;
    let dir = Scratch::new("yi-faux-constant-prompt")?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(
        (1..=3)
            .map(|n| {
                faux_assistant_message(vec![faux_text(&format!("reply {n}"))], StopReason::Stop)
            })
            .collect(),
    );
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.install_extensions(yi_runtime::ext::install(yi_runtime::ext::ExtOptions {
        cwd: dir.to_path_buf(),
        home: dir.to_path_buf(),
        mode: yi_runtime::PermissionMode::Auto,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
    }));
    session.prompt("hi")?;
    session.wait_idle().await;
    let first = session.system_prompt();
    assert!(!first.contains("# Orchestrate"), "a greeting is no program");
    session.prompt("thanks")?;
    session.wait_idle().await;
    session.prompt("plan this: split the crate in two")?;
    session.wait_idle().await;
    assert_eq!(
        session.system_prompt(),
        first,
        "the third turn must send the first request's system bytes"
    );
    assert_eq!(
        shape(&session),
        [
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "fragment: # Orchestrate",
            "assistant",
        ],
        "the protocol rides once, ahead of the reply it steers"
    );
    let keys: Vec<_> = session
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::Assistant { diagnostics, .. } => diagnostics?
                .into_iter()
                .find(|note| note.diagnostic_type == "cache")?
                .details?
                .remove("stable"),
            _ => None,
        })
        .collect();
    assert!(
        keys.len() == 3 && keys.iter().all(|key| *key == keys[0]),
        "every request records one stable key: {keys:?}"
    );
    let sent = yi_context::convert_to_llm(&session.messages());
    let AgentMessage::User {
        content: UserContent::Text(text),
        ..
    } = &sent[5]
    else {
        return Err(format!(
            "the fragment must reach the model as a user message: {:?}",
            sent[5]
        )
        .into());
    };
    assert!(
        text.starts_with("<yi_internal_context source=\"fragment\">\n# Orchestrate"),
        "{text}"
    );
    Ok(())
}

/// D310: a permission-mode flip through the broker rides once as the current mode's fragment,
/// and a compaction, which drops every internal message, is followed by every late slot's
/// current text on the next request: the mode, then the orchestrate protocol.
#[tokio::test]
async fn a_mode_flip_and_the_protocol_survive_a_compaction() -> Result<(), Box<dyn Error>> {
    use yi_context::{Settings, Tokens};
    let dir = Scratch::new("yi-faux-late-compaction")?;
    let provider = Arc::new(ProviderStream::new(None));
    let reply = |text: &str| faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
    // Turns 1 and 2 weigh about 2k tokens each, so a 3k retained tail keeps turn 3 (its
    // 8 KB protocol fragment included) and summarizes the rest.
    provider.queue_faux(vec![
        reply(&format!("reply 1 {}", "y".repeat(8_000))),
        reply(&format!("reply 2 {}", "y".repeat(8_000))),
        reply("reply 3"),
        reply("## Goal\nThe summary"),
        reply("reply 4"),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let broker = Arc::new(yi_runtime::permission::PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        dir.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    ));
    session.use_tools(Vec::new(), dir.to_path_buf(), Some(Arc::clone(&broker)));
    session.enable_compaction_with(Settings {
        enabled: true,
        reserve_tokens: Tokens(1_000),
        keep_recent_tokens: Tokens(3_000),
    });
    session.install_extensions(yi_runtime::ext::install(yi_runtime::ext::ExtOptions {
        cwd: dir.to_path_buf(),
        home: dir.to_path_buf(),
        mode: yi_runtime::PermissionMode::Auto,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
    }));
    session.prompt("hi")?;
    session.wait_idle().await;
    let first = session.system_prompt();
    broker.set_mode_and_fragment(yi_runtime::PermissionMode::Ask, &session);
    session.prompt("thanks")?;
    session.wait_idle().await;
    assert_eq!(
        shape(&session)[2..],
        ["user", "fragment: Permission mode: ask", "assistant"],
        "the broker's flip rides once, as the current mode"
    );
    assert_eq!(session.system_prompt(), first);
    session.prompt("plan this: split the crate in two")?;
    session.wait_idle().await;
    assert!(
        session
            .compact_now()
            .await
            .is_ok_and(|outcome| outcome.applied()),
        "a scheduled compaction applies at once when idle"
    );
    let compacted = shape(&session);
    assert_eq!(
        compacted.first().map(String::as_str),
        Some("summary"),
        "{compacted:?}"
    );
    assert!(
        compacted.iter().all(|kind| !kind.starts_with("fragment")),
        "a compaction drops every internal message: {compacted:?}"
    );
    session.prompt("go on")?;
    session.wait_idle().await;
    let after = shape(&session);
    let last_user = after
        .iter()
        .rposition(|kind| kind == "user")
        .ok_or("no user")?;
    assert_eq!(
        after[last_user..],
        [
            "user",
            "fragment: Permission mode: ask",
            "fragment: # Orchestrate",
            "assistant",
        ],
        "the next request carries every late slot's current text: {after:?}"
    );
    assert_eq!(session.system_prompt(), first);
    Ok(())
}

/// A session wired like `yi ask` whose faux model makes `calls` (bash arguments) in order and
/// then says each of `replies`. Each test's scratch root is its own cwd, so another session's
/// completion loop in the same process never takes its jobs.
fn bash_session(
    root: &Scratch,
    calls: &[serde_json::Value],
    replies: &[&str],
    deadline: Option<Duration>,
) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None));
    let calls = calls.iter().enumerate().map(|(n, args)| {
        let args = args.as_object().cloned().unwrap_or_default();
        let call = faux_tool_call(&format!("call-{n}"), "bash", args);
        faux_assistant_message(vec![call], StopReason::ToolUse)
    });
    let replies = replies
        .iter()
        .map(|text| faux_assistant_message(vec![faux_text(text)], StopReason::Stop));
    provider.queue_faux(calls.chain(replies).collect());
    let config = SessionConfig {
        system_prompt: "sys".to_owned(),
        model: faux_model(),
        thinking_level: None,
        tool_execution: ExecutionMode::Sequential,
    };
    let mut session = AgentSession::new(config, provider);
    attach_like_yi_ask(&mut session, root, deadline);
    session
}

/// The job a transcript's first backgrounded call became, by the id its result names, since the
/// registry is shared by every test in a `cargo test` process.
fn job_in(said: &str) -> Result<yi_tools::jobs::JobId, Box<dyn Error>> {
    let id = said
        .split("now job ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| format!("no job in {said}"))?;
    Ok(yi_tools::jobs::JobId(id))
}

/// Issue #758: a command still running at its `wait` hands the turn back as a job, and a job that
/// exits while that turn still runs reaches the model as `<async_result>` before the run ends.
#[tokio::test]
async fn a_job_that_exits_mid_turn_reports_into_that_turn() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-report")?;
    let calls = [
        serde_json::json!({"command": "sleep 6; echo w$((1+1))ke", "wait": 5}),
        serde_json::json!({"command": "sleep 4"}),
    ];
    let session = bash_session(&root, &calls, &["done", "seen"], None);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    assert!(said.contains("now job"), "not backgrounded: {said}");
    let report = said
        .split("<async_result")
        .nth(1)
        .ok_or_else(|| said.clone())?;
    assert!(report.contains("w2ke") && report.contains("seen"), "{said}");
    Ok(())
}

/// Waits for the transcript to hold `needle`, or names what it held instead.
async fn said_eventually(session: &AgentSession, needle: &str) -> Result<String, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let said = serde_json::to_string(&session.messages())?;
        if said.contains(needle) {
            return Ok(said);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no {needle:?} in {said}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Issue #820: a job that exits after its turn ended starts a turn of its own, carrying its
/// `<async_result>`, instead of waiting for the user to type.
#[tokio::test]
async fn a_job_that_exits_after_the_turn_wakes_the_idle_session() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-idle")?;
    let calls = [serde_json::json!({"command": "sleep 6; echo w$((1+1))ke", "wait": 5})];
    let session = bash_session(&root, &calls, &["done", "woken"], None);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = said_eventually(&session, "woken").await?;
    let (report, woken) = (said.find("<async_result"), said.find("woken"));
    assert!(report.is_some() && report < woken, "{said}");
    assert!(said.contains("w2ke"), "{said}");
    Ok(())
}

/// Issue #820: two sessions in one cwd, the first of them idle; only the session that started the
/// job hears it. Keyed by cwd, the earliest-attached loop took every result in that directory.
#[tokio::test]
async fn a_job_reports_only_to_the_session_that_started_it() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-owner")?;
    let bystander = bash_session(&root, &[], &["bystander woke"], None);
    let calls = [serde_json::json!({"command": "sleep 6; echo own$((1+1))er", "wait": 5})];
    let owner = bash_session(&root, &calls, &["done", "owner woke"], None);
    owner.prompt("run it")?;
    let said = said_eventually(&owner, "owner woke").await?;
    assert!(said.contains("own2er"), "{said}");
    let heard = serde_json::to_string(&bystander.messages())?;
    assert!(!heard.contains("own2er"), "{heard}");
    assert_eq!(bystander.status(), Status::Idle);
    Ok(())
}

/// A retired session (a reaped child) starts no turn when its job exits: the wake would be a
/// paid turn in a session nobody reads.
#[tokio::test]
async fn a_retired_session_s_job_starts_no_turn() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-retired")?;
    let calls = [serde_json::json!({"command": "sleep 6; echo r$((1+1))tired", "wait": 5})];
    let session = bash_session(&root, &calls, &["done", "should not run"], None);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let job = job_in(&serde_json::to_string(&session.messages())?)?;
    session.retire();
    let jobs = yi_tools::jobs::registry();
    tokio::task::spawn_blocking(move || jobs.wait_settled(Some(job), Duration::from_secs(20)))
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let said = serde_json::to_string(&session.messages())?;
    assert!(!said.contains("should not run"), "{said}");
    assert_eq!(session.status(), Status::Idle);
    Ok(())
}

/// Dies without the delivered bit: a job the completion loop queued while a turn ran, and the
/// model then polled to finished, reached the model a second time as an `<async_result>`.
#[tokio::test]
async fn a_polled_job_queued_before_the_poll_is_not_announced() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-polled")?;
    let session = bash_session(
        &root,
        &[serde_json::json!({"command": "sleep 6"})],
        &["done", "again"],
        None,
    );
    let mut context = yi_tools::ToolContext::new(root.to_path_buf());
    context.job_owner = Some(session.job_owner());
    let bash = |input: serde_json::Value, context: &yi_tools::ToolContext| {
        let ran = yi_tools::Tool::execute(
            &yi_tools::BashTool::default(),
            input.as_object().cloned().unwrap_or_default(),
            context,
        );
        serde_json::to_string(&ran.result)
    };
    let started = bash(
        serde_json::json!({"command": "sleep 7; echo w$((1+1))ke", "wait": 5}),
        &context,
    )?;
    let job = job_in(&started)?;
    session.prompt("run it")?;
    let jobs = yi_tools::jobs::registry();
    tokio::task::spawn_blocking(move || jobs.wait_settled(Some(job), Duration::from_secs(20)))
        .await?;
    assert_eq!(
        session.status(),
        Status::Running,
        "the poll must land while the turn runs"
    );
    let polled = bash(serde_json::json!({ "job": job.0 }), &context)?;
    assert!(
        polled.contains("finished (exit 0)") && polled.contains("w2ke"),
        "{polled}"
    );
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    assert!(!said.contains("<async_result"), "{said}");
    Ok(())
}

/// A job the watchdog killed says so in its `<async_result>`, not a bare exit code.
#[tokio::test]
async fn a_killed_job_reports_that_timeout_secs_killed_it() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-killed")?;
    let calls = [
        serde_json::json!({"command": "echo started; sleep 30", "wait": 5, "timeout_secs": 7}),
        serde_json::json!({"command": "sleep 5"}),
    ];
    let session = bash_session(&root, &calls, &["done", "seen"], None);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    let report = said
        .split("<async_result")
        .nth(1)
        .ok_or_else(|| said.clone())?;
    assert!(
        report.contains("killed (timeout_secs or an interrupt): echo started; sleep 30"),
        "{said}"
    );
    Ok(())
}

/// `--deadline` ends a job the turn handed back, as it ends a command in the turn: once yi exits
/// nothing else would, since the watchdog is a thread of yi (#819).
#[tokio::test]
async fn a_deadline_kills_a_backgrounded_job() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-deadline")?;
    let calls = [
        serde_json::json!({"command": "sleep 40", "wait": 5}),
        serde_json::json!({"command": "sleep 30"}),
    ];
    let deadline = Some(Duration::from_secs(12));
    let session = bash_session(&root, &calls, &["done", "late"], deadline);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    let job = job_in(&said)?;
    let jobs = yi_tools::jobs::registry();
    tokio::task::spawn_blocking(move || jobs.wait_settled(Some(job), Duration::from_secs(5)))
        .await?;
    let state = jobs.report(job).map(|report| report.state);
    let killed = yi_tools::jobs::JobState::Settled(yi_tools::jobs::Outcome::Killed);
    assert_eq!(state, Some(killed), "{said}");
    Ok(())
}

/// Dies with the wait that reads the status before it listens: `settle` fires `notify_waiters`,
/// which stores no permit, so an idle landing between the two was lost and the wait never woke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_wait_idle_wakes_when_its_run_settles() -> Result<(), Box<dyn Error>> {
    const RUNS: usize = 2_000;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(
        (0..RUNS)
            .map(|_| faux_assistant_message(vec![faux_text("ok")], StopReason::Stop))
            .collect(),
    );
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    for run in 0..RUNS {
        session.prompt("go")?;
        tokio::time::timeout(Duration::from_secs(2), session.wait_idle())
            .await
            .map_err(|_| format!("run {run}: wait_idle never woke after the run settled"))?;
    }
    Ok(())
}

/// A reader that falls behind the broadcast is told how many events it lost, as a gap every
/// stream writes, instead of reading on as if nothing were missing.
#[tokio::test]
async fn a_lagging_reader_gets_the_gap_then_the_next_event() -> Result<(), Box<dyn Error>> {
    let (events, mut reader) = tokio::sync::broadcast::channel(1);
    for _ in 0..3 {
        events.send(yi_types::event::AgentEvent::TurnStart)?;
    }
    let gap = yi_runtime::next_event(&mut reader).await;
    assert_eq!(gap, Some(Err(yi_types::event::EventGap { dropped: 2 })));
    let next = yi_runtime::next_event(&mut reader).await;
    assert_eq!(next, Some(Ok(yi_types::event::AgentEvent::TurnStart)));
    drop(events);
    assert_eq!(yi_runtime::next_event(&mut reader).await, None);
    Ok(())
}

/// Two steers sent while one tool batch runs are read together when it ends: both sit in the
/// context ahead of the next reply, in arrival order, and nothing is left queued behind them.
#[tokio::test]
async fn steers_sent_during_a_tool_batch_arrive_together_in_the_next_request()
-> Result<(), Box<dyn Error>> {
    let dir = Scratch::new("yi-runtime-steer-batch")?;
    let started = dir.join("started");
    let mut session = tool_call_session("echo up > started; sleep 1");
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        yi_runtime::PermissionMode::Yolo,
        dir.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), Some(broker));
    session.prompt("run it")?;
    for _ in 0..500 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(started.exists(), "the tool never started");
    for text in ["fast forward main", "then run the tests"] {
        session.steer_message(yi_runtime::session::user_input(text));
    }
    session.wait_idle().await;
    assert_eq!(
        transcript(&session),
        [
            "user: run it",
            "assistant",
            "user: fast forward main",
            "user: then run the tests",
            "assistant"
        ]
    );
    Ok(())
}

/// Dies with an empty optional argument read as a value: glm-5.3-flash sent `read` with
/// `"find": ""` and `"pages": ""` it did not mean, refused as a PDF page range on a source file.
/// A required empty value still reaches the tool, so `write` with `""` makes an empty file.
#[tokio::test]
async fn an_empty_optional_argument_is_not_sent_and_a_required_one_is() -> Result<(), Box<dyn Error>>
{
    let dir = Scratch::new("yi-runtime-empties")?;
    std::fs::write(dir.join("calc.py"), "def add(a, b):\n    return a + b\n")?;
    let provider = Arc::new(ProviderStream::new(None));
    let read = serde_json::json!({"path": "calc.py", "find": "", "pages": ""});
    let write = serde_json::json!({"path": "empty.txt", "content": ""});
    let arguments = |value: serde_json::Value| value.as_object().cloned().unwrap_or_default();
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "read", arguments(read))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(
            vec![faux_tool_call("call-2", "write", arguments(write))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.use_tools(yi_tools::builtin_tools(), dir.to_path_buf(), None);
    let mut events = session.subscribe();
    session.prompt("read calc.py and write an empty file")?;
    session.wait_idle().await;
    let mut ended = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } = event
        {
            ended.push((tool_name, is_error, serde_json::to_string(&result.content)?));
        }
    }
    assert_eq!(ended.len(), 2, "{ended:?}");
    for (tool, is_error, text) in &ended {
        assert!(!is_error, "{tool}: {text}");
    }
    assert!(
        ended
            .iter()
            .any(|(tool, _, text)| tool == "read" && text.contains("return a + b"))
    );
    assert_eq!(std::fs::read(dir.join("empty.txt"))?.len(), 0);
    Ok(())
}

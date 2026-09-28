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
    let provider = Arc::new(ProviderStream::new(None, None));
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
    let provider = Arc::new(ProviderStream::new(None, None));
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

    let provider = Arc::new(ProviderStream::new(None, None));
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
    let provider = Arc::new(ProviderStream::new(None, None));
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

    let provider = Arc::new(ProviderStream::new(None, None));
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
    let provider = Arc::new(ProviderStream::new(None, None));
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
    let provider = Arc::new(ProviderStream::new(None, None));
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
    let provider = Arc::new(ProviderStream::new(None, None));
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

/// A session wired like `yi ask` whose faux model makes `calls` (bash arguments) in order and
/// then says each of `replies`.
fn bash_session(root: &Scratch, calls: &[serde_json::Value], replies: &[&str]) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None, None));
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
    attach_like_yi_ask(&mut session, root, None);
    session
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
    let session = bash_session(&root, &calls, &["done", "seen"]);
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

/// What the bash text now says of an idle session: a job that exits after the turn ended is
/// heard only once a later turn ends. #820 wakes the session instead; this pins today's truth.
#[tokio::test]
async fn a_job_that_exits_after_the_turn_waits_for_the_next_one() -> Result<(), Box<dyn Error>> {
    let root = scratch("job-idle")?;
    let calls = [serde_json::json!({"command": "sleep 6; echo w$((1+1))ke", "wait": 5})];
    let session = bash_session(&root, &calls, &["done", "seen", "after"]);
    session.prompt("run it")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let jobs = yi_tools::jobs::registry();
    tokio::task::spawn_blocking(|| jobs.wait_settled(None, Duration::from_secs(20))).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let idle = serde_json::to_string(&session.messages())?;
    assert!(!idle.contains("<async_result"), "{idle}");
    assert_eq!(session.status(), Status::Idle);
    session.prompt("next")?;
    tokio::time::timeout(Duration::from_secs(30), session.wait_idle()).await?;
    let said = serde_json::to_string(&session.messages())?;
    let (seen, report) = (said.find("seen"), said.find("<async_result"));
    assert!(seen.is_some() && seen < report, "{said}");
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
    let session = bash_session(&root, &calls, &["done", "seen"]);
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

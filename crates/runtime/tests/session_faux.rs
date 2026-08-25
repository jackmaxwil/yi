use std::error::Error;
use std::sync::Arc;
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Status};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::event::AgentEvent;
use yi_types::message::StopReason;
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
    let dir = std::env::temp_dir().join(format!("yi-runtime-tool-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
    session.use_tools(yi_tools::builtin_tools(), dir.clone(), None);
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
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[tokio::test]
async fn persists_a_turn_to_the_store_and_resumes_from_it() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("yi-runtime-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut repo = JsonlRepo::new(root.clone(), "/tmp/yi-runtime-test");

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
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

fn tool_call_session(command: &str) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call_args = serde_json::Map::new();
    call_args.insert("command".to_owned(), serde_json::json!(command));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "bash", call_args)],
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
    static PERM_DIR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = PERM_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("yi-runtime-perm-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut session = tool_call_session(command);
    let broker = Arc::new(yi_runtime::PermissionBroker::new(
        mode,
        dir.clone(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.clone(), Some(broker));
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
    let _ = std::fs::remove_dir_all(&dir);
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

#[tokio::test]
async fn write_approval_carries_the_patch() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-runtime-diff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        dir.clone(),
        Vec::new(),
        None,
        session.events_sender(),
    ));
    session.use_tools(yi_tools::builtin_tools(), dir.clone(), Some(broker));
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

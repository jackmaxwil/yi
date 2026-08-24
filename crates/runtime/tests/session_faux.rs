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
    session.use_tools(yi_tools::builtin_tools(), dir.clone());
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

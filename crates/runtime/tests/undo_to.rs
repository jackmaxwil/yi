//! Files back to before a chosen turn, by /undo's rule: what the agent moved goes back.
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

use std::error::Error;
use std::sync::Arc;

use scratch::Scratch;
use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, UndoOutcome};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
use yi_types::entry::Entry;
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

fn write(id: &str, path: &str, content: &str) -> AgentMessage {
    let args: Map<String, Value> = match json!({"path": path, "content": content}) {
        Value::Object(args) => args,
        _ => Map::new(),
    };
    faux_assistant_message(vec![faux_tool_call(id, "write", args)], StopReason::ToolUse)
}

/// Dies with a rewind that left the files where the later turns put them: the conversation
/// went back two turns and the work it no longer knew about stayed on disk.
#[tokio::test]
async fn a_restore_to_an_earlier_turn_moves_back_what_the_agent_wrote_and_keeps_hand_edits()
-> Result<(), Box<dyn Error>> {
    let (home, project, sessions) = (
        Scratch::new("yi-undo-to-home")?,
        Scratch::new("yi-undo-to-project")?,
        Scratch::new("yi-undo-to-sessions")?,
    );
    let project = project.to_path_buf();
    let mut repo = JsonlRepo::new(
        sessions.to_path_buf(),
        project.to_string_lossy().into_owned(),
    );
    let store = repo.create(CreateOptions {
        id: Some("undo-to".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        write("c1", "a.txt", "one\n"),
        faux_assistant_message(vec![faux_text("wrote a")], StopReason::Stop),
        write("c2", "b.txt", "two\n"),
        faux_assistant_message(vec![faux_text("wrote b")], StopReason::Stop),
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
    session.attach_store(Arc::clone(&store))?;
    session.use_tools(yi_tools::builtin_tools(), project.clone(), None);
    yi_runtime::wire_turn_checkpoints(&session, &home, &project);
    session.prompt("write a")?;
    session.wait_idle().await;
    session.prompt("write b")?;
    session.wait_idle().await;
    for _ in 0..100 {
        let ends = yi_runtime::recorded(&store)
            .iter()
            .filter(|recorded| recorded.data.at == yi_types::checkpoint::CheckpointAt::TurnEnd)
            .count();
        if ends == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(project.join("a.txt").exists() && project.join("b.txt").exists());
    std::fs::write(project.join("mine.txt"), "hand\n")?;

    let first = lock_session(&store)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })?
        .into_iter()
        .find_map(|entry| match &entry {
            Entry::Message {
                message: AgentMessage::User { .. },
                id,
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .ok_or("no first turn")?;
    let outcome = yi_runtime::undo_to(&store, &first, &project, &home);
    let UndoOutcome::Restored { changes, .. } = outcome else {
        return Err("no restore".into());
    };
    assert!(!changes.is_empty());
    assert!(!project.join("a.txt").exists(), "turn one's file went back");
    assert!(!project.join("b.txt").exists(), "turn two's file went back");
    assert_eq!(std::fs::read_to_string(project.join("mine.txt"))?, "hand\n");
    Ok(())
}

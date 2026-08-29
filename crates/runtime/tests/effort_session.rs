use std::error::Error;
use std::sync::Arc;

use serde_json::json;
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_types::model::{Effort, Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn model(id: &str, reasoning: bool, map: Option<serde_json::Value>) -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning,
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
        thinking_level_map: map,
        headers: None,
    }
}

fn session(model: Model, thinking: Option<Effort>, provider: Arc<ProviderStream>) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model,
            thinking_level: thinking,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

/// The level used to live on the shared [`ProviderStream`], so constructing a
/// child rewrote the parent's for the rest of its life.
#[test]
fn a_child_sharing_the_provider_leaves_the_parent_alone() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let parent = session(
        model("m", true, None),
        Some(Effort::High),
        Arc::clone(&provider),
    );
    let child = session(model("m", true, None), Some(Effort::Low), provider);
    assert_eq!(child.effort(), Effort::Low);
    assert_eq!(parent.effort(), Effort::High);
}

#[test]
fn the_default_is_medium() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(model("m", true, None), None, provider);
    assert_eq!(session.effort(), Effort::Medium);
}

#[test]
fn a_non_reasoning_model_clamps_the_default_to_off() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(model("m", false, None), None, provider);
    assert_eq!(session.effort(), Effort::Off);
}

#[test]
fn set_effort_returns_what_it_actually_set() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(model("m", true, None), None, provider);
    assert_eq!(session.set_effort(Effort::Max), Effort::High);
    assert_eq!(session.effort(), Effort::High);
}

#[test]
fn switching_model_keeps_a_level_the_new_model_advertises() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(model("m", true, None), Some(Effort::Low), provider);
    session.set_model(model("n", true, Some(json!({"max": "max"}))));
    assert_eq!(session.effort(), Effort::Low);
}

#[test]
fn switching_model_clamps_a_level_the_new_model_rejects() {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(
        model("m", true, Some(json!({"max": "max"}))),
        Some(Effort::Max),
        provider,
    );
    session.set_model(model("n", true, None));
    assert_eq!(session.effort(), Effort::High);
}

#[test]
fn resume_comes_back_on_the_model_and_effort_it_left_on() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-effort-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut repo = JsonlRepo::new(dir.clone(), "/tmp/yi-effort-test");
    let store = repo.create(CreateOptions::default())?;
    let id = yi_session::lock_session(&store).metadata().id.clone();

    let provider = Arc::new(ProviderStream::new(None, None));
    let first = session(model("m", true, None), None, Arc::clone(&provider));
    first.attach_store(store)?;
    first.set_effort(Effort::Low);
    assert_eq!(first.effort(), Effort::Low);
    drop(first);

    let reopened = repo.open(&id)?;
    let second = session(model("m", true, None), None, provider);
    second.attach_store(reopened)?;
    assert_eq!(second.effort(), Effort::Low);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

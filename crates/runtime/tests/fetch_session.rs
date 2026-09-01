use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Number;
use yi_runtime::fetch::{FETCH_ENTRY_TYPE, FetchError, Resolver};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Wall};
use yi_types::model::{Model, ModelCost};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || Number::from(0u64);
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

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "fetch-session-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
        },
    )))
}

/// Production order: the resolver is wired from the session's store handle
/// before any store exists, and a store attached afterwards serves
/// `history://` and receives the fetch log's own writes.
#[test]
fn a_store_attached_after_the_resolver_still_serves_history() -> TestResult {
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::new(ProviderStream::new(None, None)),
    );
    let workspace = std::env::temp_dir().join(format!("yi-fetch-session-{}", std::process::id()));
    std::fs::create_dir_all(&workspace)?;
    let resolver = Resolver::new(workspace, Wall::default())
        .with_session_handle("main", session.store_handle());

    let transcript: Url = "history://main".parse()?;
    let before = resolver
        .fetch(&transcript)
        .err()
        .ok_or("history:// must miss before a store is attached, not serve from a stale handle")?;
    assert!(matches!(before, FetchError::Unsupported { .. }), "{before}");

    let store = memory_store();
    let id = yi_session::lock_session(&store).append_custom("main", "note", None)?;
    session.attach_store(store.clone())?;

    let entry: Url = format!("history://main/{id}").parse()?;
    assert_eq!(resolver.fetch(&entry)?.served_by, "session-entry");
    assert_eq!(resolver.fetch(&transcript)?.served_by, "session-transcript");

    let logged = yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery {
        entry_type: Some("custom"),
        custom_type: Some(FETCH_ENTRY_TYPE.to_owned()),
        ..yi_session::EntryQuery::default()
    })?;
    assert!(
        !logged.is_empty(),
        "the fetch log's own writes must land in the late-attached store"
    );
    Ok(())
}

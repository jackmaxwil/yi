//! A session's stream holds one credential per provider (D294): a child on another provider
//! streams with that provider's key, a model with no credential never reaches the wire, and a
//! spawn or a listing refuses what no credential backs.
//!
//! | test | tier | claim | mechanism | contrast |
//! |---|---|---|---|---|
//! | `a_child_model_streams_with_its_own_providers_key` | T1 | An `anthropic` model on a stream that also holds an `openrouter` key sends the `anthropic` one. | `ProviderStream::credential` keyed by `model.provider`. | One stored secret: the last seeded key goes to every provider (the dogfood 401). |
//! | `a_model_with_no_credential_never_reaches_the_wire` | T1 | A provider with no credential gets one `Error` event carrying the startup refusal text, and no connection. | `stream_raw` resolves before it opens a request. | The request goes out with another provider's key. |
//! | `an_entry_near_its_expiry_is_resolved_again_before_use` | T1 | A held credential within 30 s of expiry is resolved again before the request; one two minutes out is kept. | The expiry check in `credential` (D191). | Keep an expiring entry and a days-long session streams a dead token turn by turn. |
//! | `only_credentialed_models_are_offered_or_spawned` | T1 | On an OpenRouter-only setup `find_models` lists only `openrouter` models and `rlm.run(model="anthropic/claude-haiku-4-5")` is refused with the missing-credential text. | `has_credential` filters the listing; `cast` checks before a lane or a lease. | A text-only filter offers models the child then fails on with 401. |

use crate::scratch::Scratch;
use crate::support;

use std::error::Error;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::json;
use yi_loop::run::StreamFn;
use yi_runtime::ProviderStream;
use yi_runtime::auth::{AuthKind, Resolved, Secret, missing_message};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::AgentMessage;
use yi_types::model::{Effort, LlmContext, Model};

type TestResult = Result<(), Box<dyn Error>>;

/// Every request the mock receives, as text; it answers each with an empty 200.
fn listen() -> Result<(u16, mpsc::Receiver<String>), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (sender, requests) = mpsc::channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 8192];
            let read = stream.read(&mut buf).unwrap_or(0);
            let _ = sender.send(String::from_utf8_lossy(&buf[..read]).into_owned());
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n");
        }
    });
    Ok((port, requests))
}

fn model(provider: &str, port: u16) -> Result<Model, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "id": "probe",
        "name": "probe",
        "api": "anthropic-messages",
        "provider": provider,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000,
        "maxTokens": 16
    }))?)
}

fn key(secret: &str) -> Resolved {
    Resolved {
        secret: Secret::new(secret.to_owned()),
        kind: AuthKind::ApiKey,
        org: None,
        expires: None,
        headers: Vec::new(),
    }
}

async fn stream(provider: &ProviderStream, model: &Model) -> Vec<AssistantMessageEvent> {
    let context = LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: String::new(),
        messages: vec![],
        tools: None,
        tool_choice: None,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::OneShot,
    };
    let signal = yi_loop::interrupt::InterruptSignal::default();
    let mut receiver = provider.stream(model, &context, Effort::Off, &signal);
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn a_child_model_streams_with_its_own_providers_key() -> TestResult {
    let (port, requests) = listen()?;
    let provider = ProviderStream::new(None)
        .with_auth("anthropic", key("key-a"))
        .with_auth("openrouter", key("key-or"));
    stream(&provider, &model("anthropic", port)?).await;
    let request = requests.recv_timeout(Duration::from_secs(10))?;
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("x-api-key: key-a"), "{request}");
    assert!(!lower.contains("key-or"), "{request}");
    Ok(())
}

#[tokio::test]
async fn a_model_with_no_credential_never_reaches_the_wire() -> TestResult {
    let (port, requests) = listen()?;
    let provider = ProviderStream::new(None).with_auth("openrouter", key("key-or"));
    let events = stream(&provider, &model("yi-test-nokey", port)?).await;
    if let Ok(request) = requests.recv_timeout(Duration::from_secs(1)) {
        return Err(format!("the request reached the wire: {request}").into());
    }
    let [AssistantMessageEvent::Error { error, .. }] = events.as_slice() else {
        return Err(format!("expected one Error event, got {events:?}").into());
    };
    let AgentMessage::Assistant { error_message, .. } = error else {
        return Err("the Error event carries no assistant message".into());
    };
    assert_eq!(
        error_message.as_deref(),
        Some(missing_message("yi-test-nokey").as_str())
    );
    Ok(())
}

/// Nextest runs each test in its own process, so HOME and the key set here reach no other test.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the entry expires against the clock credential() reads"
)]
fn an_entry_near_its_expiry_is_resolved_again_before_use() -> TestResult {
    let root = Scratch::new("yi-provider-expiry")?;
    unsafe { std::env::set_var("HOME", root.home()?) };
    unsafe { std::env::set_var("OPENROUTER_API_KEY", "key-fresh") };
    let now = std::time::SystemTime::now();
    let held = |seconds: u64| {
        let mut stale = key("key-stale");
        stale.expires = Some(now + Duration::from_secs(seconds));
        ProviderStream::new(None).with_auth("openrouter", stale)
    };
    let near = held(10).credential("openrouter")?;
    assert_eq!(near.secret.expose(), "key-fresh");
    let far = held(120).credential("openrouter")?;
    assert_eq!(far.secret.expose(), "key-stale");
    Ok(())
}

/// Nextest runs each test in its own process, so HOME and the provider variables set here
/// reach no other test.
#[tokio::test]
async fn only_credentialed_models_are_offered_or_spawned() -> TestResult {
    let root = Scratch::new("yi-provider-keys")?;
    let home = root.home()?;
    unsafe { std::env::set_var("HOME", &home) };
    for variable in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"] {
        unsafe { std::env::remove_var(variable) };
    }
    unsafe { std::env::set_var("OPENROUTER_API_KEY", "key-or") };
    let family = support::family(
        root.to_path_buf(),
        root.to_path_buf(),
        support::memory_store("root"),
        None,
    );

    let listed = family.host.find_models("", 10_000);
    let providers: std::collections::BTreeSet<&str> = listed["models"]
        .as_array()
        .ok_or("find_models returned no list")?
        .iter()
        .filter_map(|model| model["provider"].as_str())
        .collect();
    assert_eq!(providers, ["openrouter"].into(), "{listed:?}");

    let kwargs = json!({"model": "anthropic/claude-haiku-4-5"});
    let refused = family.host.spawn(
        "summarize".to_owned(),
        kwargs.as_object().cloned().unwrap_or_default(),
    );
    assert_eq!(refused.err(), Some(missing_message("anthropic")));
    Ok(())
}

/// Dies with no `cost` on an entry, which is a reader picked by name with its price unseen,
/// or with the listing reordered, which makes the cheapest family (the one that refused) the default.
#[tokio::test]
async fn find_models_cost_rides_every_entry_in_registry_order() -> TestResult {
    let root = Scratch::new("yi-find-models-cost")?;
    unsafe { std::env::set_var("HOME", root.home()?) };
    for variable in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"] {
        unsafe { std::env::remove_var(variable) };
    }
    unsafe { std::env::set_var("OPENROUTER_API_KEY", "key-or") };
    let family = support::family(
        root.to_path_buf(),
        root.to_path_buf(),
        support::memory_store("root"),
        None,
    );
    let registry: Vec<Model> = yi_runtime::provider::available_models()
        .into_iter()
        .filter(|model| model.provider == "openrouter")
        .collect();
    let listed = family.host.find_models("", 10_000);
    let entries = listed["models"]
        .as_array()
        .ok_or("find_models returned no list")?;
    let order: Vec<&str> = entries.iter().filter_map(|m| m["id"].as_str()).collect();
    let wanted: Vec<&str> = registry.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(order, wanted);
    for (entry, model) in entries.iter().zip(&registry) {
        let cost = json!({"input": model.cost.input, "output": model.cost.output});
        assert_eq!(entry["cost"], cost, "{entry}");
    }
    Ok(())
}

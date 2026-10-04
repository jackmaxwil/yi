//! A session's model-written title, cleaned to a short plain line.
use crate::scratch;

use std::error::Error;
use std::sync::Arc;

use scratch::Scratch;
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::title::clean;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};

/// Dies with the model's reply used verbatim: quotes, a heading mark, a trailing period and a
/// second line all reached the sidebar as the session's name.
#[test]
fn a_title_is_one_short_plain_line() {
    assert_eq!(
        clean("\"Fix the console context gauge.\"\nBecause the figure was wrong."),
        Some("Fix the console context gauge".to_owned())
    );
    assert_eq!(clean("# Orb states\n"), Some("Orb states".to_owned()));
    assert_eq!(clean("   \n  "), None);
    let long = clean(&"word ".repeat(30)).unwrap_or_default();
    assert!(long.chars().count() <= 48, "{long}");
    assert!(long.ends_with("word"), "cut at a word: {long}");
}

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

/// Dies with the title never asked for: the faux provider was skipped inside the call, so
/// no test took a title from a model reply to the session's stored name.
#[tokio::test]
async fn a_first_turn_is_titled_by_the_model_and_the_name_is_stored() -> Result<(), Box<dyn Error>>
{
    let root = Scratch::new("yi-title-e2e")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-title-e2e");
    let store = repo.create(CreateOptions {
        id: Some("titled".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        faux_assistant_message(vec![faux_text("The gauge now counts.")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_text("\"Fix the context gauge.\"")],
            StopReason::Stop,
        ),
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
    session.attach_store(Arc::clone(&store))?;
    session.prompt("the status row says 0 / 1M")?;
    session.wait_idle().await;
    let title = yi_runtime::title::title_session(&session).await?;
    assert_eq!(title.as_deref(), Some("Fix the context gauge"));
    assert_eq!(
        lock_session(&store).name().as_deref(),
        Some("Fix the context gauge")
    );
    assert_eq!(
        yi_runtime::title::title_session(&session).await?,
        None,
        "a named session keeps its name"
    );
    Ok(())
}

/// Dies with the title's spend never booked: `complete_text` handed back the text alone, so a
/// session's cost total stayed at the main turns' sum while a side call billed on the side.
#[tokio::test]
async fn a_title_call_is_booked_in_the_session_cost_total() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-title-cost")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-title-cost");
    let store = repo.create(CreateOptions {
        id: Some("titled-cost".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        faux_assistant_message(vec![faux_text("The gauge now counts.")], StopReason::Stop),
        crate::support::priced_reply("Fix the context gauge", 0.25),
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
    session.attach_store(Arc::clone(&store))?;
    session.prompt("the status row says 0 / 1M")?;
    session.wait_idle().await;
    assert_eq!(lock_session(&store).stats().cost_total, 0.0);
    yi_runtime::title::title_session(&session).await?;
    let stats = lock_session(&store).stats();
    assert!(
        (stats.cost_total - 0.25).abs() < 1e-9,
        "{}",
        stats.cost_total
    );
    assert_eq!(stats.total_tokens, 120);
    Ok(())
}

fn priced_failure(dollars: f64) -> yi_types::message::AgentMessage {
    let mut message = crate::support::priced_reply("", dollars);
    if let yi_types::message::AgentMessage::Assistant {
        stop_reason,
        error_message,
        ..
    } = &mut message
    {
        *stop_reason = StopReason::Error;
        *error_message = Some("upstream stopped mid-stream".to_owned());
    }
    message
}

/// Dies with a billed call left off the books because its reply was unusable: an empty title
/// and an error stop both answered, and both carry the usage the provider charged.
#[tokio::test]
async fn a_title_call_is_booked_even_when_its_reply_is_unusable() -> Result<(), Box<dyn Error>> {
    for (tag, reply) in [
        ("blank", crate::support::priced_reply("  \n ", 0.25)),
        ("error", priced_failure(0.25)),
    ] {
        let root = Scratch::new(&format!("yi-title-unusable-{tag}"))?;
        let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp/yi-title-unusable");
        let store = repo.create(CreateOptions {
            id: Some(format!("titled-{tag}")),
            ..CreateOptions::default()
        })?;
        let provider = Arc::new(ProviderStream::new(None));
        provider.queue_faux(vec![
            faux_assistant_message(vec![faux_text("The gauge now counts.")], StopReason::Stop),
            reply,
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
        session.attach_store(Arc::clone(&store))?;
        session.prompt("the status row says 0 / 1M")?;
        session.wait_idle().await;
        assert!(yi_runtime::title::title_session(&session).await.is_err());
        let total = lock_session(&store).stats().cost_total;
        assert!((total - 0.25).abs() < 1e-9, "{tag}: {total}");
        assert_eq!(lock_session(&store).name(), None, "{tag}");
    }
    Ok(())
}

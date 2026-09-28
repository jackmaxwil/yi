//! The request facts `yi_ai::compat` derives from a model's id and route, read off the body
//! each wire builds (#749).

use std::error::Error;

use serde_json::Value;
use yi_ai::catalog::Catalog;
use yi_ai::compat::adaptive_thinking;
use yi_ai::openai::{OpenAiOptions, build_params};
use yi_types::model::{Effort, Model};

use crate::openrouter::history_context;

type TestResult = Result<(), Box<dyn Error>>;

fn bundled(provider: &str, id: &str) -> Result<Model, Box<dyn Error>> {
    Catalog::bundled()
        .get(provider, id)
        .cloned()
        .ok_or_else(|| format!("bundled catalog is missing {provider}/{id}").into())
}

/// A reasoning model on OpenRouter under `id`, whatever the bundle knows of it.
fn routed(id: &str) -> Result<Model, Box<dyn Error>> {
    Ok(Model {
        id: id.to_owned(),
        ..bundled("openrouter", "deepseek/deepseek-v4-flash")?
    })
}

fn body(model: &Model) -> Value {
    let options = OpenAiOptions {
        reasoning_effort: Some(Effort::High),
        ..OpenAiOptions::default()
    };
    build_params(model, &history_context(), &options).into_value()
}

#[test]
fn deepseek_from_v4_and_kimi_k2_6_replay_reasoning_content() -> TestResult {
    let replays = |model: &Model| {
        body(model)["messages"][2]
            .get("reasoning_content")
            .is_some()
    };
    for id in [
        "deepseek/deepseek-v4-flash",
        "deepseek/deepseek-v4.1-flash",
        "deepseek/deepseek-v5-pro",
        "~deepseek/deepseek-flash-latest",
        "moonshotai/kimi-k2.6",
    ] {
        assert!(replays(&routed(id)?), "{id}");
    }
    for id in [
        "deepseek/deepseek-r1-0528",
        "deepseek/deepseek-r1-distill-llama-70b",
        "deepseek/deepseek-chat-v3.1",
        "deepseek/deepseek-v3.2",
        "moonshotai/kimi-k2-thinking",
        "moonshotai/kimi-k3",
        "anthropic/claude-opus-5.5",
    ] {
        assert!(!replays(&routed(id)?), "{id}");
    }
    let chat = Model {
        reasoning: false,
        ..routed("deepseek/deepseek-v4-flash")?
    };
    assert!(!replays(&chat), "no thinking, nothing to replay");
    Ok(())
}

/// OpenAI's own endpoint takes `developer` and a flat effort; OpenRouter and Gemini's
/// OpenAI-compatible endpoint take `system`, and only OpenRouter nests the effort.
#[test]
fn the_role_and_the_effort_shape_follow_the_endpoint() -> TestResult {
    let direct = Model {
        api: "openai-completions".to_owned(),
        ..bundled("openai", "gpt-5.5")?
    };
    let gemini = bundled("google", "gemini-2.5-pro")?;
    let cases = [
        (direct, "developer", "reasoning_effort"),
        (gemini, "system", "reasoning_effort"),
        (routed("openai/gpt-6-astra")?, "system", "reasoning"),
        (routed("anthropic/claude-opus-5")?, "system", "reasoning"),
    ];
    for (model, role, effort) in cases {
        let sent = body(&model);
        assert_eq!(sent["messages"][0]["role"], role, "{}", model.id);
        assert!(sent.get(effort).is_some(), "{}: {sent}", model.id);
    }
    Ok(())
}

#[test]
fn claude_is_adaptive_from_generation_4_6() -> TestResult {
    let direct = |id: &str| -> Result<Model, Box<dyn Error>> {
        Ok(Model {
            id: id.to_owned(),
            ..bundled("anthropic", "claude-opus-5")?
        })
    };
    for id in [
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-haiku-5",
    ] {
        assert!(adaptive_thinking(&direct(id)?), "{id}");
    }
    for id in [
        "claude-haiku-4-5-20251001",
        "claude-opus-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-opus-4-1-20250805",
        "claude-opus-4-20250514",
        "claude-3-7-sonnet-20250219",
    ] {
        assert!(!adaptive_thinking(&direct(id)?), "{id}");
    }
    Ok(())
}

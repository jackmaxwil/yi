use serde_json::json;
use yi_ai::catalog::Catalog;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_user_overlay_resolves_both_provider_routes() -> TestResult {
    let scratch = super::scratch::Scratch::new("openrelay-catalog")?;
    let overlay = json!({
        "schema": 2,
        "openai-responses": {
            "bedrock-gpt-6-luna": {
                "id": "bedrock-gpt-6-luna",
                "name": "GPT-6 Luna",
                "api": "openai-responses",
                "provider": "openrelay",
                "baseUrl": "http://127.0.0.1:31337/v1",
                "reasoning": true,
                "input": ["text", "image"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 1050000,
                "maxTokens": 128000
            }
        },
        "anthropic-messages": {
            "bedrock-claude-sonnet-5-5": {
                "id": "bedrock-claude-sonnet-5-5",
                "name": "Claude Sonnet 5.5",
                "api": "anthropic-messages",
                "provider": "openrelay",
                "baseUrl": "http://127.0.0.1:31337/anthropic",
                "reasoning": true,
                "input": ["text", "image"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 1000000,
                "maxTokens": 128000
            }
        }
    });
    std::fs::write(
        scratch.join("openrelay.json"),
        serde_json::to_vec(&overlay)?,
    )?;

    assert!(!Catalog::bundled().has_provider("openrelay"));
    let catalog = Catalog::bundled().with_cache(&scratch);
    for (id, api, base_url, context_window) in [
        (
            "bedrock-gpt-6-luna",
            "openai-responses",
            "http://127.0.0.1:31337/v1",
            1_050_000,
        ),
        (
            "bedrock-claude-sonnet-5-5",
            "anthropic-messages",
            "http://127.0.0.1:31337/anthropic",
            1_000_000,
        ),
    ] {
        let model = catalog
            .get("openrelay", id)
            .ok_or_else(|| format!("missing OpenRelay model {id}"))?;
        assert_eq!(model.api, api);
        assert_eq!(model.base_url, base_url);
        assert_eq!(model.input, vec!["text".to_owned(), "image".to_owned()]);
        assert!(model.reasoning);
        assert_eq!(model.context_window, context_window);
        assert_eq!(model.max_tokens, 128_000);
    }
    Ok(())
}

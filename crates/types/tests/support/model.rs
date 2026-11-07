use serde_json::Value;
use yi_types::model::{Model, ModelCost};

/// The zero-cost fixture model the test binaries of several crates build on.
pub fn model(id: &str, api: &str, provider: &str, base_url: &str, compat: Option<Value>) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: api.to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost::free(),
        context_window: 1_000,
        max_tokens: 100,
        compat,
        thinking_level_map: None,
        headers: None,
        read_timeout_secs: None,
    }
}

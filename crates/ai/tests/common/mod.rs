use serde_json::Value;
use yi_types::model::{Model, ModelCost};

/// The zero-cost fixture model every test in this crate builds its provider calls on.
pub fn model(id: &str, api: &str, provider: &str, base_url: &str, compat: Option<Value>) -> Model {
    let zero = || serde_json::Number::from(0);
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: api.to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 1_000,
        max_tokens: 100,
        compat,
        thinking_level_map: None,
        headers: None,
    }
}

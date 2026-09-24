//! The model registry as a spawn sees it: the `rlm.find_models` listing, and the family rule
//! the judge tier seats its jurors by (plan section 6.4).

use serde_json::{Map, Value, json};
use yi_types::model::Model;

use super::SubagentHost;
use crate::provider::available_models;

impl SubagentHost {
    pub fn find_models(query: &str, limit: usize) -> Map<String, Value> {
        let needle = query.to_lowercase();
        let models: Vec<Value> = available_models()
            .into_iter()
            .filter(|model| {
                needle.is_empty()
                    || model.id.to_lowercase().contains(&needle)
                    || model.name.to_lowercase().contains(&needle)
                    || model.provider.to_lowercase().contains(&needle)
            })
            .take(limit)
            .map(|model| {
                json!({
                    "provider": model.provider,
                    "id": model.id,
                    "name": model.name,
                    "selector": selector_of(&model),
                })
            })
            .collect();
        let mut reply = Map::new();
        reply.insert("models".to_owned(), Value::Array(models));
        reply
    }
}

pub fn selector_of(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// The vendor segment of a recorded model identity: `openrouter/z-ai/glm-5.3-flash` is
/// `z-ai`, `anthropic/claude-x` is `anthropic`. A weak heuristic for independent errors.
pub fn family_of(model: &Model) -> &str {
    model
        .id
        .split_once('/')
        .map_or(model.provider.as_str(), |(vendor, _)| vendor)
}

/// The registry's models of none of the `owners` families, cheapest input first.
pub fn other_families(mut pool: Vec<Model>, owners: &[&str]) -> Vec<Model> {
    pool.retain(|model| !owners.contains(&family_of(model)));
    // ponytail: cheapest first is the reader rule; a configured judge role is the upgrade.
    let cost = |model: &Model| model.cost.input.as_f64().unwrap_or(f64::MAX);
    pool.sort_by(|left, right| {
        cost(left)
            .total_cmp(&cost(right))
            .then_with(|| selector_of(left).cmp(&selector_of(right)))
    });
    pool
}

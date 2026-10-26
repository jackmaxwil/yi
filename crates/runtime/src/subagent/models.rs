//! The model registry as a spawn sees it: the `rlm.find_models` listing.

use serde_json::{Map, Value, json};
use yi_types::model::Model;

use super::SubagentHost;
use crate::provider::{ProviderStream, available_models};

impl SubagentHost {
    pub fn find_models(&self, query: &str, limit: usize) -> Map<String, Value> {
        let needle = query.to_lowercase();
        let models: Vec<Value> = credentialed_models(&self.options.provider)
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
                    "cost": {"input": model.cost.input, "output": model.cost.output},
                })
            })
            .collect();
        let mut reply = Map::new();
        reply.insert("models".to_owned(), Value::Array(models));
        reply
    }
}

/// The registry's models whose provider holds a credential, each provider asked once.
pub fn credentialed_models(stream: &ProviderStream) -> Vec<Model> {
    let mut asked = std::collections::HashMap::new();
    let mut models = available_models();
    models.retain(|model| {
        *asked
            .entry(model.provider.clone())
            .or_insert_with(|| stream.has_credential(&model.provider))
    });
    models
}

pub fn selector_of(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

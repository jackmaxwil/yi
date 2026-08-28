use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;
use yi_types::model::Model;

const ANTHROPIC_DATA: &str = include_str!("../data/anthropic.json");
const OPENAI_DATA: &str = include_str!("../data/openai.json");
const OPENROUTER_DATA: &str = include_str!("../data/openrouter.json");

fn parse_catalog(data: &str, models: &mut HashMap<(String, String), Model>) {
    let Ok(Value::Object(by_api)) = serde_json::from_str::<Value>(data) else {
        return;
    };
    for by_model in by_api.into_iter().filter_map(|(_, value)| match value {
        Value::Object(map) => Some(map),
        _ => None,
    }) {
        for (_, entry) in by_model {
            if let Ok(model) = serde_json::from_value::<Model>(entry) {
                models.insert((model.provider.clone(), model.id.clone()), model);
            }
        }
    }
}

pub struct Catalog {
    models: HashMap<(String, String), Model>,
}

static SHARED: OnceLock<Catalog> = OnceLock::new();

impl Catalog {
    /// The bundled catalog, parsed once per process. [`Catalog::bundled`] parses
    /// 170 KB of JSON, and model lookup runs per turn and per subagent spawn.
    pub fn shared() -> &'static Self {
        SHARED.get_or_init(Self::bundled)
    }

    pub fn bundled() -> Self {
        let mut models = HashMap::new();
        parse_catalog(ANTHROPIC_DATA, &mut models);
        parse_catalog(OPENAI_DATA, &mut models);
        parse_catalog(OPENROUTER_DATA, &mut models);
        Self { models }
    }

    pub fn get(&self, provider: &str, id: &str) -> Option<&Model> {
        self.models.get(&(provider.to_owned(), id.to_owned()))
    }

    pub fn models(&self) -> Vec<Model> {
        let mut listed: Vec<Model> = self.models.values().cloned().collect();
        listed.sort_by(|left, right| (&left.provider, &left.id).cmp(&(&right.provider, &right.id)));
        listed
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

pub fn calculate_cost(model: &Model, usage: &mut yi_types::message::Usage) {
    let as_f64 = |number: &serde_json::Number| number.as_f64().unwrap_or(0.0);
    let input_tokens = usage
        .input
        .saturating_add(usage.cache_read)
        .saturating_add(usage.cache_write);
    let mut rates = (
        as_f64(&model.cost.input),
        as_f64(&model.cost.output),
        as_f64(&model.cost.cache_read),
        as_f64(&model.cost.cache_write),
    );
    let mut matched: i128 = -1;
    for tier in model.cost.tiers.as_deref().unwrap_or_default() {
        if i128::from(input_tokens) > i128::from(tier.input_tokens_above)
            && i128::from(tier.input_tokens_above) > matched
        {
            rates = (
                as_f64(&tier.input),
                as_f64(&tier.output),
                as_f64(&tier.cache_read),
                as_f64(&tier.cache_write),
            );
            matched = i128::from(tier.input_tokens_above);
        }
    }
    let long_write = usage.cache_write1h.unwrap_or(0) as f64;
    let short_write = (usage.cache_write as f64) - long_write;
    let per = 1_000_000.0;
    let number = |value: f64| {
        serde_json::Number::from_f64(value).unwrap_or_else(|| serde_json::Number::from(0u64))
    };
    let input_cost = rates.0 / per * usage.input as f64;
    let output_cost = rates.1 / per * usage.output as f64;
    let cache_read_cost = rates.2 / per * usage.cache_read as f64;
    let cache_write_cost = (rates.3 * short_write + rates.0 * 2.0 * long_write) / per;
    usage.cost.input = number(input_cost);
    usage.cost.output = number(output_cost);
    usage.cost.cache_read = number(cache_read_cost);
    usage.cost.cache_write = number(cache_write_cost);
    usage.cost.total = number(input_cost + output_cost + cache_read_cost + cache_write_cost);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_catalog_is_parsed_once_and_matches_a_fresh_parse() {
        assert!(std::ptr::eq(Catalog::shared(), Catalog::shared()));
        assert_eq!(Catalog::shared().len(), Catalog::bundled().len());
    }

    #[test]
    fn bundled_catalog_loads_models() {
        let catalog = Catalog::bundled();
        assert!(!catalog.is_empty());
        assert!(catalog.get("anthropic", "claude-opus-4-5").is_some() || catalog.len() > 3);
        assert_eq!(
            catalog
                .get("openai", "gpt-5.6-luna")
                .map(|model| model.api.as_str()),
            Some("openai-responses")
        );
    }
}

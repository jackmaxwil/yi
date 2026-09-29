use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;
use yi_types::model::Model;

/// Deflated by `build.rs`: 146,182 bytes of JSON became 11,857 in the binary, and only
/// [`Catalog::bundled`] inflates them, once, behind its `OnceLock`.
const ANTHROPIC_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/anthropic.zz"));
const OPENAI_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openai.zz"));
const OPENROUTER_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openrouter.zz"));
const CODEX_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openai-codex.zz"));
const GOOGLE_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/google.zz"));

pub const PROVIDERS: [&str; 5] = [
    "anthropic",
    "openai",
    "openrouter",
    "openai-codex",
    "google",
];

/// A fetched file's top-level `"schema"`. One under another is stale and rewritten by the next
/// session; its entries still load, so a model only the fetch knows keeps resolving.
pub const SCHEMA: u64 = 2;

pub struct Catalog {
    models: HashMap<(String, String), Model>,
    /// A line per entry or file that did not load; `yi doctor` names them.
    rejected: Vec<String>,
}

static SHARED: OnceLock<Catalog> = OnceLock::new();
static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

impl Catalog {
    /// The bundled catalog under the cache dir's overlay, parsed once per process; model
    /// lookup runs per turn and per subagent spawn.
    pub fn shared() -> &'static Self {
        SHARED.get_or_init(|| match CACHE_DIR.get() {
            Some(dir) => Self::bundled().with_cache(dir),
            None => Self::bundled(),
        })
    }

    /// Set before the first lookup; `yi --version` never looks up, so it never reads a cache.
    pub fn set_cache_dir(dir: PathBuf) {
        let _already_set = CACHE_DIR.set(dir);
    }

    pub fn cache_dir() -> Option<&'static Path> {
        CACHE_DIR.get().map(PathBuf::as_path)
    }

    pub fn bundled() -> Self {
        let mut catalog = Self {
            models: HashMap::new(),
            rejected: Vec::new(),
        };
        for (provider, packed) in PROVIDERS.into_iter().zip([
            ANTHROPIC_DATA,
            OPENAI_DATA,
            OPENROUTER_DATA,
            CODEX_DATA,
            GOOGLE_DATA,
        ]) {
            let source = format!("bundled {provider}.json");
            match miniz_oxide::inflate::decompress_to_vec_zlib(packed) {
                Ok(data) => catalog.parse_catalog(&source, &data),
                Err(error) => catalog.rejected.push(format!("{source}: {error:?}")),
            }
        }
        catalog
    }

    /// Invariant: a cache entry overrides the bundled one by `(provider, id)` and a missing or
    /// unreadable cache file changes nothing, so the bundled set is the floor, never shrunk.
    pub fn with_cache(mut self, dir: &Path) -> Self {
        for provider in PROVIDERS {
            let path = dir.join(format!("{provider}.json"));
            if let Ok(data) = std::fs::read(&path) {
                self.parse_catalog(&path.display().to_string(), &data);
            }
        }
        self
    }

    fn parse_catalog(&mut self, source: &str, data: &[u8]) {
        let by_api = match serde_json::from_slice::<Value>(data) {
            Ok(Value::Object(by_api)) => by_api,
            Ok(_) => return self.rejected.push(format!("{source}: not a JSON object")),
            Err(error) => return self.rejected.push(format!("{source}: {error}")),
        };
        for (api, value) in by_api {
            let by_model = match value {
                Value::Object(map) => map,
                _ if api == "schema" => continue,
                _ => {
                    self.rejected
                        .push(format!("{source}: {api}: not an object"));
                    continue;
                }
            };
            for (id, entry) in by_model {
                match serde_json::from_value::<Model>(entry) {
                    Ok(model) => {
                        self.models
                            .insert((model.provider.clone(), model.id.clone()), model);
                    }
                    Err(error) => self.rejected.push(format!("{source}: {id}: {error}")),
                }
            }
        }
    }

    pub fn rejected(&self) -> &[String] {
        &self.rejected
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

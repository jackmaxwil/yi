//! The fetched catalog: models.dev metadata, narrowed to the ids a provider's own list says a
//! key can reach, written per provider beside the bundled floor.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use yi_types::model::Model;

use crate::catalog::{Catalog, SCHEMA};
use crate::request::{ProxyConfig, get_json};

pub const MODELS_DEV: &str = "https://models.dev/api.json";
pub const CAP_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_REFRESH_HOURS: u64 = 24;

/// The per-provider facts models.dev does not carry: yi's adapter, its endpoint and the effort
/// ladder a new id inherits; the request shape is derived (`compat`), so no flag is copied.
fn adapter(provider: &str) -> Option<(&'static str, &'static str, Value)> {
    Some(match provider {
        "anthropic" => (
            "anthropic-messages",
            "https://api.anthropic.com",
            json!({"off": null, "xhigh": "xhigh", "max": "max"}),
        ),
        "openai" => ("openai-responses", "https://api.openai.com/v1", Value::Null),
        "openrouter" => (
            "openai-completions",
            "https://openrouter.ai/api/v1",
            json!({"off": null}),
        ),
        _ => return None,
    })
}

pub fn list_url(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "anthropic" => "https://api.anthropic.com/v1/models",
        "openai" => "https://api.openai.com/v1/models",
        "openrouter" => "https://openrouter.ai/api/v1/models",
        _ => return None,
    })
}

/// `data[].id` from a provider's own list; `None` when the shape is not that, so an unexpected
/// payload widens nothing and narrows nothing.
pub fn reachable_ids(list: &Value) -> Option<HashSet<String>> {
    let rows = list.get("data")?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| row.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect(),
    )
}

fn number(value: Option<&Value>) -> serde_json::Number {
    value
        .and_then(Value::as_number)
        .cloned()
        .unwrap_or_else(|| serde_json::Number::from(0))
}

fn u64_at(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

/// One models.dev entry as a yi [`Model`]; `known` is the bundled entry whose compat and effort
/// ladder it keeps.
fn model_from(
    provider: &str,
    id: &str,
    entry: &Map<String, Value>,
    known: Option<&Model>,
) -> Option<Model> {
    let (api, base_url, ladder) = adapter(provider)?;
    let cost = entry.get("cost").and_then(Value::as_object);
    let limit = entry.get("limit").and_then(Value::as_object);
    let cost_json = json!({
        "input": number(cost.and_then(|c| c.get("input"))),
        "output": number(cost.and_then(|c| c.get("output"))),
        "cacheRead": number(cost.and_then(|c| c.get("cache_read"))),
        "cacheWrite": number(cost.and_then(|c| c.get("cache_write"))),
    });
    let input = entry
        .get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(Value::as_array)
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["text".to_owned()]);
    Some(Model {
        id: id.to_owned(),
        name: entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned(),
        api: api.to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
        reasoning: entry
            .get("reasoning")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        input,
        cost: serde_json::from_value(cost_json).ok()?,
        context_window: u64_at(limit.and_then(|l| l.get("context"))),
        max_tokens: u64_at(limit.and_then(|l| l.get("output"))),
        compat: known.and_then(|k| k.compat.clone()),
        thinking_level_map: known
            .and_then(|k| k.thinking_level_map.clone())
            .or(if ladder.is_null() { None } else { Some(ladder) }),
        headers: known.and_then(|k| k.headers.clone()),
    })
}

/// Every model models.dev lists for `provider`, narrowed to `reachable` when the provider's
/// own list parsed; the result is the on-disk shape the bundled files use, `{api: {id: Model}}`.
pub fn catalog_from(
    models_dev: &Value,
    provider: &str,
    reachable: Option<&HashSet<String>>,
    bundled: &Catalog,
) -> Value {
    let mut by_id: Map<String, Value> = Map::new();
    let entries = models_dev
        .get(provider)
        .and_then(|p| p.get("models"))
        .and_then(Value::as_object);
    for (id, entry) in entries.into_iter().flatten() {
        if reachable.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        let Some(entry) = entry.as_object() else {
            continue;
        };
        if let Some(model) = model_from(provider, id, entry, bundled.get(provider, id))
            && let Ok(value) = serde_json::to_value(&model)
        {
            by_id.insert(id.clone(), value);
        }
    }
    let api = adapter(provider).map_or("", |(api, ..)| api);
    json!({ api: by_id, "schema": SCHEMA })
}

pub fn cache_path(dir: &Path, provider: &str) -> std::path::PathBuf {
    dir.join(format!("{provider}.json"))
}

pub fn age(dir: &Path, provider: &str, now: SystemTime) -> Option<Duration> {
    let modified = std::fs::metadata(cache_path(dir, provider))
        .ok()?
        .modified()
        .ok()?;
    now.duration_since(modified).ok()
}

/// Missing, older than `hours`, or written under another [`SCHEMA`].
pub fn is_stale(dir: &Path, provider: &str, hours: u64, now: SystemTime) -> bool {
    let current = || {
        std::fs::read(cache_path(dir, provider))
            .ok()
            .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
            .is_some_and(|catalog| catalog.get("schema").and_then(Value::as_u64) == Some(SCHEMA))
    };
    age(dir, provider, now).is_none_or(|age| age >= Duration::from_secs(hours.saturating_mul(3600)))
        || !current()
}

fn write_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path.parent().ok_or("cache path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, value.to_string())
        .map_err(|error| format!("{}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Fetch, narrow, write. Returns the model count written. Never touches the running process's
/// catalog: the next session reads the file.
pub fn refresh(
    dir: &Path,
    provider: &str,
    key: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<usize, String> {
    let url = list_url(provider).ok_or_else(|| format!("no adapter for provider {provider}"))?;
    let models_dev = get_json(MODELS_DEV, &[], proxy, CAP_BYTES)?;
    let headers: Vec<(&str, String)> = match (provider, key) {
        ("anthropic", Some(key)) => vec![
            ("x-api-key", key.to_owned()),
            (
                "anthropic-version",
                crate::anthropic::ANTHROPIC_VERSION.to_owned(),
            ),
        ],
        (_, Some(key)) => vec![("authorization", format!("Bearer {key}"))],
        (_, None) => Vec::new(),
    };
    let reachable = get_json(url, &headers, proxy, CAP_BYTES)
        .ok()
        .and_then(|list| reachable_ids(&list));
    let catalog = catalog_from(
        &models_dev,
        provider,
        reachable.as_ref(),
        &Catalog::bundled(),
    );
    let count: usize = catalog
        .as_object()
        .map(|by_api| {
            by_api
                .values()
                .filter_map(Value::as_object)
                .map(Map::len)
                .sum()
        })
        .unwrap_or(0);
    if count == 0 {
        return Err(format!(
            "{provider}: models.dev listed nothing reachable; cache left as is"
        ));
    }
    write_atomic(&cache_path(dir, provider), &catalog)?;
    Ok(count)
}

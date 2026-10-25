//! The Kimi account's own endpoints at the login profile's `api_base`: the model list is the
//! catalog, so no Kimi id, window or price is compiled in.

use serde_json::{Map, Value, json};
use yi_types::model::{Effort, Model, ModelCost};

use crate::request::{ProxyConfig, get_json};

pub fn api_base() -> Option<String> {
    match yi_oauth::registry::load("kimi") {
        Ok(Some(yi_oauth::registry::Kind::OauthCode(spec))) => spec
            .api_base
            .map(|base| base.trim_end_matches('/').to_owned()),
        _ => None,
    }
}

/// `reasoning.effort.valid` as a level map; a model that thinks with no effort list takes one
/// level, `on`, which the wire sends as thinking enabled with no effort.
fn levels(always: bool, valid: &[&str]) -> Value {
    let mut map = Map::new();
    if always {
        map.insert("off".to_owned(), Value::Null);
    }
    for effort in Effort::ALL.into_iter().skip(1) {
        let name = effort.as_str();
        let level = if valid.is_empty() {
            (effort == Effort::Medium).then_some("on")
        } else {
            valid.contains(&name).then_some(name)
        };
        map.insert(name.to_owned(), level.map_or(Value::Null, Value::from));
    }
    Value::Object(map)
}

fn model_from(id: &str, entry: &Map<String, Value>, base: &str) -> Result<Model, String> {
    let (api, base_url) = match entry.get("protocol").filter(|protocol| !protocol.is_null()) {
        None => ("openai-completions", base),
        Some(protocol) if *protocol == "anthropic" => (
            "anthropic-messages",
            base.strip_suffix("/v1").unwrap_or(base),
        ),
        Some(protocol) if *protocol == "response" => ("openai-responses", base),
        Some(other) => return Err(format!("unknown protocol {other}")),
    };
    let positive = |pointer: &str| {
        entry
            .get("limit")
            .and_then(|limit| limit.get(pointer))
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
    };
    let context_window = positive("context").ok_or("no positive limit.context")?;
    let reasoning = entry.get("reasoning");
    let kind = reasoning
        .and_then(|r| r.get("type"))
        .and_then(Value::as_str);
    let thinks = matches!(kind, Some("only" | "both"));
    let valid: Vec<&str> = reasoning
        .and_then(|r| r.pointer("/effort/valid"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut input: Vec<String> = entry
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|kind| matches!(*kind, "text" | "image"))
        .map(str::to_owned)
        .collect();
    if input.is_empty() {
        input.push("text".to_owned());
    }
    let zero = || serde_json::Number::from(0u8);
    Ok(Model {
        id: id.to_owned(),
        name: entry
            .get("display_name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(id)
            .to_owned(),
        api: api.to_owned(),
        provider: "kimi".to_owned(),
        base_url: base_url.to_owned(),
        reasoning: thinks,
        input,
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window,
        max_tokens: positive("output").unwrap_or(context_window),
        compat: None,
        thinking_level_map: thinks.then(|| levels(kind == Some("only"), &valid)),
        headers: None,
    })
}

/// `GET /models` as the on-disk catalog and the count that loads; an entry that does not map
/// is kept under `rejected` as its reason.
pub fn catalog(list: &Value, base: &str) -> Result<(Value, usize), String> {
    let rows = list
        .get("data")
        .and_then(Value::as_array)
        .ok_or("kimi: /models has no data list")?;
    let mut by_api = Map::new();
    let mut rejected = Map::new();
    let mut loaded = 0usize;
    for (id, entry) in rows.iter().filter_map(|row| {
        let entry = row.as_object()?;
        Some((
            entry.get("id")?.as_str().filter(|id| !id.is_empty())?,
            entry,
        ))
    }) {
        match model_from(id, entry, base) {
            Ok(model) => {
                let value = serde_json::to_value(&model).map_err(|error| error.to_string())?;
                if let Value::Object(group) = by_api.entry(model.api).or_insert_with(|| json!({})) {
                    group.insert(id.to_owned(), value);
                    loaded = loaded.saturating_add(1);
                }
            }
            Err(reason) => {
                rejected.insert(id.to_owned(), Value::String(reason));
            }
        }
    }
    if loaded == 0 {
        return Err(format!(
            "kimi: /models listed no model that loads ({} rejected)",
            rejected.len()
        ));
    }
    if !rejected.is_empty() {
        by_api.insert("rejected".to_owned(), Value::Object(rejected));
    }
    by_api.insert("schema".to_owned(), json!(crate::catalog::SCHEMA));
    Ok((Value::Object(by_api), loaded))
}

pub fn refresh(
    dir: &std::path::Path,
    key: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<usize, String> {
    let key = key.ok_or("kimi: no credential")?;
    let base = api_base().ok_or("kimi: the login profile names no api_base")?;
    let list = get_json(
        &format!("{base}/models"),
        &[("authorization", format!("Bearer {key}"))],
        proxy,
        crate::refresh::CAP_BYTES,
    )?;
    let (written, loaded) = catalog(&list, &base)?;
    crate::refresh::write_atomic(&crate::refresh::cache_path(dir, "kimi"), &written)?;
    Ok(loaded)
}

const USAGE_WINDOWS: [(&str, &str); 4] = [
    ("limit_5h", "5h"),
    ("limit_7d", "7d"),
    ("limit_month_total", "month_total"),
    ("limit_month_code", "month_code"),
];

/// `GET /usages` as one line: each window's used ratio and reset time; a window the account does
/// not report, or reports with no readable ratio, is left out.
pub fn usage_line(payload: &Value) -> Value {
    let mut line = Map::new();
    for (key, name) in USAGE_WINDOWS {
        let Some(window) = payload.get("usages").and_then(|usages| usages.get(key)) else {
            continue;
        };
        let used = window.get("used_ratio").and_then(|ratio| {
            ratio
                .as_f64()
                .or_else(|| ratio.as_str()?.parse().ok())
                .filter(|ratio| ratio.is_finite())
        });
        let Some(used) = used else {
            continue;
        };
        let mut entry = json!({ "used": used });
        if let Some(reset) = window.get("reset_time").and_then(Value::as_str) {
            entry["reset"] = json!(reset);
        }
        line.insert(name.to_owned(), entry);
    }
    Value::Object(line)
}

pub fn usages(key: &str, proxy: Option<&ProxyConfig>) -> Result<Value, String> {
    let base = api_base().ok_or("kimi: the login profile names no api_base")?;
    let payload = get_json(
        &format!("{base}/usages"),
        &[("authorization", format!("Bearer {key}"))],
        proxy,
        crate::refresh::CAP_BYTES,
    )?;
    Ok(usage_line(&payload))
}

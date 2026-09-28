//! The fetched catalog, driven by slices of the real 2026-09-05 payloads; no network.

use std::error::Error;

use serde_json::{Value, json};
use yi_ai::catalog::Catalog;
use yi_ai::openai::{OpenAiOptions, build_params};
use yi_ai::refresh::{catalog_from, is_stale, reachable_ids};

use crate::openrouter::history_context;
use crate::scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const MODELS_DEV: &str = include_str!("fixtures/models_dev_2026-09-05.json");
const OPENROUTER_LIST: &str = include_str!("fixtures/openrouter_models_2026-09-05.json");
/// models.dev's OpenRouter entries for the three DeepSeek ids and the Claude the 2026-09-27
/// dogfood fetched, none of them in the bundle.
const MODELS_DEV_0928: &str = include_str!("fixtures/models_dev_2026-09-28.json");

/// `written` as the next session reads it: the bundled floor under the cache file.
fn overlay(written: &Value) -> Result<Catalog, Box<dyn Error>> {
    let dir = Scratch::new("yi-refresh-derived")?;
    std::fs::write(dir.join("openrouter.json"), written.to_string())?;
    Ok(Catalog::bundled().with_cache(&dir))
}

fn fetched_0928() -> Result<(Value, Catalog), Box<dyn Error>> {
    let models_dev: Value = serde_json::from_str(MODELS_DEV_0928)?;
    let written = catalog_from(&models_dev, "openrouter", None, &Catalog::bundled());
    let catalog = overlay(&written)?;
    Ok((written, catalog))
}

/// A model the bundled files predate resolves once its cache file exists, with the cost and
/// limits models.dev published and the reasoning shape OpenRouter takes.
#[test]
fn a_model_newer_than_the_bundle_resolves_from_the_cache() -> TestResult {
    let models_dev: Value = serde_json::from_str(MODELS_DEV)?;
    let list: Value = serde_json::from_str(OPENROUTER_LIST)?;
    let bundled = Catalog::bundled();
    assert!(
        bundled.get("openrouter", "openai/gpt-6-astra").is_none(),
        "the bundle predates it"
    );
    let reachable = reachable_ids(&list).ok_or("openrouter list shape")?;
    let written = catalog_from(&models_dev, "openrouter", Some(&reachable), &bundled);
    let dir = Scratch::new("yi-refresh-overlay")?;
    std::fs::write(dir.join("openrouter.json"), written.to_string())?;
    let catalog = Catalog::bundled().with_cache(&dir);
    let astra = catalog
        .get("openrouter", "openai/gpt-6-astra")
        .ok_or("astra missing")?;
    assert_eq!(astra.api, "openai-completions");
    assert_eq!(astra.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(astra.context_window, 1_050_000);
    assert_eq!(astra.max_tokens, 128_000);
    assert_eq!(astra.cost.input.as_f64(), Some(10.0));
    assert_eq!(astra.cost.cache_read.as_f64(), Some(1.0));
    assert!(astra.reasoning);
    let options = OpenAiOptions {
        reasoning_effort: Some(yi_types::model::Effort::High),
        ..OpenAiOptions::default()
    };
    let body = build_params(astra, &history_context(), &options);
    assert_eq!(
        body["reasoning"],
        json!({"effort": "high"}),
        "OpenRouter's shape"
    );
    assert!(
        catalog
            .get("openrouter", "openai/gpt-6-astra-pro")
            .is_none(),
        "not in the reachable list"
    );
    assert!(
        catalog.len() > bundled.len(),
        "the overlay adds, never replaces"
    );
    Ok(())
}

/// A bundled id keeps its bundled compat when models.dev also lists it.
#[test]
fn a_bundled_id_keeps_its_compat_through_a_refresh() -> TestResult {
    let models_dev: Value = serde_json::from_str(MODELS_DEV)?;
    let bundled = Catalog::bundled();
    let known = bundled
        .get("anthropic", "claude-opus-5")
        .ok_or("opus in bundle")?;
    let written = catalog_from(&models_dev, "anthropic", None, &bundled);
    let entry = &written["anthropic-messages"]["claude-opus-5"];
    assert_eq!(entry["compat"], known.compat.clone().unwrap_or(Value::Null));
    assert_eq!(
        entry["thinkingLevelMap"],
        known.thinking_level_map.clone().unwrap_or(Value::Null)
    );
    Ok(())
}

/// A cache file that is not the catalog shape leaves the bundled set exactly as it was.
#[test]
fn a_corrupt_cache_file_changes_nothing() -> TestResult {
    let dir = Scratch::new("yi-refresh-corrupt")?;
    std::fs::write(dir.join("openrouter.json"), b"{not json")?;
    std::fs::write(dir.join("openai.json"), b"[]")?;
    let catalog = Catalog::bundled().with_cache(&dir);
    assert_eq!(catalog.len(), Catalog::bundled().len());
    Ok(())
}

/// An unexpected list shape means "no filter", never "nothing reachable".
#[test]
fn an_unexpected_list_shape_is_no_filter() -> TestResult {
    assert!(reachable_ids(&serde_json::json!({"models": []})).is_none());
    assert!(
        reachable_ids(&serde_json::json!({"data": [{"id": "a"}, {"name": "b"}]}))
            .is_some_and(|ids| ids.len() == 1)
    );
    Ok(())
}

/// A missing file is stale; a fresh one is not; the threshold is in hours.
#[test]
fn staleness_reads_the_file_age() -> TestResult {
    let dir = Scratch::new("yi-refresh-stale")?;
    let epoch = std::time::UNIX_EPOCH;
    assert!(
        is_stale(&dir, "openai", 24, epoch),
        "no file is stale at any clock"
    );
    std::fs::write(
        dir.join("openai.json"),
        json!({"schema": yi_ai::catalog::SCHEMA}).to_string(),
    )?;
    let written = std::fs::metadata(dir.join("openai.json"))?.modified()?;
    let hour = std::time::Duration::from_secs(3600);
    assert!(!is_stale(&dir, "openai", 24, written + hour));
    assert!(is_stale(&dir, "openai", 24, written + 25 * hour));
    assert!(is_stale(&dir, "openai", 0, written));
    Ok(())
}

/// Their bundled V4 siblings carry the field; the fetched ids had no flag, so the second turn
/// went out without it (#749).
#[test]
fn a_fetched_deepseek_replays_reasoning_content_on_its_assistant_turns() -> TestResult {
    let (_, catalog) = fetched_0928()?;
    for id in [
        "deepseek/deepseek-v4.1-flash",
        "~deepseek/deepseek-flash-latest",
        "~deepseek/deepseek-pro-latest",
    ] {
        let model = catalog.get("openrouter", id).ok_or(id)?;
        let body = build_params(model, &history_context(), &OpenAiOptions::default());
        let assistant = &body["messages"][2];
        assert_eq!(assistant["role"], "assistant", "{id}");
        assert_eq!(assistant["reasoning_content"], "", "{id}: {assistant}");
    }
    Ok(())
}

/// The role is the first byte string of the stable prefix, so a Claude named by the bundle
/// and one named by the fetch must spell it the same way.
#[test]
fn bundled_and_fetched_claude_send_the_same_role() -> TestResult {
    let (_, catalog) = fetched_0928()?;
    let role = |id: &str| -> Result<Value, Box<dyn Error>> {
        let model = catalog.get("openrouter", id).ok_or(id.to_owned())?;
        let body = build_params(model, &history_context(), &OpenAiOptions::default());
        Ok(body["messages"][0]["role"].clone())
    };
    let bundled = role("anthropic/claude-opus-5")?;
    let fetched = role("anthropic/claude-opus-5.5")?;
    assert_eq!(bundled, fetched);
    assert_eq!(fetched, "system");
    Ok(())
}

/// A hand-edited cache entry that no longer parses is named, and the rest of the file loads.
#[test]
fn a_malformed_cache_entry_is_reported_not_dropped() -> TestResult {
    let (mut written, _) = fetched_0928()?;
    written["openai-completions"]["deepseek/deepseek-v4.1-flash"]["contextWindow"] = json!("1M");
    let catalog = overlay(&written)?;
    assert!(
        catalog
            .get("openrouter", "deepseek/deepseek-v4.1-flash")
            .is_none()
    );
    assert!(
        catalog
            .get("openrouter", "~deepseek/deepseek-pro-latest")
            .is_some()
    );
    let named: Vec<&String> = catalog
        .rejected()
        .iter()
        .filter(|line| line.contains("deepseek/deepseek-v4.1-flash"))
        .collect();
    assert_eq!(named.len(), 1, "{:?}", catalog.rejected());
    assert!(named[0].contains("openrouter"), "{}", named[0]);
    assert!(Catalog::bundled().rejected().is_empty());
    Ok(())
}

/// A file written before the schema existed is stale however young it is, so the next session
/// rewrites it; the file a refresh writes now is not.
#[test]
fn a_cache_file_from_an_older_schema_is_stale() -> TestResult {
    let dir = Scratch::new("yi-refresh-schema")?;
    let hour = std::time::Duration::from_secs(3600);
    std::fs::write(
        dir.join("openrouter.json"),
        json!({"openai-completions": {}}).to_string(),
    )?;
    let written = std::fs::metadata(dir.join("openrouter.json"))?.modified()?;
    assert!(is_stale(&dir, "openrouter", 24, written + hour));
    let (fresh, _) = fetched_0928()?;
    std::fs::write(dir.join("openrouter.json"), fresh.to_string())?;
    let written = std::fs::metadata(dir.join("openrouter.json"))?.modified()?;
    assert!(!is_stale(&dir, "openrouter", 24, written + hour));
    Ok(())
}

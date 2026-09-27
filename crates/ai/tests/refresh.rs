//! The fetched catalog, driven by slices of the real 2026-09-05 payloads; no network.

use std::error::Error;

use serde_json::Value;
use yi_ai::catalog::Catalog;
use yi_ai::refresh::{catalog_from, is_stale, reachable_ids};

use crate::scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const MODELS_DEV: &str = include_str!("fixtures/models_dev_2026-09-05.json");
const OPENROUTER_LIST: &str = include_str!("fixtures/openrouter_models_2026-09-05.json");

/// A model the bundled files predate resolves once its cache file exists, with the cost and
/// limits models.dev published and the compat block OpenRouter ids need.
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
    assert_eq!(
        astra
            .compat
            .as_ref()
            .and_then(|c| c.get("thinkingFormat"))
            .and_then(Value::as_str),
        Some("openrouter")
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
    std::fs::write(dir.join("openai.json"), b"{}")?;
    let written = std::fs::metadata(dir.join("openai.json"))?.modified()?;
    let hour = std::time::Duration::from_secs(3600);
    assert!(!is_stale(&dir, "openai", 24, written + hour));
    assert!(is_stale(&dir, "openai", 24, written + 25 * hour));
    assert!(is_stale(&dir, "openai", 0, written));
    Ok(())
}

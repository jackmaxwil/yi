//! The catalog's own read deadline: an entry that sets it, one that does not, and the wire name
//! every loader reads it through.
use std::fs;
use std::path::Path;
use std::time::Duration;

use yi_ai::catalog::Catalog;
use yi_types::model::DEFAULT_READ_TIMEOUT_SECS;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Dies with a loader that drops the field: the entry is parsed by the same serde shape in the
/// bundled, cached and custom catalogs, and a cache that loses it hands the model 60 s again.
#[test]
fn a_catalog_entry_carries_its_read_deadline_through_the_loader() -> TestResult {
    let scratch = Scratch::new("catalog-read-timeout")?;
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog-read-timeout-v1.json");
    fs::copy(fixture, scratch.join("anthropic.json"))?;
    let catalog = Catalog::bundled().with_cache(&scratch);
    let slow = catalog
        .get("local", "local-slow-1")
        .ok_or("the cached entry did not load")?;
    assert_eq!(slow.read_timeout_secs, Some(600));
    assert_eq!(slow.read_timeout(), Duration::from_secs(600));
    let brisk = catalog
        .get("local", "local-brisk-1")
        .ok_or("the cached entry without the field did not load")?;
    assert_eq!(brisk.read_timeout_secs, None);
    assert_eq!(
        brisk.read_timeout(),
        Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS)
    );
    Ok(())
}

/// Dies with a refresh that has to know the field: it is written only by an author, and an entry
/// without one re-emits without the key, so a rebuilt catalog is not padded with `null`.
#[test]
fn an_entry_that_sets_no_deadline_writes_no_key() -> TestResult {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog-read-timeout-v1.json");
    let data: serde_json::Value = serde_json::from_str(&fs::read_to_string(fixture)?)?;
    let mut entry: yi_types::model::Model =
        serde_json::from_value(data["openai-chat"]["local-brisk-1"].clone())?;
    let written = serde_json::to_string(&entry)?;
    assert!(!written.contains("readTimeoutSecs"), "{written}");
    entry.read_timeout_secs = Some(600);
    let written = serde_json::to_string(&entry)?;
    assert!(written.contains("\"readTimeoutSecs\":600"), "{written}");
    Ok(())
}

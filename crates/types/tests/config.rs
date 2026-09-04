use std::error::Error;
use std::path::Path;

use yi_types::config::UserConfig;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn compaction_token_fixture_deserializes_beside_the_session_golden() -> TestResult {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config-compaction-tokens.json");
    let loaded: UserConfig = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let compaction = loaded.compaction.ok_or("compaction missing")?;
    assert_eq!(compaction.reserve_tokens, Some(1000));
    assert_eq!(compaction.keep_recent_tokens, Some(10));
    Ok(())
}

use std::error::Error;

use yi_types::subagent::{ChildResult, Discovery};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_result_without_discoveries_does_not_decode() {
    let error = serde_json::from_str::<ChildResult>(r#"{"value": {"files": 3}}"#)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        error.contains("discoveries"),
        "a child that omits discoveries must fail to decode, never arrive as an empty list: {error}"
    );
}

#[test]
fn a_result_round_trips_with_unknown_keys_intact() -> TestResult {
    let wire = r#"{"value":42,"discoveries":[{"text":"the wall config is stale","violatesCheckOf":"t2","fingerprint":"abc123","source":"tests"}],"notes":"kept"}"#;
    let parsed: ChildResult = serde_json::from_str(wire)?;
    assert_eq!(
        parsed
            .discoveries
            .first()
            .map(|row| row.violates_check_of.clone()),
        Some(Some("t2".to_owned())),
        "the ancestor a discovery names is what the runtime re-checks"
    );
    assert_eq!(
        serde_json::to_string(&parsed)?,
        wire,
        "unknown keys and field order survive the seam a mining reader parses"
    );
    Ok(())
}

#[test]
fn a_discovery_without_an_ancestor_omits_the_field() -> TestResult {
    let row = Discovery {
        text: "logs are unrotated".to_owned(),
        violates_check_of: None,
        fingerprint: "deadbeef".to_owned(),
        extra: serde_json::Map::new(),
    };
    assert_eq!(
        serde_json::to_string(&row)?,
        r#"{"text":"logs are unrotated","fingerprint":"deadbeef"}"#,
        "a deferred row must not carry a null ancestor a reader would resolve"
    );
    Ok(())
}

use std::error::Error;

use serde_json::json;
use yi_runtime::schema::Schema;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_malformed_schema_is_refused_by_path() -> TestResult {
    for (malformed, path) in [
        (json!([1]), "$"),
        (json!(null), "$"),
        (json!(30), "$"),
        (json!(true), "$"),
        (
            json!({"type": "object", "properties": {"files": 3}}),
            "$.properties.files",
        ),
        (json!({"items": "x"}), "$.items"),
        (json!({"required": "files"}), "$.required"),
        (json!({"type": ["string", "null"]}), "$.type"),
        (json!({"type": "str"}), "$.type"),
    ] {
        let refusal = Schema::from_value(malformed.clone())
            .err()
            .ok_or_else(|| format!("{malformed} was accepted as a schema"))?;
        assert!(
            refusal.starts_with(&format!("{path}: ")),
            "{malformed} must be refused at {path}: {refusal}"
        );
    }
    assert_eq!(
        Schema::from_value(json!({"type": "object", "properties": {"files": 3}})).err(),
        Some("$.properties.files: expected a schema object, found number".to_owned()),
        "the refusal names the path and what it found there"
    );
    Ok(())
}

#[test]
fn a_well_formed_schema_still_carries_unknown_keys_and_rejects_a_mismatch() -> TestResult {
    Schema::from_value(json!({}))?.validate(&json!(1))?;

    let carried =
        Schema::from_value(json!({"$schema": "x", "description": "y", "type": "object"}))?;
    let instruction = carried.instruction();
    assert!(
        instruction.contains("\"$schema\"") && instruction.contains("\"description\""),
        "keywords this checker does not read still reach the model: {instruction}"
    );
    assert!(
        carried.validate(&json!(1)).is_err(),
        "an object schema refuses a number"
    );
    carried.validate(&json!({}))?;
    Ok(())
}

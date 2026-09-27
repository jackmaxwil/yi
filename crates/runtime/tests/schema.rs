//! The JSON Schema subset `yi_runtime::schema` implements, and what it refuses.
//!
//! F0c. The subset is `type`, `required`, `properties`, `items` and `enum`, and it
//! ignores every other keyword (`schema.rs:3-4`). Ignoring is safe while a schema is
//! only an instruction to a model; it stops being safe the moment a schema is a
//! criterion, because a contract carrying `{"type": "number", "minimum": 0}` would
//! then read as a promise the verifier silently does not keep, and a negative number
//! would pass the item.
//!
//! | test | tier | what it pins | the control it dies with |
//! |---|---|---|---|
//! | `unsupported_schema_assertion_is_refused` | T0 | A criterion schema carrying an assertion keyword the subset does not implement (`minimum`, `maximum`, `pattern`, `additionalProperties`, `minItems`, `oneOf`, `$ref`, and the rest) is refused at `start`, when the contract is frozen, naming the JSON path of the offending keyword. Descriptive keywords (`description`, `title`, `examples`) are allowed by an explicit list, not by falling through. | The allowlist being a list of what is allowed rather than a list of what is refused. Make it a denylist and the next keyword anyone writes is silently ignored again, which is the failure mode being fixed; move the refusal from `start` to `done` and a frozen contract can name a criterion that was never checkable. The refusal is on the criterion path only: a schema handed to a model as an instruction keeps ignoring what it does not implement, because there it costs nothing. |

use crate::scratch;
use scratch::Scratch;

use std::error::Error;

use serde_json::json;
use yi_runtime::plan::artifact::Artifacts;
use yi_runtime::plan::verify::freeze;
use yi_runtime::schema::Schema;
use yi_types::plan::contract::Contract;

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

// Dies with the criterion allowlist in `schema.rs`: make it a list of what is refused and the
// next assertion keyword anyone writes is silently ignored again.
#[test]
fn unsupported_schema_assertion_is_refused() -> TestResult {
    for (keyword, document, path) in [
        (
            "minimum",
            json!({"type": "number", "minimum": 0}),
            "$.minimum",
        ),
        (
            "maximum",
            json!({"type": "number", "maximum": 10}),
            "$.maximum",
        ),
        (
            "multipleOf",
            json!({"type": "number", "multipleOf": 2}),
            "$.multipleOf",
        ),
        (
            "pattern",
            json!({"type": "string", "pattern": "^yi-"}),
            "$.pattern",
        ),
        (
            "minLength",
            json!({"type": "string", "minLength": 1}),
            "$.minLength",
        ),
        (
            "format",
            json!({"type": "string", "format": "uri"}),
            "$.format",
        ),
        (
            "additionalProperties",
            json!({"type": "object", "additionalProperties": false}),
            "$.additionalProperties",
        ),
        (
            "minItems",
            json!({"type": "array", "minItems": 1}),
            "$.minItems",
        ),
        (
            "uniqueItems",
            json!({"type": "array", "uniqueItems": true}),
            "$.uniqueItems",
        ),
        ("const", json!({"const": "ready"}), "$.const"),
        ("allOf", json!({"allOf": [{"type": "object"}]}), "$.allOf"),
        ("anyOf", json!({"anyOf": [{"type": "object"}]}), "$.anyOf"),
        ("oneOf", json!({"oneOf": [{"type": "object"}]}), "$.oneOf"),
        ("not", json!({"not": {"type": "null"}}), "$.not"),
        ("$ref", json!({"$ref": "#/$defs/finding"}), "$.$ref"),
        (
            "minimum",
            json!({"type": "object", "properties": {"score": {"type": "number", "minimum": 0}}}),
            "$.properties.score.minimum",
        ),
        (
            "pattern",
            json!({"type": "array", "items": {"type": "string", "pattern": "^yi-"}}),
            "$.items.pattern",
        ),
    ] {
        let refusal = Schema::criterion(document.clone())
            .err()
            .ok_or_else(|| format!("{document} was accepted as a criterion"))?;
        assert!(
            refusal.starts_with(&format!("{path}: ")) && refusal.contains(keyword),
            "the refusal names {keyword} and {path}: {refusal}"
        );
        // The same document stays legal as an instruction to a model, where ignoring a keyword
        // costs nothing: the refusal is on the criterion path only.
        Schema::from_value(document)?;
    }

    // Descriptive keywords pass by list, not by falling through.
    Schema::criterion(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "yi://findings",
        "$comment": "the findings shape",
        "title": "Findings",
        "description": "one finding per source",
        "default": [],
        "examples": [[]],
        "type": "array",
        "items": {
            "type": "object",
            "required": ["url", "claim"],
            "properties": {"url": {"type": "string"}, "claim": {"enum": ["yes", "no"]}}
        }
    }))?;
    Ok(())
}

// Dies with `freeze` reading a criterion schema as a criterion: refuse at done instead and a
// frozen contract can name a criterion that was never checkable.
#[test]
fn the_assertion_refusal_lands_when_the_contract_is_frozen() -> TestResult {
    let temp = Scratch::new("yi-schema-criterion")?;
    let artifacts = Artifacts::under(&temp);
    let document =
        json!({"type": "object", "properties": {"score": {"type": "number", "minimum": 0}}});
    let schema = artifacts.put(
        &serde_json::to_vec(&document)?,
        "application/schema+json",
        "test",
    )?;
    let contract: Contract = serde_json::from_value(json!({
        "class": "reader",
        "items": [{"id": "findings-shape", "critical": true, "weight": 100,
                   "decider": {"schema": {"schema": schema}}}],
        "threshold": 1000, "min_coverage": 1000
    }))?;
    let refusal = freeze(&artifacts, &contract)
        .err()
        .ok_or("freeze accepted a criterion the verifier cannot decide")?;
    assert!(
        refusal.contains("findings-shape") && refusal.contains("$.properties.score.minimum"),
        "{refusal}"
    );
    Ok(())
}

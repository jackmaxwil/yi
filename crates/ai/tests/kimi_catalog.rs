//! The Kimi account's `/models` list as yi models. Fixtures are the payloads Kimi Code's own
//! tests replay against its reference parser (packages/oauth/test/managed-kimi-code.test.ts).

use std::error::Error;

use serde_json::{Value, json};
use yi_ai::catalog::Catalog;
use yi_ai::kimi::catalog;
use yi_types::model::{Effort, Model};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const KIMI_CODE: &str = include_str!("fixtures/kimi-models-kimi-code.json");
const BASE: &str = "https://h/coding/v1";

/// `list` as the next session loads it: written under a cache dir, read over the bundle.
fn loaded(list: &Value, base: &str) -> Result<Catalog, Box<dyn Error>> {
    let (written, _) = catalog(list, base)?;
    let dir = Scratch::new("yi-kimi-catalog")?;
    std::fs::write(dir.join("kimi.json"), written.to_string())?;
    Ok(Catalog::bundled().with_cache(&dir))
}

fn payload(key: &str) -> Result<Value, Box<dyn Error>> {
    let all: Value = serde_json::from_str(KIMI_CODE)?;
    all.get(key)
        .cloned()
        .ok_or_else(|| format!("fixture has no {key}").into())
}

fn model<'a>(catalog: &'a Catalog, id: &str) -> Result<&'a Model, Box<dyn Error>> {
    catalog
        .get("kimi", id)
        .ok_or_else(|| format!("kimi/{id} did not load: {:?}", catalog.rejected()).into())
}

/// One row at a time against the reference parser's reading: `both` thinks and can stop,
/// `only` cannot stop, `none` and an absent object do not think, and `effort.valid` is the
/// ladder offered.
#[test]
fn a_models_list_becomes_kimi_models_with_protocol_window_and_efforts() -> TestResult {
    let first = loaded(&payload("managed-kimi-code.test.ts:22-40")?, BASE)?;
    let coding = model(&first, "kimi-for-coding")?;
    assert_eq!(coding.name, "Kimi for Coding");
    assert_eq!(
        (coding.api.as_str(), coding.base_url.as_str()),
        ("openai-completions", BASE)
    );
    assert_eq!(
        (coding.context_window, coding.max_tokens),
        (262_144, 262_144)
    );
    assert_eq!(coding.input, ["text", "image"]);
    assert!(coding.reasoning);
    assert_eq!(coding.supported_efforts(), [Effort::Off, Effort::Medium]);
    assert_eq!(coding.cost.input.as_u64(), Some(0));
    let plain = model(&first, "kimi-k2.5")?;
    assert_eq!(
        (plain.name.as_str(), plain.context_window),
        ("kimi-k2.5", 250_000)
    );
    assert!(
        !plain.reasoning,
        "an absent reasoning object is no thinking"
    );
    assert_eq!(plain.supported_efforts(), [Effort::Off]);

    let second = loaded(&payload("managed-kimi-code.test.ts:843-866")?, BASE)?;
    let only = model(&second, "kimi-for-coding")?;
    assert!(only.reasoning);
    assert_eq!(
        only.supported_efforts(),
        [Effort::Medium],
        "`only` cannot turn thinking off"
    );
    assert!(
        !model(&second, "kimi-plain")?.reasoning,
        "`none` is no thinking"
    );
    assert_eq!(
        model(&second, "kimi-toggle")?.supported_efforts(),
        [Effort::Off, Effort::Medium]
    );

    let third = loaded(&payload("managed-kimi-code.ts:460-461")?, BASE)?;
    assert_eq!(
        model(&third, "kimi-efforts")?.supported_efforts(),
        [Effort::Low, Effort::High, Effort::Max]
    );
    Ok(())
}

/// Kimi's Messages endpoint sits at the base without `/v1`; the adapter appends `/v1/messages`.
#[test]
fn an_anthropic_protocol_entry_streams_messages_at_the_base_without_v1() -> TestResult {
    let list = json!({"data": [
        {"id": "k-anthropic", "limit": {"context": 1000}, "protocol": "anthropic"},
        {"id": "k-response", "limit": {"context": 1000}, "protocol": "response"},
    ]});
    let catalog = loaded(&list, BASE)?;
    let messages = model(&catalog, "k-anthropic")?;
    assert_eq!(
        (messages.api.as_str(), messages.base_url.as_str()),
        ("anthropic-messages", "https://h/coding")
    );
    let responses = model(&catalog, "k-response")?;
    assert_eq!(
        (responses.api.as_str(), responses.base_url.as_str()),
        ("openai-responses", BASE)
    );
    Ok(())
}

/// A model with no window or a wire Yi does not speak never loads, and the reason is a line
/// `yi doctor` names rather than a silent drop.
#[test]
fn an_entry_without_limit_context_or_with_an_unknown_protocol_is_a_rejected_line() -> TestResult {
    let list = json!({"data": [
        {"id": "k-ok", "limit": {"context": 1000}},
        {"id": "k-nowindow"},
        {"id": "k-zero", "limit": {"context": 0}},
        {"id": "k-grpc", "limit": {"context": 1000}, "protocol": "grpc"},
    ]});
    let catalog = loaded(&list, BASE)?;
    model(&catalog, "k-ok")?;
    for id in ["k-nowindow", "k-zero", "k-grpc"] {
        assert!(catalog.get("kimi", id).is_none(), "{id} loaded");
    }
    let mut lines: Vec<String> = catalog
        .rejected()
        .iter()
        .filter_map(|line| {
            line.rsplit_once("kimi.json: ")
                .map(|(_, tail)| tail.to_owned())
        })
        .collect();
    lines.sort();
    assert_eq!(
        lines,
        [
            "k-grpc: unknown protocol \"grpc\"",
            "k-nowindow: no positive limit.context",
            "k-zero: no positive limit.context",
        ]
    );
    let none_loads = json!({"data": [{"id": "k-nowindow"}]});
    let refused = catalog_err(&none_loads)?;
    assert_eq!(
        refused,
        "kimi: /models listed no model that loads (1 rejected)"
    );
    Ok(())
}

fn catalog_err(list: &Value) -> Result<String, Box<dyn Error>> {
    match catalog(list, BASE) {
        Ok(_) => Err("a list with nothing loadable was written".into()),
        Err(error) => Ok(error),
    }
}

/// The completion cap is the model's `max_tokens`: a list with no `limit.output` must not make
/// it 0, which every request would send as `max_completion_tokens: 0`.
#[test]
fn no_limit_output_never_sends_a_zero_completion_cap() -> TestResult {
    let list = json!({"data": [
        {"id": "k-window", "limit": {"context": 262144}},
        {"id": "k-capped", "limit": {"context": 262144, "output": 32000}},
    ]});
    let catalog = loaded(&list, BASE)?;
    assert_eq!(model(&catalog, "k-window")?.max_tokens, 262_144);
    assert_eq!(model(&catalog, "k-capped")?.max_tokens, 32_000);
    Ok(())
}

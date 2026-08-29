use std::str::FromStr;

use serde_json::{Number, json};
use yi_types::model::{Effort, Model, ModelCost, UnknownEffort};

fn model(reasoning: bool, map: Option<serde_json::Value>) -> Model {
    let zero = || Number::from(0u64);
    Model {
        id: "m".to_owned(),
        name: "M".to_owned(),
        api: "openai-completions".to_owned(),
        provider: "p".to_owned(),
        base_url: String::new(),
        reasoning,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 1000,
        max_tokens: 100,
        compat: None,
        thinking_level_map: map,
        headers: None,
    }
}

#[test]
fn non_reasoning_model_advertises_only_off() {
    assert_eq!(model(false, None).supported_efforts(), vec![Effort::Off]);
}

#[test]
fn absent_map_hides_the_advanced_tiers() {
    assert_eq!(
        model(true, None).supported_efforts(),
        vec![
            Effort::Off,
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High
        ]
    );
}

#[test]
fn null_excludes_and_advanced_tiers_are_opt_in() {
    let fable = model(
        true,
        Some(json!({"off": null, "xhigh": "xhigh", "max": "max"})),
    );
    assert_eq!(
        fable.supported_efforts(),
        vec![
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max
        ]
    );
}

#[test]
fn every_level_null_still_yields_a_level() {
    let map = json!({
        "off": null, "minimal": null, "low": null,
        "medium": null, "high": null, "xhigh": null, "max": null
    });
    assert_eq!(
        model(true, Some(map)).supported_efforts(),
        vec![Effort::Off]
    );
}

#[test]
fn clamp_prefers_the_next_level_up() {
    let sparse = model(
        true,
        Some(json!({"minimal": null, "low": null, "medium": null})),
    );
    assert_eq!(sparse.clamp_effort(Effort::Low), Effort::High);
}

#[test]
fn clamp_falls_back_down_when_nothing_is_higher() {
    let capped = model(true, None);
    assert_eq!(capped.clamp_effort(Effort::Max), Effort::High);
}

#[test]
fn clamp_is_identity_on_an_advertised_level() {
    assert_eq!(
        model(true, None).clamp_effort(Effort::Medium),
        Effort::Medium
    );
}

#[test]
fn parse_round_trips_every_level() {
    for effort in Effort::ALL {
        assert_eq!(Effort::from_str(effort.as_str()), Ok(effort));
    }
}

#[test]
fn parse_rejects_an_unknown_level_by_name() {
    assert_eq!(
        Effort::from_str("extreme"),
        Err(UnknownEffort("extreme".to_owned()))
    );
}

#[test]
fn only_the_top_two_tiers_are_advanced() {
    let advanced: Vec<Effort> = Effort::ALL
        .into_iter()
        .filter(|effort| effort.is_advanced())
        .collect();
    assert_eq!(advanced, vec![Effort::XHigh, Effort::Max]);
}

use serde_json::json;
use yi_types::config::{CustomModelList, CustomProvider};
use yi_types::model::Effort;

#[test]
fn object_reasoning_keeps_the_model_and_reads_as_thinking() -> Result<(), Box<dyn std::error::Error>>
{
    let list: CustomModelList = serde_json::from_value(json!({
        "data": [{"id": "kimi-k2", "reasoning": {"type": "only", "effort": {"valid": ["low"]}}}]
    }))?;
    let [model] = list.data.as_slice() else {
        return Err("the model list lost its entry".into());
    };
    assert_eq!(model.id, "kimi-k2");
    assert_eq!(model.reasoning, Some(true));
    Ok(())
}

#[test]
fn hand_written_level_map_advertises_the_advanced_tiers() -> Result<(), Box<dyn std::error::Error>>
{
    let provider: CustomProvider = serde_json::from_value(json!({
        "baseUrl": "https://example.invalid/v1",
        "models": [{
            "id": "fable",
            "reasoning": true,
            "thinkingLevelMap": {"xhigh": "xhigh", "max": "max"}
        }]
    }))?;
    let models = provider.catalog("fable-provider");
    let [model] = models.as_slice() else {
        return Err("catalog dropped the configured model".into());
    };
    let efforts = model.supported_efforts();
    assert!(efforts.contains(&Effort::XHigh), "xhigh: {efforts:?}");
    assert!(efforts.contains(&Effort::Max), "max missing: {efforts:?}");
    Ok(())
}

/// Dies with a gateway model that keeps the 60 s default because the declared block had no key
/// for it, and a config typing `readTimeoutSecs` refused as an unknown field.
#[test]
fn a_declared_model_carries_its_read_deadline_into_the_catalog()
-> Result<(), Box<dyn std::error::Error>> {
    let provider: CustomProvider = serde_json::from_value(json!({
        "baseUrl": "https://example.invalid/v1",
        "models": [{"id": "slow", "readTimeoutSecs": 600}, {"id": "brisk"}]
    }))?;
    let models = provider.catalog("gw");
    let deadlines: Vec<_> = models.iter().map(|model| model.read_timeout_secs).collect();
    assert_eq!(deadlines, [Some(600), None]);
    Ok(())
}

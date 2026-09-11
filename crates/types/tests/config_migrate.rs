use yi_types::config::UserConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BEFORE: &str = include_str!("fixtures/config-gates-before.json");
const AFTER: &str = include_str!("fixtures/config-gates-after.json");

#[test]
fn the_strict_config_refuses_gates_and_loads_the_migrated_file() -> TestResult {
    serde_json::from_value::<UserConfig>(serde_json::from_str(AFTER)?)?;
    let refused = serde_json::from_value::<UserConfig>(serde_json::from_str(BEFORE)?)
        .err()
        .ok_or("`gates` is an unknown key once the gates are gone")?;
    assert!(refused.to_string().contains("gates"), "{refused}");
    Ok(())
}

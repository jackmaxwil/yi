use serde_json::Value;
use yi_types::config::{ConfigMigration, UserConfig, migrate, parse};

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

/// X7: a config with nothing to migrate is read as strictly as before migrations existed.
#[test]
fn a_config_with_nothing_to_migrate_keeps_the_strict_errors() -> TestResult {
    let twice = parse(r#"{"model":"a","model":"b"}"#)
        .err()
        .ok_or("a key named twice is refused, not read last-wins")?;
    assert!(twice.to_string().contains("duplicate field"), "{twice}");
    let typed = parse(r#"{"tui":{"pace":"fast"}}"#)
        .err()
        .ok_or("a string pace is refused")?;
    assert_eq!((typed.line(), typed.column()), (1, 21), "{typed}");
    Ok(())
}

#[test]
fn a_config_naming_gates_migrates_to_the_current_shape_once() -> TestResult {
    let after: Value = serde_json::from_str(AFTER)?;
    let mut config: Value = serde_json::from_str(BEFORE)?;
    assert_eq!(migrate(&mut config), [ConfigMigration::RemovedGates]);
    assert_eq!(config, after, "before migrates to after");
    assert!(migrate(&mut config).is_empty(), "after migrates to itself");
    assert_eq!(config, after);
    let (loaded, migrations) = parse(BEFORE)?;
    assert_eq!(
        (loaded, migrations),
        (parse(AFTER)?.0, vec![ConfigMigration::RemovedGates])
    );
    assert!(
        ConfigMigration::RemovedGates
            .to_string()
            .contains("`gates`")
    );
    Ok(())
}

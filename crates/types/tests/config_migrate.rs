use serde_json::Value;
use yi_types::config::{UserConfig, migrate, parse};

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

/// A config with nothing to migrate is read as strictly as before migrations existed.
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
fn follow_modes_parse_and_an_unknown_mode_is_refused() -> TestResult {
    for mode in ["slide", "page", "still"] {
        let raw = format!(r#"{{"tui":{{"follow":"{mode}"}}}}"#);
        let (config, _) = parse(&raw)?;
        assert_eq!(
            config.tui.and_then(|tui| tui.follow).map(|_| mode),
            Some(mode)
        );
    }
    let refused = parse(r#"{"tui":{"follow":"instant"}}"#)
        .err()
        .ok_or("an unknown follow mode must fail the strict parse")?;
    assert!(refused.to_string().contains("instant"), "{refused}");
    Ok(())
}

/// The sentence `yi` and `yi doctor` print for the dropped key.
const GATES: &str =
    "the config's `gates` key is ignored: the artifact and closure stop gates are gone; delete it";

#[test]
fn a_config_naming_gates_migrates_to_the_current_shape_once() -> TestResult {
    let after: Value = serde_json::from_str(AFTER)?;
    let mut config: Value = serde_json::from_str(BEFORE)?;
    let applied = migrate(&mut config);
    let said: Vec<String> = applied.iter().map(ToString::to_string).collect();
    assert_eq!(said, [GATES]);
    assert_eq!(config, after, "before migrates to after");
    assert!(migrate(&mut config).is_empty(), "after migrates to itself");
    assert_eq!(config, after);
    let (loaded, migrations) = parse(BEFORE)?;
    assert_eq!(loaded, parse(AFTER)?.0);
    assert_eq!(migrations, applied);
    Ok(())
}

/// A host the proxy could never match must fail at load, naming itself, not ask forever.
#[test]
fn a_sandbox_egress_entry_that_is_no_host_name_is_refused_by_name()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted: UserConfig =
        serde_json::from_str(r#"{"sandbox": {"egress": ["Corp.Example."]}}"#)?;
    let egress = accepted
        .sandbox
        .and_then(|sandbox| sandbox.egress)
        .ok_or("no egress")?;
    assert_eq!(
        egress.first().map(|domain| domain.as_str()),
        Some("corp.example")
    );
    for bad in ["127.0.0.1", "2130706433", "0x7f.1", "[::1]", "a b"] {
        let text = format!(r#"{{"sandbox": {{"egress": ["{bad}"]}}}}"#);
        let refused = serde_json::from_str::<UserConfig>(&text)
            .err()
            .ok_or("taken")?;
        assert!(refused.to_string().contains("not a host name"), "{refused}");
    }
    Ok(())
}

/// Each provider config with exactly one bad part refuses at load and names the key it broke.
#[test]
fn a_provider_typo_refuses_at_load_naming_the_key() -> TestResult {
    let model = r#"{"id":"m1"}"#;
    parse(&format!(
        r#"{{"providers":{{"gw.local":{{"baseUrl":"http://127.0.0.1:1/v1","models":[{model}]}}}}}}"#
    ))?;
    for (base, models, named) in [
        (
            "htp://127.0.0.1:1/v1",
            format!("[{model}]"),
            "providers.gw.local.baseUrl",
        ),
        ("", format!("[{model}]"), "providers.gw.local.baseUrl"),
        (
            "http://127.0.0.1:1/v1",
            "[]".to_owned(),
            "providers.gw.local.models",
        ),
        (
            "http://127.0.0.1:1/v1",
            format!("[{model},{model}]"),
            "providers.gw.local.models",
        ),
        (
            "http://127.0.0.1:1/v1",
            r#"[{"id":""}]"#.to_owned(),
            "providers.gw.local.models",
        ),
    ] {
        let raw =
            format!(r#"{{"providers":{{"gw.local":{{"baseUrl":"{base}","models":{models}}}}}}}"#);
        let refused = parse(&raw).err().ok_or_else(|| format!("loaded: {raw}"))?;
        assert!(refused.to_string().contains(named), "{raw}: {refused}");
    }
    let raw = format!(
        r#"{{"providers":{{"gw.local":{{"baseUrl":"http://h","api":"anthropic","models":[{model}]}}}}}}"#
    );
    let refused = parse(&raw)
        .err()
        .ok_or("an api typo loaded as completions")?;
    assert!(
        refused
            .to_string()
            .contains("expected one of `openai-completions`"),
        "{refused}"
    );
    Ok(())
}

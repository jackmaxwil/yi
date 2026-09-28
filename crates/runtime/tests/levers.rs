use std::cell::Cell;
use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;
use yi_runtime::levers::Levers;
use yi_types::config::RlmConfig;

fn shared(name: &str) -> Result<Value, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/levers")
        .join(name);
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn overrides(name: &str, text: &str) -> Result<PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!("yi-levers-{}-{name}.json", std::process::id()));
    std::fs::write(&path, text)?;
    Ok(path)
}

fn load(path: &Path) -> Result<Levers, String> {
    Levers::load(true, || Some(OsString::from(path)))
}

/// Invariant: Rust and `evals/levers.py` read one fixture, so neither side's defaults drift.
#[test]
fn the_default_fixture_equals_the_compiled_defaults() -> Result<(), Box<dyn Error>> {
    assert_eq!(shared("default.json")?, Levers::DEFAULT.to_value());
    assert_eq!(Levers::default(), Levers::DEFAULT);
    Ok(())
}

/// D280: the three loop guards reach yi-loop through `LoopConfig`, whose defaults are the loop's
/// own constants; an override in an eval run moves exactly the guard it names.
#[test]
fn the_loop_guards_are_the_levers_the_run_read() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        Levers::DEFAULT.loop_guards(),
        yi_loop::LoopGuards::default()
    );
    let path = overrides(
        "guards",
        r#"{"loop.cut_stop_at": 2, "loop.length_stop_at": 1, "loop.reasoning_cap": 9000}"#,
    )?;
    let guards = load(&path)?.loop_guards();
    std::fs::remove_file(&path)?;
    assert_eq!(
        guards,
        yi_loop::LoopGuards {
            length_stop_at: 1,
            cut_stop_at: 2,
            reasoning_cap: 9000
        }
    );
    Ok(())
}

/// Invariant: `family.depth`'s default is the same number an unset `rlm.maxDepth` resolves
/// to, so the inventory names the config's real default rather than a shadow copy of it.
#[test]
fn family_depth_matches_the_unset_rlm_max_depth() {
    assert_eq!(Levers::DEFAULT.family_depth, RlmConfig::default().depth());
}

/// Invariant: the manifest's ranges and tunable marks are the ones the loader enforces.
#[test]
fn the_manifest_matches() -> Result<(), Box<dyn Error>> {
    let manifest = shared("levers.json")?;
    let rows = manifest["levers"].as_array().ok_or("levers is a list")?;
    let listed: Vec<(&str, i64, i64, bool)> = rows
        .iter()
        .filter_map(|row| {
            Some((
                row["name"].as_str()?,
                row["min"].as_i64()?,
                row["max"].as_i64()?,
                row["tunable"].as_bool()?,
            ))
        })
        .collect();
    let compiled: Vec<(&str, i64, i64, bool)> = Levers::SPECS
        .iter()
        .map(|spec| (spec.key, spec.min, spec.max, spec.tunable))
        .collect();
    assert_eq!(listed, compiled);
    Ok(())
}

#[test]
fn without_yi_levers_the_defaults_are_used_and_the_file_is_never_read() {
    assert_eq!(Levers::load(true, || None), Ok(Levers::DEFAULT));
}

/// Invariant: outside an eval run the variable is not even asked for, so a path that
/// would fail to open and bytes that would fail to parse are both inert.
#[test]
fn yi_levers_set_outside_eval_mode_is_ignored() -> Result<(), Box<dyn Error>> {
    let garbage = overrides("garbage", "{not json")?;
    for path in [garbage.clone(), PathBuf::from("/nonexistent/levers.json")] {
        let asked = Cell::new(false);
        let loaded = Levers::load(false, || {
            asked.set(true);
            Some(path.clone().into_os_string())
        });
        assert_eq!(loaded, Ok(Levers::DEFAULT));
        assert!(!asked.get(), "the variable was read outside eval mode");
        assert!(
            load(&path).is_err(),
            "the same path is refused in eval mode"
        );
    }
    std::fs::remove_file(garbage)?;
    Ok(())
}

#[test]
fn an_override_is_applied_whole_or_refused_with_its_reason() -> Result<(), Box<dyn Error>> {
    let good = overrides("good", r#"{"plan.width_max": 4, "route.oneshot_at": -5}"#)?;
    let levers = load(&good)?;
    assert_eq!((levers.plan_width_max, levers.route_oneshot_at), (4, -5));
    assert_eq!(levers.todo_nudge_work, Levers::DEFAULT.todo_nudge_work);
    std::fs::remove_file(good)?;
    for (text, reason) in [
        (r#"{"plan.width_maxx": 4}"#, "unknown lever plan.width_maxx"),
        (r#"{"plan.spawn_cap": 64}"#, "plan.spawn_cap is not tunable"),
        (r#"{"plan.width_max": 17}"#, "integer in 1..=16, not 17"),
        (r#"{"plan.width_max": 0}"#, "integer in 1..=16, not 0"),
        (r#"{"plan.width_max": 4.5}"#, "integer in 1..=16, not 4.5"),
        (r#"{"plan.width_max": "4"}"#, "integer in 1..=16, not \"4\""),
        (r#"{"plan.width_max": null}"#, "integer in 1..=16, not null"),
        (
            r#"{"plan.width_max": 99999999999999999999}"#,
            "integer in 1..=16, not 1e+20",
        ),
        ("[]", "invalid type"),
        ("", "EOF while parsing"),
    ] {
        let path = overrides("refused", text)?;
        let refused = load(&path).err().ok_or(format!("{text} was accepted"))?;
        assert!(
            refused.contains(reason) && refused.starts_with("YI_LEVERS="),
            "{refused}"
        );
        std::fs::remove_file(path)?;
    }
    // A path that is not a readable UTF-8 file is the same refusal, not silent defaults.
    let binary = overrides("binary", "")?;
    std::fs::write(&binary, [0xff, 0xfe])?;
    let refused = load(&binary).err().ok_or("non-UTF-8 bytes were read")?;
    assert!(refused.contains("UTF-8"), "{refused}");
    std::fs::remove_file(binary)?;
    assert!(load(&std::env::temp_dir()).is_err(), "a directory was read");
    Ok(())
}

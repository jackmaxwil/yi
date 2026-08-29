use yi_ai::catalog::Catalog;
use yi_types::model::Effort;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_bundled_model_advertises_at_least_one_level() {
    for model in Catalog::shared().models() {
        assert!(
            !model.supported_efforts().is_empty(),
            "{}/{} advertises nothing",
            model.provider,
            model.id
        );
    }
}

#[test]
fn a_non_reasoning_model_advertises_only_off() {
    for model in Catalog::shared().models().iter().filter(|m| !m.reasoning) {
        assert_eq!(
            model.supported_efforts(),
            vec![Effort::Off],
            "{}/{}",
            model.provider,
            model.id
        );
    }
}

#[test]
fn fable_cannot_disable_thinking_and_reaches_max() -> TestResult {
    let fable = Catalog::shared()
        .get("anthropic", "claude-fable-5")
        .ok_or("bundled catalog missing claude-fable-5")?;
    let supported = fable.supported_efforts();
    assert!(!supported.contains(&Effort::Off));
    assert!(supported.contains(&Effort::Max));
    assert_eq!(fable.clamp_effort(Effort::Off), Effort::Minimal);
    Ok(())
}

#[test]
fn haiku_has_no_map_so_the_advanced_tiers_stay_hidden() -> TestResult {
    let haiku = Catalog::shared()
        .get("anthropic", "claude-haiku-4-5")
        .ok_or("bundled catalog missing claude-haiku-4-5")?;
    assert_eq!(
        haiku.supported_efforts(),
        vec![
            Effort::Off,
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High
        ]
    );
    assert_eq!(haiku.clamp_effort(Effort::Max), Effort::High);
    Ok(())
}

/// The default every session starts on must exist on every bundled model,
/// otherwise the clamp silently moves users off it at startup.
#[test]
fn medium_survives_the_clamp_on_every_reasoning_model() {
    for model in Catalog::shared().models().iter().filter(|m| m.reasoning) {
        let clamped = model.clamp_effort(Effort::Medium);
        assert!(
            model.supported_efforts().contains(&clamped),
            "{}/{} clamped medium to an unadvertised {clamped}",
            model.provider,
            model.id
        );
    }
}

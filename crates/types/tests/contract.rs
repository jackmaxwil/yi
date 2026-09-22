//! F0c, the item list type: `aggregate` is pure, total and monotone, and it is the one
//! place a set of item verdicts becomes an outcome.
//!
//! Test plan. Plan section 6.2 is six numbered rules over one input, evaluated in order:
//! any critical `Fail` is `Fail`; else any `Escalate` is `Escalate`; else any critical
//! `Abstain` is `Abstain`; else the decided set is the `Pass` and `Fail` items and an
//! empty decided set is `Abstain`; else `coverage` is the decided weight over the total
//! weight in permille and below `min_coverage` is `Abstain`; else `score` is the passing
//! weight over the decided weight in permille and `Pass` at or above `threshold`. The
//! order is the specification, not an implementation detail: rule 3 before rule 4 is what
//! keeps a critical item that abstained out of the denominator, and the original ordering
//! let one unavailable critical judge plus one passing item aggregate to a pass (§0 R4).
//! Arithmetic is `u64` with `checked_mul` and a `NonZeroU64` divisor, because a weight
//! set that overflows must refuse rather than wrap into a pass.
//!
//! | test | tier | what it pins | the control it dies with |
//! |---|---|---|---|
//! | `aggregate_never_panics_for_any_item_set` | T0, proptest | Over arbitrary item sets up to the 16 item cap, arbitrary `Weight` in 1..=100, arbitrary verdicts, and `threshold` and `min_coverage` anywhere in 1..=1000, `aggregate` returns an `Outcome` and never panics, divides by zero, or overflows. A duplicate, missing or unknown id invalidates the verdict rather than being counted twice or silently dropped. | The `NonZeroU64` divisor and the `checked_mul`, plus handling the empty decided set at rule 4 before the division rather than after it. The workspace denies `panic`, `unwrap_used` and `expect_used`, so the failure mode this hunts is arithmetic, not an explicit panic: make the divisor a plain `u64` and the empty decided set divides by zero. A `kani` harness over 16 items in its own pinned lane is offered for the same property and is not required. |
//! | `adding_a_critical_fail_never_raises_the_outcome` | T0, proptest | Monotonicity, on the axis that matters: for any item set, adding one critical `Fail` never produces an outcome better than the one before it, on the order `Fail < Abstain < Escalate < Pass` that the refusal path reads. The same holds for turning any item's verdict from `Pass` into `Fail`. A contract cannot be made to pass by adding work that failed. | Rule 1 running before every other rule, and score being a ratio over the decided weight rather than over the whole set. Move the critical-fail check below the threshold arithmetic and a failing critical item with a small weight is outvoted by the rest, which is the shape of every gamed benchmark. |
//! | `critical_abstention_never_aggregates_to_pass` | T0 | One critical item that abstains, beside any number of passing items, aggregates to `Abstain` and never to `Pass`, whatever the weights, the threshold and the coverage floor say. An abstention is the absence of a decision; it is not a soft pass and it does not leave the denominator. | Rule 3 sitting above rule 4, and the decided set being the `Pass` and `Fail` items only. Let a critical abstention fall through to rule 4 and it leaves the denominator, which is exactly how one unavailable judge used to let the rest of a contract carry a pass. This is the regression row of §0 R4 and the one to watch failing before the fix lands. |
//!
//! Not here. `floor_of` is data and is checked by `Plan::validate()` at `init`, `append`,
//! `retry` and `supersede`, so the writer and reader floors are pinned from
//! `plan_ops.rs`, where a plan exists to validate. A `Judge` item's verdict is the jury's
//! tally (`plan/judge.rs`), so by the time it aggregates it is an item verdict like any other.

use std::error::Error;

use proptest::prelude::{Just, Strategy, prop, prop_oneof};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use yi_types::plan::canonical::{ArtifactRef, Digest};
use yi_types::plan::contract::{
    Contract, ContractClass, ContractError, ContractItem, Decider, ItemId, ItemLine, ItemVerdict,
    Outcome, Permille, Weight, aggregate,
};

type TestResult = Result<(), Box<dyn Error>>;

fn artifact(tag: &str) -> ArtifactRef {
    ArtifactRef {
        digest: Digest::of(tag.as_bytes()),
        media_type: "text/plain".to_owned(),
        length: 0,
        provenance: None,
    }
}

fn item(id: &str, critical: bool, weight: u16) -> Result<ContractItem, Box<dyn Error>> {
    Ok(ContractItem {
        id: ItemId::new(id)?,
        critical,
        weight: Weight::new(weight)?,
        decider: Decider::Cmd {
            checker: artifact(id),
            timeout_ms: 1_000,
        },
    })
}

fn fail() -> ItemVerdict {
    ItemVerdict::Fail {
        detail: "exit 1".to_owned(),
    }
}

fn abstain() -> ItemVerdict {
    ItemVerdict::Abstain {
        reason: "no judge".to_owned(),
    }
}

fn escalate() -> ItemVerdict {
    ItemVerdict::Escalate {
        question: "is this in scope".to_owned(),
    }
}

fn permille(value: u16) -> Result<Permille, Box<dyn Error>> {
    Ok(Permille::new(value)?)
}

fn verdict_strategy() -> impl Strategy<Value = ItemVerdict> {
    prop_oneof![
        Just(ItemVerdict::Pass),
        Just(fail()),
        Just(abstain()),
        Just(escalate()),
    ]
}

type Picked = (bool, u16, ItemVerdict);

fn items_strategy() -> impl Strategy<Value = Vec<Picked>> {
    prop::collection::vec(
        (proptest::bool::ANY, 1..=100_u16, verdict_strategy()),
        0..=16,
    )
}

fn build(picks: &[Picked]) -> Result<Vec<(ContractItem, ItemVerdict)>, TestCaseError> {
    picks
        .iter()
        .enumerate()
        .map(|(index, (critical, weight, verdict))| {
            item(&format!("item-{index}"), *critical, *weight)
                .map(|item| (item, verdict.clone()))
                .map_err(|error| TestCaseError::fail(error.to_string()))
        })
        .collect()
}

#[test]
fn aggregate_never_panics_for_any_item_set() -> TestResult {
    let mut runner = TestRunner::new(Config {
        cases: 512,
        ..Config::default()
    });
    runner.run(
        &(items_strategy(), 1..=1000_u16, 1..=1000_u16),
        |(picks, threshold, floor)| {
            let items = build(&picks)?;
            let threshold =
                Permille::new(threshold).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let floor = Permille::new(floor).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let result = aggregate(&items, threshold, floor);
            proptest::prop_assert!(result.score <= 1000 && result.coverage <= 1000);
            if items.is_empty() {
                proptest::prop_assert_eq!(result.outcome, Outcome::Abstain);
            }
            Ok(())
        },
    )?;
    // A duplicate, missing or unknown id invalidates the verdict before aggregation.
    let contract = Contract {
        class: ContractClass::Writer,
        items: vec![item("a", true, 10)?, item("b", false, 10)?],
        threshold: Permille::FULL,
        min_coverage: Permille::FULL,
        covers: Vec::new(),
    };
    let line = |id: &str| -> Result<ItemLine, Box<dyn Error>> {
        Ok(ItemLine {
            id: ItemId::new(id)?,
            verdict: ItemVerdict::Pass,
            jurors: Vec::new(),
        })
    };
    assert!(matches!(
        contract.pair(&[line("a")?]),
        Err(ContractError::MissingItem { .. })
    ));
    assert!(matches!(
        contract.pair(&[line("a")?, line("b")?, line("b")?]),
        Err(ContractError::DuplicateVerdict { .. })
    ));
    assert!(matches!(
        contract.pair(&[line("a")?, line("b")?, line("c")?]),
        Err(ContractError::UnknownItem { .. })
    ));
    assert_eq!(contract.pair(&[line("b")?, line("a")?])?.len(), 2);
    Ok(())
}

#[test]
fn adding_a_critical_fail_never_raises_the_outcome() -> TestResult {
    let mut runner = TestRunner::new(Config {
        cases: 512,
        ..Config::default()
    });
    runner.run(
        &(items_strategy(), 1..=1000_u16, 1..=1000_u16, 1..=100_u16),
        |(picks, threshold, floor, weight)| {
            let items = build(&picks)?;
            let threshold =
                Permille::new(threshold).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let floor = Permille::new(floor).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let before = aggregate(&items, threshold, floor).outcome;
            let mut more = items.clone();
            more.push((
                item("added-critical", true, weight)
                    .map_err(|e| TestCaseError::fail(e.to_string()))?,
                fail(),
            ));
            let after = aggregate(&more, threshold, floor).outcome;
            proptest::prop_assert!(
                after.rank() <= before.rank(),
                "adding a critical fail raised {before} to {after}"
            );
            for index in 0..items.len() {
                let mut flipped = items.clone();
                if let Some((_, verdict)) = flipped.get_mut(index)
                    && *verdict == ItemVerdict::Pass
                {
                    *verdict = fail();
                    let after = aggregate(&flipped, threshold, floor).outcome;
                    proptest::prop_assert!(
                        after.rank() <= before.rank(),
                        "flipping item {index} to fail raised {before} to {after}"
                    );
                }
            }
            Ok(())
        },
    )?;
    Ok(())
}

#[test]
fn critical_abstention_never_aggregates_to_pass() -> TestResult {
    for (weights, threshold, floor) in [
        (vec![1, 100, 100, 100], 1, 1),
        (vec![100, 1], 1000, 1000),
        (vec![50, 50, 50], 500, 500),
    ] {
        let mut items = Vec::new();
        for (index, weight) in weights.iter().enumerate() {
            let verdict = if index == 0 {
                abstain()
            } else {
                ItemVerdict::Pass
            };
            items.push((item(&format!("i{index}"), index == 0, *weight)?, verdict));
        }
        let result = aggregate(&items, permille(threshold)?, permille(floor)?);
        assert_eq!(result.outcome, Outcome::Abstain, "{weights:?}");
    }
    Ok(())
}

#[test]
fn rule_1_a_critical_fail_is_a_fail_before_any_arithmetic() -> TestResult {
    let items = vec![
        (item("tiny", true, 1)?, fail()),
        (item("big", false, 100)?, ItemVerdict::Pass),
        (item("asks", false, 100)?, escalate()),
    ];
    let result = aggregate(&items, permille(1)?, permille(1)?);
    assert_eq!(result.outcome, Outcome::Fail);
    assert_eq!((result.score, result.coverage), (990, 502));
    Ok(())
}

#[test]
fn rule_2_an_escalation_outranks_everything_but_a_critical_fail() -> TestResult {
    let items = vec![
        (item("a", false, 10)?, fail()),
        (item("b", true, 10)?, abstain()),
        (item("c", false, 10)?, escalate()),
    ];
    assert_eq!(
        aggregate(&items, Permille::FULL, Permille::FULL).outcome,
        Outcome::Escalate
    );
    Ok(())
}

#[test]
fn rule_3_a_critical_abstention_abstains_and_a_minor_one_does_not() -> TestResult {
    let critical = vec![
        (item("a", true, 10)?, abstain()),
        (item("b", false, 10)?, ItemVerdict::Pass),
    ];
    assert_eq!(
        aggregate(&critical, permille(1)?, permille(1)?).outcome,
        Outcome::Abstain
    );
    let minor = vec![
        (item("a", false, 10)?, abstain()),
        (item("b", true, 10)?, ItemVerdict::Pass),
    ];
    let result = aggregate(&minor, permille(500)?, permille(500)?);
    assert_eq!(result.outcome, Outcome::Pass);
    assert_eq!((result.score, result.coverage), (1000, 500));
    Ok(())
}

#[test]
fn rule_4_an_empty_decided_set_abstains() -> TestResult {
    assert_eq!(
        aggregate(&[], permille(1)?, permille(1)?).outcome,
        Outcome::Abstain
    );
    let undecided = vec![(item("a", false, 10)?, abstain())];
    let result = aggregate(&undecided, permille(1)?, permille(1)?);
    assert_eq!(result.outcome, Outcome::Abstain);
    assert_eq!((result.score, result.coverage), (0, 0));
    Ok(())
}

#[test]
fn rule_5_coverage_below_the_floor_abstains() -> TestResult {
    let items = vec![
        (item("a", false, 30)?, ItemVerdict::Pass),
        (item("b", false, 70)?, abstain()),
    ];
    let low = aggregate(&items, permille(1)?, permille(301)?);
    assert_eq!((low.outcome, low.coverage), (Outcome::Abstain, 300));
    let met = aggregate(&items, permille(1)?, permille(300)?);
    assert_eq!(met.outcome, Outcome::Pass);
    Ok(())
}

#[test]
fn rule_6_score_meets_the_threshold_or_fails() -> TestResult {
    let items = vec![
        (item("a", false, 60)?, ItemVerdict::Pass),
        (item("b", false, 40)?, fail()),
    ];
    let pass = aggregate(&items, permille(600)?, permille(1)?);
    assert_eq!((pass.outcome, pass.score), (Outcome::Pass, 600));
    let fail = aggregate(&items, permille(601)?, permille(1)?);
    assert_eq!(fail.outcome, Outcome::Fail);
    Ok(())
}

#[test]
fn validate_enforces_the_floors_and_a_judge_never_stands_alone() -> TestResult {
    let schema_only = Contract {
        class: ContractClass::Writer,
        items: vec![ContractItem {
            id: ItemId::new("shape")?,
            critical: true,
            weight: Weight::new(1)?,
            decider: Decider::Schema {
                schema: artifact("schema"),
            },
        }],
        threshold: Permille::FULL,
        min_coverage: Permille::FULL,
        covers: Vec::new(),
    };
    assert!(matches!(
        schema_only.validate(),
        Err(ContractError::Floor {
            class: ContractClass::Writer,
            ..
        })
    ));
    let reader = Contract {
        class: ContractClass::Reader,
        ..schema_only.clone()
    };
    reader.validate()?;
    let inline = Contract {
        class: ContractClass::Inline,
        ..schema_only.clone()
    };
    inline.validate()?;
    let judge = Contract {
        class: ContractClass::Writer,
        items: vec![
            item("cmd", true, 1)?,
            ContractItem {
                id: ItemId::new("taste")?,
                critical: false,
                weight: Weight::new(1)?,
                decider: Decider::Judge {
                    rubric: artifact("rubric"),
                    evidence: Vec::new(),
                    policy: yi_types::plan::contract::JuryPolicy { n: 3 },
                },
            },
        ],
        threshold: Permille::FULL,
        min_coverage: Permille::FULL,
        covers: Vec::new(),
    };
    // Beside a critical behavioural item a live judge is declared; alone it meets no floor,
    // whatever class asks and however critical it says it is.
    judge.validate()?;
    let taste = judge.items.get(1).ok_or("the judge item")?;
    for class in [
        ContractClass::Writer,
        ContractClass::Reader,
        ContractClass::Inline,
    ] {
        let alone = Contract {
            class,
            items: vec![ContractItem {
                critical: true,
                ..taste.clone()
            }],
            ..judge.clone()
        };
        assert!(matches!(alone.validate(), Err(ContractError::Floor { .. })));
    }
    let mut crowd = judge.clone();
    if let Some(ContractItem {
        decider: Decider::Judge { policy, .. },
        ..
    }) = crowd.items.get_mut(1)
    {
        policy.n = 2;
    }
    assert!(matches!(
        crowd.validate(),
        Err(ContractError::JurySize { n: 2, .. })
    ));
    let dup = Contract {
        class: ContractClass::Writer,
        items: vec![item("same", true, 1)?, item("same", false, 1)?],
        threshold: Permille::FULL,
        min_coverage: Permille::FULL,
        covers: Vec::new(),
    };
    assert!(matches!(
        dup.validate(),
        Err(ContractError::DuplicateItem { .. })
    ));
    assert!(Weight::new(0).is_err() && Weight::new(101).is_err());
    assert!(Permille::new(0).is_err() && Permille::new(1001).is_err());
    let wire: Contract = serde_json::from_value(serde_json::json!({
        "class": "writer",
        "items": [{"id": "suite", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": {"digest": Digest::of(b"x").to_string(),
                   "media_type": "application/json", "length": 1}, "timeout_ms": 10}}}]
    }))?;
    assert_eq!(wire.threshold, Permille::FULL);
    assert_eq!(
        serde_json::to_value(ItemVerdict::Pass)?,
        serde_json::json!("pass")
    );
    assert_eq!(
        serde_json::to_value(fail())?,
        serde_json::json!({"fail": {"detail": "exit 1"}})
    );
    Ok(())
}

/// G4: `covers` is new, so a contract frozen before it must digest as it did then.
#[test]
fn a_contract_without_covers_keeps_its_digest() -> TestResult {
    let wire = serde_json::json!({
        "class": "writer",
        "items": [{"id": "suite", "critical": true, "weight": 100,
                   "decider": {"cmd": {"checker": {"digest": Digest::of(b"x").to_string(),
                   "media_type": "application/json", "length": 1}, "timeout_ms": 10}}}]
    });
    let plain: Contract = serde_json::from_value(wire.clone())?;
    assert!(plain.covers.is_empty());
    assert_eq!(
        plain.digest()?.to_string(),
        "sha256:b77f682f47c6fa45260882b688c04249c9bb170d2b7796ecf4923513ea39792a"
    );
    let mut covered = wire;
    covered["covers"] = serde_json::json!(["src/*.rs"]);
    let covered: Contract = serde_json::from_value(covered)?;
    assert_eq!(covered.covers, ["src/*.rs"]);
    assert_ne!(covered.digest()?, plain.digest()?);
    Ok(())
}

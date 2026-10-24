//! The verification share of the lane pool: concurrent worktree acceptances verify side by side
//! up to the share, and one past it waits for a lane rather than being bounced.

use std::error::Error;
use std::time::Duration;

use yi_runtime::plan::capacity::{Capacity, Purpose};
use yi_types::plan::doc::TodoState;

use crate::worktree::{Shape, bench};

type TestResult = Result<(), Box<dyn Error>>;

/// Every check marks itself running and waits, for at most ten seconds, until a peer is running
/// too or it is the third to start, then logs how many checks were running.
fn overlap_check(dir: &std::path::Path) -> String {
    let (runs, started, peak) = (dir.join("runs"), dir.join("started"), dir.join("peak.log"));
    let (runs, started, peak) = (runs.display(), started.display(), peak.display());
    format!(
        "mkdir -p {runs}; touch {runs}/run.$$; echo $$ >> {started}; n=0; \
         while [ $(ls {runs} | grep -c '^run\\.') -lt 2 ] && [ $(wc -l < {started}) -lt 3 ] \
         && [ $n -lt 50 ]; do sleep 0.2; n=$((n+1)); done; \
         ls {runs} | grep -c '^run\\.' >> {peak}; rm {runs}/run.$$"
    )
}

/// Dies with one verification lane per pool: a pool of eight verifies two submits at once, the
/// third waits its turn, and none is bounced. With a share of one no two checks overlap.
#[test]
fn two_acceptances_verify_at_once_and_a_third_waits() -> TestResult {
    let scratch = crate::scratch::Scratch::new("yi-plan-capacity-overlap")?;
    let check = overlap_check(&scratch);
    let labels = ["first", "second", "third"];
    let bench = bench("overlap", &labels, 8, &check, Shape::Unmoved)?;
    let ran: Vec<Result<(), String>> = std::thread::scope(|scope| {
        let runs: Vec<_> = labels
            .iter()
            .map(|label| {
                let bench = &bench;
                scope.spawn(move || {
                    bench
                        .submit(label)
                        .map(drop)
                        .map_err(|e| format!("submit {label}: {e}"))
                })
            })
            .collect();
        runs.into_iter()
            .map(|run| run.join().unwrap_or_else(|_| Err("panicked".to_owned())))
            .collect()
    });
    for (label, outcome) in labels.iter().zip(&ran) {
        assert_eq!(outcome, &Ok(()), "{label}");
        bench.done(label)?;
        assert!(
            matches!(bench.state(label)?, TodoState::Done { .. }),
            "{label}"
        );
    }
    let peak = std::fs::read_to_string(scratch.join("peak.log"))?;
    let most = peak
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .max();
    assert_eq!(most, Some(2), "checks running at once, per run: {peak}");
    assert_eq!(bench.engine.capacity().held(Purpose::Verification), 0);
    Ok(())
}

/// A quarter of the pool verifies, at least one lane and at most four, and workers keep the rest.
#[test]
fn the_share_is_a_quarter_of_the_pool_from_one_to_four() {
    for (slots, share) in [(2, 1), (3, 1), (8, 2), (16, 4), (20, 4), (255, 4)] {
        let capacity = Capacity::for_slots(slots);
        assert_eq!(capacity.cap(Purpose::Verification), share, "{slots} slots");
        assert_eq!(
            capacity.cap(Purpose::Worker),
            slots - share,
            "{slots} slots"
        );
    }
}

/// Dies with the 30 s reserve wait back: a check holding a pool of two's one verification lane
/// for 31 s bounced the next acceptance, whose ended child could not submit again.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn an_acceptance_waits_out_a_check_longer_than_thirty_seconds() -> TestResult {
    let bench = bench("long-wait", &["only"], 2, "true", Shape::Unmoved)?;
    let capacity = bench.engine.capacity();
    let held = capacity.reserve(Purpose::Verification)?;
    let accepted = std::thread::scope(|scope| {
        let accepting = scope.spawn(|| {
            bench.submit("only")?;
            bench.done("only")
        });
        std::thread::sleep(Duration::from_secs(31));
        drop(held);
        accepting.join()
    });
    let accepted = accepted.map_err(|_| "the acceptance panicked")?;
    assert!(accepted.is_ok(), "{accepted:?}");
    assert!(matches!(bench.state("only")?, TodoState::Done { .. }));
    Ok(())
}

/// Dies with the session's deadline ignored by the wait: a submit under a session three seconds
/// from its end waited out the verifier's whole minute for a lane nobody would release.
#[test]
fn a_full_share_refuses_at_the_session_deadline() -> TestResult {
    let left = Duration::from_secs(3);
    let bench = bench("deadline", &["only"], 2, "true", Shape::Unmoved)?
        .until(std::time::Instant::now() + left);
    let _held = bench.engine.capacity().reserve(Purpose::Verification)?;
    let started = std::time::Instant::now();
    let refusal = bench
        .submit("only")
        .err()
        .ok_or("submitted past a held lane")?;
    let waited = started.elapsed();
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("verification capacity is full: 1 of 1 held"),
        "{refusal}"
    );
    assert!(waited >= left - Duration::from_millis(500), "{waited:?}");
    assert!(waited < Duration::from_secs(20), "{waited:?}");
    Ok(())
}

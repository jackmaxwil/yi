//! A worktree acceptance runs its checks once per tree: an integration whose merged tree is the
//! candidate's own takes the candidate's verdict, and one the parent moved is checked again.

use std::error::Error;

use yi_types::plan::doc::TodoState;

use crate::worktree::{Shape, bench};

/// How many times the contract's check ran for one child's submit and done.
fn runs(name: &str, shape: Shape) -> Result<usize, Box<dyn Error>> {
    let scratch = crate::scratch::Scratch::new(&format!("yi-plan-worktree-{name}"))?;
    let log = scratch.join("runs.log");
    let check = format!("echo run >> {}", log.display());
    let bench = bench(name, &["only"], 4, &check, shape)?;
    bench.submit("only")?;
    bench.done("only")?;
    assert!(matches!(bench.state("only")?, TodoState::Done { .. }));
    Ok(std::fs::read_to_string(&log)?.lines().count())
}

/// Dies with the integration check run whatever the tree: a parent that did not move merges to
/// the candidate's own tree, and its check ran a second time on the same bytes.
#[test]
fn an_unmoved_parent_takes_the_candidates_verdict() -> Result<(), Box<dyn Error>> {
    assert_eq!(runs("unmoved", Shape::Unmoved)?, 1);
    Ok(())
}

/// Dies with the verdict reused on a commit match alone: a parent moved by an unrelated commit
/// merges to a tree no check has seen, so the integration is checked.
#[test]
fn a_moved_parent_checks_the_merged_tree() -> Result<(), Box<dyn Error>> {
    assert_eq!(runs("moved", Shape::Moved)?, 2);
    Ok(())
}

/// Dies with the reuse blind to reproducibility: a checker that reads outside the snapshot is
/// no function of the tree, so the same tree is checked again at integration.
#[test]
fn a_check_reading_outside_the_snapshot_runs_again() -> Result<(), Box<dyn Error>> {
    assert_eq!(runs("outside", Shape::ReadsOutside)?, 2);
    Ok(())
}

/// Dies with the reuse blind to the product: the candidate read its output in its own checkout
/// and a parent serving none reads no product, so the integration's verdict is its own.
#[test]
fn an_integration_reading_another_product_runs_again() -> Result<(), Box<dyn Error>> {
    assert_eq!(runs("unserved", Shape::Unserved)?, 2);
    Ok(())
}

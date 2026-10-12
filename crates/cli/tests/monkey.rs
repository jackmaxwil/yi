//! Tier-2 journey: seeded random key runs through both headless TUI loops, so a crash, a hang
//! or a control character in a cell shows as a seed that replays.

use std::error::Error;
use std::process::Command;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// Seeds that played green when they were chosen; a seed that fails is filed on the forge with
/// its script, not dropped from here to turn this green.
const SEEDS: &str = "103";
const RUNS: &str = "5";

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn monkey_seeds_play_without_a_crash_hang_or_control_cell() -> TestResult {
    let dir = Scratch::new("yi-monkey")?;
    let home = dir.home()?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the journey's contract is the monkey runner driving the spawned binary"
    )]
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/tui_see.py"
        ))
        .args(["monkey", "--runs", RUNS, "--seed", SEEDS])
        .args(["--bin", env!("CARGO_BIN_EXE_yi")])
        .arg("--out")
        .arg(dir.join("monkey"))
        .env("HOME", home)
        .output()?;
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a monkey seed failed; its script is in the scratch dir kept under target/tmp:\n{said}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        said.contains(&format!("{RUNS} seeds from {SEEDS}")),
        "the runner did not play the seeds it was given:\n{said}"
    );
    Ok(())
}

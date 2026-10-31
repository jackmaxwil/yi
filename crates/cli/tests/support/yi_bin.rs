//! Incident: 11,633 of the 11,635 checkpoint dirs under the owner's real ~/.yi belonged to test
//! runs that spawned the binary without a HOME of their own. Every spawn starts here.
use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_yi");

pub fn bin() -> &'static Path {
    Path::new(BIN)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the harness under test is the shipped binary; every test spawn goes through here"
)]
pub fn yi_command(home: &Path) -> Command {
    let mut command = Command::new(BIN);
    command.env("HOME", home);
    command
}

/// Invariant: the binary's refusal of an unset HOME is the contract; only that test spawns bare.
#[expect(
    clippy::disallowed_methods,
    reason = "the harness under test is the shipped binary; every test spawn goes through here"
)]
pub fn yi_command_homeless() -> Command {
    let mut command = Command::new(BIN);
    command.env_remove("HOME");
    command
}

#[test]
fn a_spawned_run_reads_the_home_the_harness_pins() -> Result<(), Box<dyn std::error::Error>> {
    let dir = crate::scratch::Scratch::new("yi-bin-home")?;
    let home = dir.home()?;
    let output = yi_command(&home)
        .args(["doctor", "--json"])
        .env("YI_KERNEL_PYTHON", "/nonexistent/python3")
        .output()?;
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let detail = rows
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "home"))
        .and_then(|row| row["detail"].as_str())
        .map(str::to_owned);
    assert_eq!(
        detail.as_deref(),
        Some(home.display().to_string().as_str()),
        "the spawned binary must read the harness's HOME, never the developer's"
    );
    Ok(())
}

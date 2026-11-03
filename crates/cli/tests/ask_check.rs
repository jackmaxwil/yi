//! `yi ask --check` probes a declared check and exits before any provider request: an
//! unrunnable check is refused with the exit a shell would give it, running nothing.

use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

struct Check {
    root: PathBuf,
}

impl Check {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("yi-ask-check-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("project"))?;
        std::fs::create_dir_all(root.join("home/.yi"))?;
        std::fs::write(
            root.join("home/.yi/config.json"),
            r#"{"kernel":{"prewarm":false}}"#,
        )?;
        std::fs::write(root.join("cassette.jsonl"), "")?;
        Ok(Self { root })
    }

    fn run(&self, command: &str) -> Result<std::process::Output, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the contract is the spawned binary's argv, exit code and stderr"
        )]
        let mut process = Command::new(env!("CARGO_BIN_EXE_yi"));
        process
            .arg("ask")
            .arg("--check")
            .arg(command)
            .arg("--model")
            .arg("faux/faux-1")
            .arg("--faux")
            .arg(self.root.join("cassette.jsonl"))
            .arg("--session-dir")
            .arg(self.root.join("home/sessions"))
            .arg("--cwd")
            .arg(self.root.join("project"))
            .env("HOME", self.root.join("home"))
            .current_dir(self.root.join("project"));
        Ok(process.output()?)
    }

    fn sessions(&self) -> usize {
        std::fs::read_dir(self.root.join("home/sessions"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_check_that_cannot_run_exits_2_before_any_request() -> TestResult {
    let check = Check::new("jsut")?;
    let output = check.run("jsut check")?;
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(said.contains("`jsut` is not runnable"), "{said}");
    assert_eq!(check.sessions(), 0, "the probe started a session: {said}");
    Ok(())
}

#[test]
fn a_check_the_shell_cannot_parse_exits_2() -> TestResult {
    let check = Check::new("syntax")?;
    let output = check.run("if then fi")?;
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(said.contains("cannot parse"), "{said}");
    Ok(())
}

#[test]
fn a_check_that_can_run_exits_0_without_a_turn() -> TestResult {
    let check = Check::new("green")?;
    let output = check.run("cd . && true")?;
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert_eq!(check.sessions(), 0, "the probe started a session: {said}");
    Ok(())
}

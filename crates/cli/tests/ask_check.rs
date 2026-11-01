//! `yi ask --check CMD`: the run's own verdict on work it claims to have finished. Each check
//! is probed before any provider request: an unrunnable one exits 2, running nothing.
use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_types::message::StopReason;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

struct Workspace(Scratch);

impl Workspace {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-ask-check-{tag}"))?;
        std::fs::create_dir_all(dir.join("project"))?;
        dir.home()?;
        Ok(Self(dir))
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn yi(&self, args: &[&str]) -> Result<Output, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the surfaces are the spawned binary's argv, exit code and output"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        command
            .args(args)
            .arg("--session-dir")
            .arg(self.0.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project())
            .env("HOME", self.0.join("home"))
            .current_dir(self.project());
        Ok(command.output()?)
    }

    fn ask(&self, script: &str, checks: &[&str]) -> Result<Output, Box<dyn Error>> {
        let mut args = vec![
            "ask",
            "--yolo",
            "--json",
            "--model",
            "faux/faux-1",
            "--faux",
            script,
        ];
        for check in checks {
            args.push("--check");
            args.push(check);
        }
        args.push("go");
        self.yi(&args)
    }
}

/// One scripted turn: a sentence, one command, or a provider error.
enum Turn<'a> {
    Say(&'a str),
    Run(&'a str),
    Fail,
}

fn cassette(workspace: &Workspace, turns: &[Turn<'_>]) -> Result<String, Box<dyn Error>> {
    let mut lines = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        match turn {
            Turn::Say(text) => lines.push(serde_json::to_string(&faux_assistant_message(
                vec![faux_text(text)],
                StopReason::Stop,
            ))?),
            Turn::Run(command) => {
                let args = json!({"command": command})
                    .as_object()
                    .cloned()
                    .ok_or("args")?;
                let call = faux_tool_call(&format!("c{index}"), "bash", args);
                lines.push(serde_json::to_string(&faux_assistant_message(
                    vec![call],
                    StopReason::ToolUse,
                ))?);
                lines.push(serde_json::to_string(&faux_assistant_message(
                    vec![faux_text("ran it")],
                    StopReason::Stop,
                ))?);
            }
            Turn::Fail => lines.push(serde_json::to_string(&faux_assistant_message(
                Vec::new(),
                StopReason::Error,
            ))?),
        }
    }
    let path = workspace.project().join("script.jsonl");
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path.display().to_string())
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn check_lines(output: &Output) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut rows = Vec::new();
    for line in stdout(output).lines() {
        let row: Value = serde_json::from_str(line)?;
        if row["type"] == "check" {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn log_lines(path: &std::path::Path) -> Result<usize, Box<dyn Error>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.lines().count()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(Box::new(error)),
    }
}

/// Dies with a re-prompt that never re-runs the check: a reader-only turn keeps stale evidence
/// and a later fix cannot release green.
#[test]
fn every_re_prompt_re_runs_the_check() -> TestResult {
    let workspace = Workspace::new("wasted")?;
    let log = workspace.project().join("runs.log");
    let script = cassette(
        &workspace,
        &[
            Turn::Say("it is done"),
            Turn::Say("it is still done"),
            Turn::Run("touch done.txt"),
        ],
    )?;
    let check = format!(
        "echo run >> {log}; test -f {done}",
        log = log.display(),
        done = workspace.project().join("done.txt").display()
    );
    let run = workspace.ask(&script, &[&check])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(0), "{out}{err}");
    assert_eq!(log_lines(&log)?, 3, "two red runs, one green: {err}");
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "green", "{out}");
    assert_eq!(last["reprompts"], 2, "{out}");
    assert!(workspace.project().join("done.txt").exists());
    Ok(())
}

/// Dies with a red check that never releases: the run re-prompts past its cap, exits 0 on a
/// check that never passed, or skips the rerun after a turn that called no writing tool.
#[test]
fn a_check_that_stays_red_releases_at_the_cap() -> TestResult {
    let workspace = Workspace::new("cap")?;
    let log = workspace.project().join("runs.log");
    let script = cassette(
        &workspace,
        &[
            Turn::Run("touch one.txt"),
            Turn::Say("it is fixed now"),
            Turn::Run("touch three.txt"),
            Turn::Run("touch four.txt"),
        ],
    )?;
    let first = "true";
    let second = format!(
        "echo run >> {log}; test -f {done}",
        log = log.display(),
        done = workspace.project().join("done.txt").display()
    );
    let run = workspace.ask(&script, &[first, &second])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(4), "{out}{err}");
    assert_eq!(log_lines(&log)?, 4, "four red runs, no fifth: {err}");
    assert!(
        err.contains("cap 3") && err.contains("test -f"),
        "the release row names the red second command: {err}"
    );
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "cap", "{out}");
    assert_eq!(last["reds"], 4, "{out}");
    assert_eq!(last["reprompts"], 3, "{out}");
    Ok(())
}

/// Dies with a failed first turn checked and re-prompted: an ended run must retain its own exit
/// and must not charge a check that cannot repair its error.
#[test]
fn an_ended_first_turn_skips_the_check() -> TestResult {
    let workspace = Workspace::new("ended")?;
    let log = workspace.project().join("runs.log");
    let script = cassette(&workspace, &[Turn::Fail])?;
    let check = format!("echo run >> {} && false", log.display());
    let run = workspace.ask(&script, &[&check])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(1), "{out}{err}");
    assert_eq!(
        log_lines(&log)?,
        0,
        "an ended turn must not run a check: {err}"
    );
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "ended", "{out}");
    Ok(())
}

/// Dies with a check that could not run counted as a pass, or a green first check still
/// re-prompting a model that already finished.
#[test]
fn a_check_that_passes_on_the_first_run_prompts_once() -> TestResult {
    let workspace = Workspace::new("green")?;
    let log = workspace.project().join("runs.log");
    let script = cassette(&workspace, &[Turn::Run("touch done.txt")])?;
    let check = format!(
        "echo run >> {log}; test -f {done}",
        log = log.display(),
        done = workspace.project().join("done.txt").display()
    );
    let run = workspace.ask(&script, &[&check])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(0), "{out}{err}");
    assert_eq!(log_lines(&log)?, 1, "{err}");
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "green", "{out}");
    assert_eq!(last["reprompts"], 0, "{out}");
    Ok(())
}

/// Dies with the check's own ten minutes outliving the run's deadline: `--deadline 2` with a
/// check that never returns would hold the process past the reserve it keeps for shutdown.
#[test]
fn a_check_never_holds_the_run_past_its_deadline() -> TestResult {
    let workspace = Workspace::new("deadline")?;
    let script = cassette(
        &workspace,
        &[Turn::Say("it is done"), Turn::Say("it is done")],
    )?;
    let started = std::time::Instant::now();
    let run = workspace.yi(&[
        "ask",
        "--yolo",
        "--json",
        "--deadline",
        "2",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "sleep 30",
        "go",
    ])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(4), "{out}{err}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the check outlived the deadline"
    );
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "timeout", "{out}{err}");
    Ok(())
}

/// Dies with the check run in the launch checkout: work created by a lane turn is invisible
/// there, so a relative check stays red after the model has fixed it.
#[test]
fn a_lane_ask_checks_the_lane_worktree() -> TestResult {
    let workspace = Workspace::new("lane")?;
    crate::repo::init_repo(&workspace.project())?;
    let script = cassette(&workspace, &[Turn::Run("touch done.txt")])?;
    let run = workspace.yi(&[
        "ask",
        "--yolo",
        "--json",
        "--lanes",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "test -f done.txt",
        "go",
    ])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(0), "{out}{err}");
    let rows = check_lines(&run)?;
    assert_eq!(
        rows.last().map(|row| &row["kind"]),
        Some(&"green".into()),
        "{out}"
    );
    assert!(!workspace.project().join("done.txt").exists());
    Ok(())
}

/// Dies with the check probed in the launch checkout: a script only that checkout holds passes
/// the probe, then exits 127 in the lane and spends every re-prompt on a check that cannot run.
#[test]
fn a_lane_ask_probes_the_check_in_the_lane_worktree() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let workspace = Workspace::new("lane-probe")?;
    crate::repo::init_repo(&workspace.project())?;
    let gate = workspace.project().join("gate.sh");
    std::fs::write(&gate, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o755))?;
    let script = cassette(&workspace, &[Turn::Say("it is done")])?;
    let run = workspace.yi(&[
        "ask",
        "--yolo",
        "--json",
        "--lanes",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "./gate.sh",
        "go",
    ])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(2), "{out}{err}");
    assert!(err.contains("`./gate.sh` is not runnable"), "{err}");
    assert!(check_lines(&run)?.is_empty(), "{out}");
    let sessions = sessions(&workspace.0.join("home/sessions"));
    assert_eq!(sessions, 0, "the refused run recorded a session: {err}");
    let probe = workspace.yi(&[
        "ask",
        "--lanes",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "./gate.sh",
    ])?;
    let said = String::from_utf8_lossy(&probe.stderr);
    assert_eq!(
        probe.status.code(),
        Some(2),
        "the two forms disagree: {said}"
    );
    Ok(())
}

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
        sessions(&self.root.join("home/sessions"))
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sessions(dir: &std::path::Path) -> usize {
    crate::session_files::session_files(dir).map_or(0, |files| files.len())
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

/// Dies with the prompt-less probe printing its refusal on stderr alone: a `--json` harness
/// pre-validating a check reads an empty stdout where the prompted form closes with `final`.
#[test]
fn a_json_probe_without_a_prompt_closes_with_a_final_row() -> TestResult {
    let workspace = Workspace::new("json-probe")?;
    let script = cassette(&workspace, &[Turn::Say("unused")])?;
    let probe = workspace.yi(&[
        "ask",
        "--json",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "jsut check",
    ])?;
    let out = stdout(&probe);
    assert_eq!(probe.status.code(), Some(2), "{out}");
    let last: Value = serde_json::from_str(out.lines().last().unwrap_or_default())?;
    assert_eq!(last["type"], "final", "{out}");
    assert_eq!(last["class"], "refusal:config", "{out}");
    Ok(())
}

/// Dies with the prompt-less probe's lane refusal printed on stderr alone: a `--json` harness
/// reads an empty stdout where the prompted form closes with a `refusal:lane` final row.
#[test]
fn a_json_probe_refused_a_lane_closes_with_a_final_row() -> TestResult {
    let workspace = Workspace::new("json-probe-lane")?;
    crate::repo::init_repo(&workspace.project())?;
    std::fs::write(workspace.0.join("home/.yi/lanes"), "not a directory")?;
    let script = cassette(&workspace, &[Turn::Say("unused")])?;
    let probe = workspace.yi(&[
        "ask",
        "--json",
        "--lanes",
        "--model",
        "faux/faux-1",
        "--faux",
        &script,
        "--check",
        "true",
    ])?;
    let (out, err) = (
        stdout(&probe),
        String::from_utf8_lossy(&probe.stderr).into_owned(),
    );
    assert_eq!(probe.status.code(), Some(1), "{out}{err}");
    let last: Value = serde_json::from_str(out.lines().last().unwrap_or_default())?;
    assert_eq!(last["type"], "final", "{out}");
    assert_eq!(last["class"], "refusal:lane", "{out}");
    Ok(())
}

//! `yi ask --check CMD`: the run's own verdict on work it claims to have finished.
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

/// One scripted turn: a sentence and no work, or one `bash` call and a sentence.
enum Turn<'a> {
    Say(&'a str),
    Run(&'a str),
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

/// Dies with the rerun spent on a turn that changed nothing: the same red check runs three
/// times for one run of work, and the model is re-prompted to fix what it never touched.
#[test]
fn a_re_prompt_that_changed_nothing_spends_no_rerun() -> TestResult {
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
    assert_eq!(log_lines(&log)?, 2, "one red run, one green: {err}");
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "green", "{out}");
    assert_eq!(last["wasted"], 1, "{out}");
    assert_eq!(last["reprompts"], 2, "{out}");
    assert!(workspace.project().join("done.txt").exists());
    Ok(())
}

/// Dies with a red check that never releases: the run re-prompts past its cap, or exits 0 on
/// a check that never passed.
#[test]
fn a_check_that_stays_red_releases_at_the_cap() -> TestResult {
    let workspace = Workspace::new("cap")?;
    let log = workspace.project().join("runs.log");
    let script = cassette(
        &workspace,
        &[
            Turn::Run("touch one.txt"),
            Turn::Run("touch two.txt"),
            Turn::Run("touch three.txt"),
            Turn::Run("touch four.txt"),
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
    assert_eq!(run.status.code(), Some(4), "{out}{err}");
    assert_eq!(log_lines(&log)?, 4, "four red runs, no fifth: {err}");
    assert!(
        err.contains("cap 3") && err.contains("test -f"),
        "the release row names the cap and the command: {err}"
    );
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "cap", "{out}");
    assert_eq!(last["reds"], 4, "{out}");
    assert_eq!(last["reprompts"], 3, "{out}");
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

/// Dies with a command that cannot run read as the model's fault: the run re-prompts it four
/// times and never names the exit the shell gave.
#[test]
fn an_unrunnable_check_is_red_four_times() -> TestResult {
    let workspace = Workspace::new("unrunnable")?;
    let script = cassette(
        &workspace,
        &[
            Turn::Run("touch one.txt"),
            Turn::Run("touch two.txt"),
            Turn::Run("touch three.txt"),
            Turn::Run("touch four.txt"),
        ],
    )?;
    let run = workspace.ask(&script, &["nosuchprog-1528"])?;
    let (out, err) = (
        stdout(&run),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    );
    assert_eq!(run.status.code(), Some(4), "{out}{err}");
    let rows = check_lines(&run)?;
    let last = rows.last().ok_or("no check line")?;
    assert_eq!(last["kind"], "cap", "{out}");
    assert!(
        last["evidence"]
            .as_str()
            .unwrap_or_default()
            .contains("127"),
        "the release names the shell's exit: {out}"
    );
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

//! Tier-2 journeys: the built binary driven end to end over the faux model,
//! offline and keyless. `just journeys` runs them by name; the ordinary suite
//! skips them, because each one costs a process tree per assertion.

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

/// A throwaway project and the home the binary writes sessions and checkpoints
/// into, so a journey never reads or restores the developer's own store.
struct Journey {
    root: PathBuf,
}

impl Journey {
    /// Invariant: the tree outlives the run — a failed journey is read from the
    /// files it left behind, and the next run clears it by name.
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("yi-journey-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("project"))?;
        std::fs::create_dir_all(root.join("home"))?;
        Ok(Self { root })
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn command(&self, args: &[&str]) -> Command {
        #[expect(
            clippy::disallowed_methods,
            reason = "a journey's contract is the spawned binary's argv, exit code and files"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        command
            .args(args)
            .arg("--model")
            .arg("faux/faux-1")
            .arg("--session-dir")
            .arg(self.root.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project())
            .env("HOME", self.root.join("home"))
            .current_dir(self.project())
            // The kernel provisioner narrates on stderr; only stdout is contract.
            .stderr(Stdio::null());
        command
    }

    fn yi(&self, args: &[&str]) -> Result<Output, Box<dyn Error>> {
        Ok(self.command(args).output()?)
    }

    /// One `yi rpc` process fed every frame at once: the journey asserts on the
    /// response stream, and interleaving would make it a timing test.
    fn rpc(&self, frames: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
        let mut child = self
            .command(&["rpc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        {
            let mut stdin = child.stdin.take().ok_or("rpc has no stdin")?;
            for frame in frames {
                serde_json::to_writer(&mut stdin, frame)?;
                stdin.write_all(b"\n")?;
            }
        }
        let output = child.wait_with_output()?;
        let mut responses = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let frame: Value = serde_json::from_str(line)?;
            if frame["type"] == "response" {
                responses.push(frame);
            }
        }
        Ok(responses)
    }
}

fn succeeded(output: &Output, what: &str) -> Result<String, Box<dyn Error>> {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    Err(format!("{what} exited {:?}: {stdout}", output.status.code()).into())
}

fn reply<'a>(responses: &'a [Value], id: &str) -> Result<&'a Value, Box<dyn Error>> {
    responses
        .iter()
        .find(|frame| frame["id"] == id)
        .ok_or_else(|| format!("no response frame for {id}: {responses:?}").into())
}

fn error_of(frame: &Value) -> String {
    frame["error"].as_str().unwrap_or_default().to_owned()
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_resumed_session_keeps_one_file_and_undo_restores_the_turns_start_tree() -> TestResult {
    let journey = Journey::new("session")?;
    let note = journey.project().join("note.txt");

    std::fs::write(&note, "before\n")?;
    succeeded(&journey.yi(&["ask", "first"])?, "the opening ask")?;
    std::fs::write(&note, "after\n")?;
    succeeded(&journey.yi(&["ask", "--continue", "second"])?, "the resume")?;

    let listed: Value = serde_json::from_str(&succeeded(
        &journey.yi(&["sessions", "--json", "list"])?,
        "sessions list",
    )?)?;
    let sessions = listed.as_array().ok_or("sessions list is not an array")?;
    assert_eq!(
        sessions.len(),
        1,
        "a resume must not fork the store: {listed}"
    );
    let id = sessions
        .first()
        .and_then(|entry| entry["id"].as_str())
        .ok_or("the listed session carries no id")?;

    let shown = succeeded(&journey.yi(&["sessions", "show", id])?, "sessions show")?;
    assert!(
        shown.contains("first") && shown.contains("second"),
        "both turns must read back out of the one file: {shown}"
    );

    std::fs::write(&note, "later\n")?;
    let restored = succeeded(&journey.yi(&["undo"])?, "undo")?;
    assert!(
        restored.contains("note.txt"),
        "undo must name the file it put back: {restored}"
    );
    assert_eq!(
        std::fs::read_to_string(&note)?,
        "after\n",
        "undo restores the tree the last turn started from"
    );
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_red_check_refuses_the_done_claim_and_the_completion_gate_holds() -> TestResult {
    let journey = Journey::new("plan")?;
    let responses = journey.rpc(&[
        json!({"id": "create", "type": "plan", "action": "create", "tasks": [
            {"title": "land it", "acceptance": "the check passes", "check": "echo case 9 diverges; exit 2"},
            {"title": "tidy up", "acceptance": "nothing is left over", "check": "true"}
        ]}),
        json!({"id": "red", "type": "plan", "action": "update", "taskId": "t1", "state": "done"}),
        json!({"id": "green", "type": "plan", "action": "update", "taskId": "t2", "state": "done"}),
        json!({"id": "goal", "type": "goal", "action": "create", "objective": "ship it", "check": "echo goal check red; exit 3"}),
        json!({"id": "complete", "type": "goal", "action": "update", "status": "complete"}),
    ])?;

    let created = reply(&responses, "create")?;
    assert_eq!(created["success"], json!(true), "plan.create: {created}");

    let red = reply(&responses, "red")?;
    assert_eq!(
        red["success"],
        json!(false),
        "a red check must refuse the done claim: {red}"
    );
    let refusal = error_of(red);
    assert!(
        refusal.contains("case 9 diverges"),
        "the refusal carries the check's own output: {refusal}"
    );

    let green = reply(&responses, "green")?;
    assert_eq!(
        green["success"],
        json!(true),
        "a green check admits the claim the gate is not blanket: {green}"
    );
    let tasks = green["data"]["plan"]["tasks"]
        .as_array()
        .ok_or("the accepted claim returned no plan")?;
    let states: Vec<&str> = tasks
        .iter()
        .map(|task| task["state"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        states,
        vec!["blocked", "done"],
        "the refused task stays blocked with its evidence: {tasks:?}"
    );

    let complete = reply(&responses, "complete")?;
    assert_eq!(
        complete["success"],
        json!(false),
        "a goal whose check is red must not complete: {complete}"
    );
    let rejection = error_of(complete);
    assert!(
        rejection.contains("goal check red"),
        "the completion refusal names the red check: {rejection}"
    );
    Ok(())
}

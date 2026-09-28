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
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("yi-journey-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("project"))?;
        std::fs::create_dir_all(root.join("home/.yi"))?;
        // Incident: HOME is fresh, so the default kernel prewarm built a 600 MB
        // venv per run under a name no later run could guess; 26 abandoned
        // trees reached 15 GB. Neither journey runs a cell.
        std::fs::write(
            root.join("home/.yi/config.json"),
            r#"{"kernel":{"prewarm":false}}"#,
        )?;
        Ok(Self { root })
    }

    /// Invariant: only a green journey reclaims its tree — a red one is read
    /// from the files it left behind, which is why this is not a [`Drop`].
    fn reclaim(self) {
        let _ = std::fs::remove_dir_all(&self.root);
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

    /// A refusal's contract is its stderr, the one place the binary explains itself; every
    /// other call leaves stderr null because the kernel provisioner narrates there.
    fn refused(&self, args: &[&str]) -> Result<String, Box<dyn Error>> {
        let output = self.command(args).stderr(Stdio::piped()).output()?;
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        if output.status.success() {
            return Err(
                format!("yi {args:?} succeeded; a refusal was the contract: {said}").into(),
            );
        }
        Ok(said)
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
    std::fs::write(&note, "between\n")?;
    let script = write_script(&journey, "note.txt", "after\n")?;
    succeeded(
        &journey.yi(&["ask", "--continue", "--faux", &script, "second"])?,
        "the resume",
    )?;
    assert_eq!(std::fs::read_to_string(&note)?, "after\n");

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

    let restored = succeeded(&journey.yi(&["undo"])?, "undo")?;
    assert!(
        restored.contains("note.txt"),
        "undo must name the file it put back: {restored}"
    );
    assert_eq!(
        std::fs::read_to_string(&note)?,
        "between\n",
        "undo restores the tree the last turn started from, hand edits before it included"
    );
    journey.reclaim();
    Ok(())
}

/// A `--faux` script whose one turn writes `content` to `path`, then says so.
fn write_script(journey: &Journey, path: &str, content: &str) -> Result<String, Box<dyn Error>> {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let args = json!({"path": path, "content": content});
    let call = faux_tool_call("c1", "write", args.as_object().cloned().unwrap_or_default());
    let lines = [
        serde_json::to_string(&faux_assistant_message(vec![call], StopReason::ToolUse))?,
        serde_json::to_string(&faux_assistant_message(
            vec![faux_text("written")],
            StopReason::Stop,
        ))?,
    ];
    let script = journey.project().join("script.jsonl");
    std::fs::write(&script, lines.join("\n"))?;
    Ok(script.display().to_string())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_red_goal_check_refuses_the_completion_claim_and_the_plan_surface_mints_no_principal()
-> TestResult {
    let journey = Journey::new("goalgate")?;
    let responses = journey.rpc(&[
        json!({"id": "goal", "type": "goal", "action": "create", "objective": "ship it", "check": "echo goal check red; exit 3"}),
        json!({"id": "complete", "type": "goal", "action": "update", "status": "complete"}),
        json!({"id": "read", "type": "plan", "action": "get"}),
        json!({"id": "write", "type": "plan", "action": "update", "taskId": "t1", "state": "done"}),
        json!({"id": "claim", "type": "plan", "action": "submit", "actor": "user://1", "op": "init", "goal": "ship it"}),
    ])?;

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

    // Invariant: plan section 5.6. The rpc surface reads and submits and nothing else, and a
    // submit names no principal: `Actor::User` is minted only by the session's confirmation.
    let read = reply(&responses, "read")?;
    assert_eq!(
        read["success"],
        json!(false),
        "no plan is open, so the view says so rather than inventing one: {read}"
    );
    let write = reply(&responses, "write")?;
    assert_eq!(write["success"], json!(false), "{write}");
    assert!(
        error_of(write).contains("get|submit"),
        "a hand edit through rpc is refused and the whole surface is named: {}",
        error_of(write)
    );
    let claim = reply(&responses, "claim")?;
    assert_eq!(claim["success"], json!(false), "{claim}");
    assert!(
        error_of(claim).contains("actor is not an argument"),
        "a submit that names its own actor is refused before any op runs: {}",
        error_of(claim)
    );
    journey.reclaim();
    Ok(())
}

const HAND_WRITTEN_PLAN: &str = r#"---
{
  "format": 1,
  "plan": "ship-the-widget",
  "goal": "Ship the widget end to end",
  "version": 1,
  "touched": 3,
  "tier": "root",
  "state": "active",
  "todos": [
    {"label": "Cut the seam", "state": "done",
     "output": "agent://ship-the-widget/cut-the-seam"},
    {"label": "Wire the adapter", "state": "blocked", "after": ["Cut the seam"],
     "blocked": {"on": {"external": {}}, "note": "the staging deploy has to finish"}},
    {"label": "Write the manpage", "state": "pending",
     "delegation": {"spec": {"role": "writer"}, "accept": {"stated": "the manpage reads well"}}}
  ]
}
---

## Wire the adapter

The adapter cannot land before staging is green.
"#;

/// The plan document driven through the real binary: a format-1 document names its own
/// import (plan section 5.6: a hand-edited view never overwrites the journal), the import
/// opens it, the resolver addresses a todo by slug, the lint reads it, and a `Plan:`
/// trailer resolves back to the goal. The tool itself is model-invoked, so this journey
/// covers every surface around it rather than the tool's own call path.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_hand_written_plan_lints_resolves_and_answers_why() -> TestResult {
    let journey = Journey::new("plandoc")?;
    let plans = journey.project().join(".yi/plans");
    std::fs::create_dir_all(&plans)?;
    std::fs::write(plans.join("ship-the-widget.md"), HAND_WRITTEN_PLAN)?;

    let named = journey.refused(&["plan", "lint", "ship-the-widget"])?;
    assert!(
        named.contains("yi plan import ship-the-widget"),
        "a format-1 document is read-only and the refusal names the way on: {named}"
    );
    let imported = succeeded(
        &journey.yi(&["plan", "import", "ship-the-widget"])?,
        "yi plan import",
    )?;
    assert!(
        imported.contains("ship-the-widget"),
        "the import names the plan it opened: {imported}"
    );

    let linted = succeeded(&journey.yi(&["plan", "lint", "--json"])?, "yi plan lint")?;
    let findings: Value = serde_json::from_str(linted.trim())?;
    let rules: Vec<&str> = findings["findings"]
        .as_array()
        .ok_or("lint returned no findings array")?
        .iter()
        .filter_map(|finding| finding["rule"].as_str())
        .collect();
    assert!(
        rules.contains(&"ephemeral-terminal"),
        "a done output that will not outlive the run is a hard finding: {linted}"
    );
    assert!(
        rules.contains(&"no-probe"),
        "an external block with no probe is a hard finding: {linted}"
    );
    assert!(
        rules.contains(&"unrunnable-acceptance"),
        "a stated acceptance adjudicates nothing: {linted}"
    );

    let fetched = succeeded(
        &journey.yi(&["fetch", "plan://ship-the-widget/wire-the-adapter"])?,
        "yi fetch plan://",
    )?;
    assert!(
        fetched.contains("staging is green"),
        "the todo's body section rides its address: {fetched}"
    );

    let reported = succeeded(
        &journey.yi(&["plan", "report", "--json"])?,
        "yi plan report",
    )?;
    let measured: Value = serde_json::from_str(reported.trim())?;
    assert_eq!(
        measured["plan"],
        json!("ship-the-widget"),
        "the report names the plan even with no op stream: {reported}"
    );

    let project = journey.project();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "journey@example.invalid"],
        vec!["config", "user.name", "Journey"],
        vec!["add", "-A"],
        vec![
            "commit",
            "-q",
            "-m",
            "Wire the adapter through the staging seam\n\nPlan: plan://ship-the-widget/wire-the-adapter",
        ],
    ] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the trailer index is git's own, so the journey has to write a real commit"
        )]
        let status = Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(&args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        assert!(status.success(), "git {args:?} failed");
    }

    let answered = succeeded(
        &journey.yi(&["why", ".yi/plans/ship-the-widget.md:1"])?,
        "yi why <file>:<line>",
    )?;
    assert!(
        answered.contains("Ship the widget end to end"),
        "blame to commit to todo to goal resolves with no inference: {answered}"
    );
    assert!(
        answered.contains("Wire the adapter"),
        "the answer names the todo the trailer pointed at: {answered}"
    );

    let indexed = succeeded(
        &journey.yi(&["why", "ship-the-widget/wire-the-adapter"])?,
        "yi why <plan>/<todo>",
    )?;
    assert!(
        indexed.contains("Wire the adapter through the staging seam"),
        "the reverse index is the same data: {indexed}"
    );

    // Last, because it breaks the plan: a checkpoint that cannot be read is a damaged plan,
    // and an unnamed verb says which one rather than reporting an empty directory.
    std::fs::write(plans.join("ship-the-widget/plan.json"), "{ not json")?;
    let damaged = journey.refused(&["plan", "lint"])?;
    assert!(
        damaged.contains("ship-the-widget"),
        "an unreadable root is named, not counted as absent: {damaged}"
    );
    journey.reclaim();
    Ok(())
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the journey drives the real git the binary drives"
    )]
    let output = Command::new("git").current_dir(dir).args(args).output()?;
    if !output.status.success() {
        return Err(format!("git {} failed", args.join(" ")).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// D119: the ask ran on a slot outside the checkout and handed it back; the trunk never
/// moved, and `--here` is the only way to run without a slot.
#[test]
fn an_ask_in_a_repository_runs_on_a_lane_and_hands_it_back() -> TestResult {
    let journey = Journey::new("lane")?;
    let project = journey.project();
    git_in(&project, &["init", "-q", "-b", "main"])?;
    git_in(&project, &["config", "user.email", "lane@journey"])?;
    git_in(&project, &["config", "user.name", "journey"])?;
    std::fs::write(project.join("README.md"), "trunk\n")?;
    git_in(&project, &["add", "README.md"])?;
    git_in(&project, &["commit", "-qm", "base"])?;
    let head = git_in(&project, &["rev-parse", "HEAD"])?;

    succeeded(&journey.yi(&["ask", "hello"])?, "ask on a lane")?;
    let lanes = succeeded(&journey.yi(&["lanes"])?, "lanes")?;
    assert!(
        lanes.starts_with("lane 0: idle"),
        "the slot came back idle: {lanes}"
    );
    assert!(
        lanes.contains(&head[..12]),
        "the slot sits at the trunk's head: {lanes}"
    );
    assert_eq!(
        git_in(&project, &["symbolic-ref", "--short", "HEAD"])?,
        "main"
    );
    assert_eq!(git_in(&project, &["status", "--porcelain"])?, "");
    assert_eq!(
        git_in(&project, &["branch", "--list", "yi/*"])?,
        "",
        "no branch left behind"
    );
    let worktrees = git_in(&project, &["worktree", "list", "--porcelain"])?;
    assert!(
        worktrees.contains("home/.yi/lanes/"),
        "the slot is a worktree under the home, not the checkout: {worktrees}"
    );

    let here = Journey::new("here")?;
    git_in(&here.project(), &["init", "-q", "-b", "main"])?;
    succeeded(&here.yi(&["ask", "--here", "hello"])?, "ask --here")?;
    let none = succeeded(&here.yi(&["lanes"])?, "lanes after --here")?;
    assert_eq!(none.trim(), "no lanes yet");
    here.reclaim();
    journey.reclaim();
    Ok(())
}

/// An orphaned lane with nothing to keep is what `doctor --fix` reaps; the pool reads idle after.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn doctor_fix_reaps_an_orphaned_lane() -> TestResult {
    let journey = Journey::new("doctor-lane")?;
    let project = journey.project();
    git_in(&project, &["init", "-q", "-b", "main"])?;
    git_in(&project, &["config", "user.email", "lane@journey"])?;
    git_in(&project, &["config", "user.name", "journey"])?;
    std::fs::write(project.join("README.md"), "trunk\n")?;
    git_in(&project, &["add", "README.md"])?;
    git_in(&project, &["commit", "-qm", "base"])?;
    succeeded(&journey.yi(&["ask", "claim once"])?, "ask")?;
    let pool = std::fs::read_dir(journey.root.join("home/.yi/lanes"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_dir())
        .ok_or("no pool dir")?;
    let state_path = pool.join("0.json");
    let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_path)?)?;
    state["session"] = serde_json::Value::String("pid-1".to_owned());
    std::fs::write(&state_path, serde_json::to_vec(&state)?)?;
    git_in(&project, &["branch", "-q", "yi/pid-1", "HEAD"])?;
    git_in(&pool.join("0"), &["checkout", "-q", "yi/pid-1"])?;
    let seen = journey.yi(&["doctor"])?;
    assert_eq!(seen.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&seen.stdout).contains("FAIL  lanes"),
        "{}",
        String::from_utf8_lossy(&seen.stdout)
    );
    let fixed = journey.yi(&["doctor", "--fix"])?;
    let text = String::from_utf8_lossy(&fixed.stdout);
    assert!(text.contains("fixed lanes"), "{text}");
    assert!(String::from_utf8_lossy(&journey.yi(&["lanes"])?.stdout).contains("idle"));
    Ok(())
}

fn write_skill(journey: &Journey, name: &str, frontmatter: &str) -> TestResult {
    let dir = journey.project().join(".yi/skills").join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} method.\n{frontmatter}\n---\nBody.\n"),
    )?;
    Ok(())
}

/// The session file in order: `user`, `assistant`, and each skill pointer as `reminder: <text>`.
fn trace(journey: &Journey) -> Result<Vec<String>, Box<dyn Error>> {
    let sessions = journey.root.join("home/sessions");
    let file = std::fs::read_dir(&sessions)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .flat_map(|files| files.filter_map(Result::ok))
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .ok_or("no session file")?;
    let mut seen = Vec::new();
    for line in std::fs::read_to_string(file)?.lines() {
        let entry: Value = serde_json::from_str(line)?;
        let message = &entry["message"];
        match (message["role"].as_str(), message["customType"].as_str()) {
            (Some("user"), _) => seen.push("user".to_owned()),
            (Some("assistant"), _) => seen.push("assistant".to_owned()),
            (Some("custom"), Some("reminder")) => seen.push(format!(
                "reminder: {}",
                message["content"].as_str().unwrap_or_default()
            )),
            _ => {}
        }
    }
    Ok(seen)
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_typed_dollar_name_points_at_the_skill_before_the_first_reply() -> TestResult {
    let journey = Journey::new("skill-name")?;
    write_skill(&journey, "gate", "trigger: cargo nextest")?;
    write_skill(&journey, "review", "trigger: review the work")?;
    succeeded(
        &journey.yi(&["ask", "$gate run the focused tests"])?,
        "the named ask",
    )?;
    succeeded(
        &journey.yi(&["ask", "--continue", "Review the work"])?,
        "the plain-words ask",
    )?;
    assert_eq!(
        trace(&journey)?,
        [
            "user",
            "reminder: Relevant: skill://gate (matched \"$gate\")",
            "assistant",
            "user",
            "reminder: Relevant: skill://review (matched \"review the work\")",
            "assistant",
        ],
        "each pointer sits between the message that asked for it and the first reply"
    );
    journey.reclaim();
    Ok(())
}

/// #587: a probe session was pointed at skills by its own "verified" and `cargo nextest`.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn the_agents_own_words_fire_no_skill() -> TestResult {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let journey = Journey::new("own-words")?;
    write_skill(&journey, "verify", "trigger: verified")?;
    write_skill(&journey, "gate", "trigger: cargo nextest\nscope: tool:bash")?;
    let call = |id: &str, name: &str, args: Value| {
        faux_tool_call(id, name, args.as_object().cloned().unwrap_or_default())
    };
    let turns = [
        faux_assistant_message(
            vec![
                faux_text("verified the plan; writing the note"),
                call(
                    "c1",
                    "write",
                    json!({"path": "note.txt", "content": "tidy\n"}),
                ),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(
            vec![
                faux_text("verified; running cargo nextest"),
                call("c2", "bash", json!({"command": "echo cargo nextest run"})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("verified, complete")], StopReason::Stop),
    ];
    let lines = turns
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?;
    let script = journey.project().join("script.jsonl");
    std::fs::write(&script, lines.join("\n"))?;
    succeeded(
        &journey.yi(&[
            "ask",
            "--faux",
            &script.display().to_string(),
            "tidy the note",
        ])?,
        "the ask",
    )?;
    assert_eq!(
        std::fs::read_to_string(journey.project().join("note.txt"))?,
        "tidy\n",
        "the script ran: the write and the bash call happened"
    );
    assert_eq!(
        trace(&journey)?,
        ["user", "assistant", "assistant", "assistant"],
        "prose, a command and its output are the agent's own words, and a dropped scope says nothing"
    );
    journey.reclaim();
    Ok(())
}

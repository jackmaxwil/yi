use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::session_files::session_files;
use crate::yi_bin::yi_command;

type TestResult = Result<(), Box<dyn Error>>;

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

    fn reclaim(self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = yi_command(&self.root.join("home"));
        command
            .args(args)
            .arg("--model")
            .arg("faux/faux-1")
            .arg("--session-dir")
            .arg(self.root.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project())
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
        let sessions = self.root.join("home/sessions");
        Ok(
            crate::rpc::run_rpc_in(&self.root, &sessions, &self.project(), frames)?
                .into_iter()
                .filter(|frame| frame["type"] == "response")
                .collect(),
        )
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
    let script = write_script(
        &journey,
        "write",
        json!({"path": "note.txt", "content": "after\n"}),
        "written",
    )?;
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

fn write_script(
    journey: &Journey,
    tool: &str,
    args: serde_json::Value,
    closing: &str,
) -> Result<String, Box<dyn Error>> {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let call = faux_tool_call("c1", tool, args.as_object().cloned().unwrap_or_default());
    let lines = [
        serde_json::to_string(&faux_assistant_message(vec![call], StopReason::ToolUse))?,
        serde_json::to_string(&faux_assistant_message(
            vec![faux_text(closing)],
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

#[test]
fn a_failed_lane_sync_warns_on_stderr_and_the_ask_still_runs() -> TestResult {
    let journey = Journey::new("lane-sync")?;
    let project = trunk(&journey)?;
    std::fs::write(project.join("uv.lock"), "not a lockfile\n")?;
    git_in(&project, &["add", "uv.lock"])?;
    git_in(&project, &["commit", "-qm", "lock"])?;
    let output = journey
        .command(&["ask", "hello"])
        .stderr(Stdio::piped())
        .output()?;
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    succeeded(&output, "ask on an unsynced lane")?;
    assert!(
        said.lines()
            .any(|line| line.starts_with("warning: lane 0: toolchain sync failed")),
        "{said}"
    );
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn doctor_fix_reaps_an_orphaned_lane() -> TestResult {
    let journey = Journey::new("doctor-lane")?;
    let project = trunk(&journey)?;
    succeeded(&journey.yi(&["ask", "claim once"])?, "ask")?;
    let pool = pool_dir(&journey)?;
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

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn doctor_fix_keeps_an_orphan_with_uncommitted_work() -> TestResult {
    let journey = Journey::new("doctor-dirty")?;
    trunk(&journey)?;
    succeeded(&journey.yi(&["ask", "claim once"])?, "ask")?;
    let pool = pool_dir(&journey)?;
    let note = pool.join("0/notes-é.txt");
    std::fs::write(&note, "only copy\n")?;
    for args in [&["doctor"][..], &["doctor", "--fix"]] {
        let seen = journey.yi(args)?;
        let text = String::from_utf8_lossy(&seen.stdout);
        assert!(
            text.lines().any(|row| row.starts_with("ok    lanes")
                && row.ends_with("reap discards the uncommitted paths; `doctor --fix` keeps it")),
            "{args:?}: {text}"
        );
        assert!(note.is_file(), "{args:?} kept the uncommitted file");
    }
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_quit_over_lane_work_says_which_slot_kept_it() -> TestResult {
    let journey = Journey::new("lane-kept")?;
    trunk(&journey)?;
    let script = write_script(
        &journey,
        "write",
        json!({"path": "notes-é.txt", "content": "only copy\n"}),
        "written",
    )?;
    let output = (journey.command(&["ask", "--lanes", "--faux", &script, "write the note"]))
        .stderr(Stdio::piped())
        .output()?;
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{said}");
    assert!(
        said.lines().any(|line| line.starts_with("lane 0 kept: ")
            && line.ends_with("; `yi lanes reap 0` discards them")),
        "{said}"
    );
    assert!(pool_dir(&journey)?.join("0/notes-é.txt").is_file());
    journey.reclaim();
    Ok(())
}

fn trunk(journey: &Journey) -> Result<PathBuf, Box<dyn Error>> {
    let project = journey.project();
    git_in(&project, &["init", "-q", "-b", "main"])?;
    git_in(&project, &["config", "user.email", "lane@journey"])?;
    git_in(&project, &["config", "user.name", "journey"])?;
    std::fs::write(project.join("README.md"), "trunk\n")?;
    git_in(&project, &["add", "README.md"])?;
    git_in(&project, &["commit", "-qm", "base"])?;
    Ok(project)
}

fn pool_dir(journey: &Journey) -> Result<PathBuf, Box<dyn Error>> {
    let found = std::fs::read_dir(journey.root.join("home/.yi/lanes"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_dir());
    Ok(found.ok_or("no pool dir")?)
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

fn trace(journey: &Journey) -> Result<Vec<String>, Box<dyn Error>> {
    let sessions = journey.root.join("home/sessions");
    let file = session_files(&sessions)?
        .into_iter()
        .next()
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
            "reminder: Relevant: yi://skills/gate (matched \"$gate\")",
            "assistant",
            "user",
            "reminder: Relevant: yi://skills/review (matched \"review the work\")",
            "assistant",
        ],
        "each pointer sits between the message that asked for it and the first reply"
    );
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_trigger_word_in_a_repository_without_the_skills_path_points_nowhere() -> TestResult {
    let journey = Journey::new("skill-within")?;
    write_skill(
        &journey,
        "land",
        "trigger: git commit\nwithin: scripts/forge_pr.py",
    )?;
    succeeded(
        &journey.yi(&["ask", "git commit the change"])?,
        "the ask in a repository lacking the path",
    )?;
    assert_eq!(
        trace(&journey)?,
        ["user", "assistant"],
        "no pointer at a recipe for a forge this repository lacks"
    );
    std::fs::create_dir_all(journey.project().join("scripts"))?;
    std::fs::write(journey.project().join("scripts/forge_pr.py"), "")?;
    succeeded(
        &journey.yi(&["ask", "--continue", "git commit the change"])?,
        "the ask once the path exists",
    )?;
    let rows = trace(&journey)?;
    assert!(
        rows.iter()
            .any(|row| row == "reminder: Relevant: yi://skills/land (matched \"git commit\")"),
        "the control: the same words fire the skill where the path exists: {rows:?}"
    );
    journey.reclaim();
    Ok(())
}

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

fn setup(journey: &Journey, answers: &str) -> Result<Output, Box<dyn Error>> {
    setup_with(journey, answers, &[])
}

fn setup_with(
    journey: &Journey,
    answers: &str,
    keys: &[(&str, &str)],
) -> Result<Output, Box<dyn Error>> {
    let mut command = journey.command(&["setup"]);
    for (_, variable) in yi_types::PROVIDER_KEY_VARS {
        command.env_remove(variable);
    }
    command.envs(keys.iter().copied());
    feed(command, answers)
}

fn feed(mut command: Command, answers: &str) -> Result<Output, Box<dyn Error>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("setup has no stdin")?
        .write_all(answers.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn config_of(journey: &Journey) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        journey.root.join("home/.yi/config.json"),
    )?)?)
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_saves_the_answered_steps_and_keeps_every_other_key() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let journey = Journey::new("setup")?;
    let file = journey.root.join("home/.yi/config.json");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    let before = std::fs::read_to_string(&file)?;
    let skipped = setup(&journey, "\n\n")?;
    assert!(skipped.status.success(), "{skipped:?}");
    assert_eq!(
        std::fs::read_to_string(journey.root.join("home/.yi/config.json"))?,
        before,
        "every step skipped writes nothing"
    );
    let saved = setup(&journey, "3\nsk-journey\n2\n")?;
    assert!(saved.status.success(), "{saved:?}");
    let config = config_of(&journey)?;
    assert_eq!(config["model"], "openrouter/anthropic/claude-sonnet-5");
    assert_eq!(config["permissions"]["mode"], "ask");
    assert_eq!(
        config["kernel"]["prewarm"], false,
        "a key setup never asked about survives"
    );
    assert_eq!(
        std::fs::metadata(&file)?.permissions().mode() & 0o777,
        0o600,
        "a config that may hold keys stays private"
    );
    assert!(!journey.root.join("home/.yi/.config.json.setup").exists());
    assert!(config.get("models").is_none(), "no role was set: {config}");
    let asked: Value = serde_json::from_str(&String::from_utf8_lossy(
        &journey.yi(&["gate", "--json", "echo hi"])?.stdout,
    ))?;
    assert_eq!(asked["decision"], "ask", "the saved mode is the run's mode");
    let flagged: Value = serde_json::from_str(&String::from_utf8_lossy(
        &journey.yi(&["gate", "--auto", "--json", "echo hi"])?.stdout,
    ))?;
    assert_eq!(
        flagged["decision"], "allow",
        "a flag still wins over the saved mode"
    );
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_defaults_to_the_credentialed_model() -> TestResult {
    let journey = Journey::new("setup-one-key")?;
    let saved = setup_with(&journey, "\n\n", &[("ANTHROPIC_API_KEY", "sk-journey")])?;
    assert!(saved.status.success(), "{saved:?}");
    let said = String::from_utf8_lossy(&saved.stderr);
    assert!(
        said.contains("Claude Sonnet 5 on Anthropic (key found) [Enter]"),
        "{said}"
    );
    assert_eq!(config_of(&journey)?["model"], "anthropic/claude-sonnet-5");
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_takes_the_key_of_the_provider_picked() -> TestResult {
    let journey = Journey::new("setup-key")?;
    let saved = setup(&journey, "anthropic\nsk-journey\n\n")?;
    assert!(saved.status.success(), "{saved:?}");
    assert_eq!(config_of(&journey)?["model"], "anthropic/claude-sonnet-5");
    let token = journey
        .root
        .join("home/.yi/providers/tokens/anthropic.json");
    assert!(std::fs::read_to_string(token)?.contains("sk-journey"));
    let said = String::from_utf8_lossy(&saved.stderr);
    assert!(
        !said.contains("sk-journey"),
        "the key is never echoed: {said}"
    );
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_does_not_save_a_model_without_its_key() -> TestResult {
    let journey = Journey::new("setup-blank-key")?;
    let saved = setup(&journey, "3\n\n2\n")?;
    assert!(saved.status.success(), "{saved:?}");
    assert!(config_of(&journey)?.get("model").is_none());
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_offers_a_model_for_a_stored_login() -> TestResult {
    let journey = Journey::new("setup-login-found")?;
    let tokens = journey.root.join("home/.yi/providers/tokens");
    std::fs::create_dir_all(&tokens)?;
    std::fs::write(
        tokens.join("openai-codex.json"),
        r#"{"version":1,"kind":"oauth","access":"stored-access","expires_ms":0}"#,
    )?;
    let saved = setup(&journey, "\n\n")?;
    assert!(saved.status.success(), "{saved:?}");
    let said = String::from_utf8_lossy(&saved.stderr);
    assert!(said.contains("(login found)"), "{said}");
    assert_eq!(config_of(&journey)?["model"], "openai-codex/gpt-5.5");
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_with_nothing_chosen_still_records_that_it_ran() -> TestResult {
    let journey = Journey::new("setup-nothing")?;
    let file = journey.root.join("home/.yi/config.json");
    std::fs::remove_file(&file)?;
    let done = setup(&journey, "\n\n")?;
    assert!(done.status.success(), "{done:?}");
    assert_eq!(config_of(&journey)?, serde_json::json!({}));
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_writes_the_model_where_the_config_reads_it() -> TestResult {
    let journey = Journey::new("setup-primary")?;
    std::fs::write(
        journey.root.join("home/.yi/config.json"),
        r#"{"kernel":{"prewarm":false},"models":{"primary":"faux/faux-1"}}"#,
    )?;
    let saved = setup(&journey, "4\nsk-journey\n\n")?;
    assert!(saved.status.success(), "{saved:?}");
    let config = config_of(&journey)?;
    assert_eq!(
        config["models"]["primary"],
        "openrouter/anthropic/claude-sonnet-5"
    );
    assert!(config.get("model").is_none(), "{config}");
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_refuses_a_bad_config_before_asking_anything() -> TestResult {
    let journey = Journey::new("setup-bad")?;
    let file = journey.root.join("home/.yi/config.json");
    std::fs::write(&file, r#"{"modle":"x"}"#)?;
    let refused = setup(&journey, "3\n2\n")?;
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(said.contains("fix it, then run `yi setup` again"), "{said}");
    assert!(!said.contains("Which AI"), "nothing was asked: {said}");
    assert_eq!(std::fs::read_to_string(&file)?, r#"{"modle":"x"}"#);
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn setup_writes_through_a_dangling_config_link() -> TestResult {
    let journey = Journey::new("setup-link")?;
    let file = journey.root.join("home/.yi/config.json");
    std::fs::remove_file(&file)?;
    std::os::unix::fs::symlink("../dotfiles/yi.json", &file)?;
    let saved = setup(&journey, "\n2\n")?;
    assert!(saved.status.success(), "{saved:?}");
    assert!(std::fs::symlink_metadata(&file)?.file_type().is_symlink());
    let config: Value = serde_json::from_str(&std::fs::read_to_string(
        journey.root.join("home/dotfiles/yi.json"),
    )?)?;
    assert_eq!(config["permissions"]["mode"], "ask");
    journey.reclaim();
    Ok(())
}

/// The background sweep a session start triggers is the cleanup a user never runs by hand;
/// the journey reads the home the binary was given, not the developer's.
#[expect(
    clippy::disallowed_methods,
    reason = "the orphan's age is real: the sweep judges real mtimes"
)]
fn aged(days: u64) -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600)
}

#[ignore = "tier-2 journey: `just journeys`"]
#[test]
fn a_session_start_sweeps_an_aged_orphan_without_yi_gc() -> TestResult {
    let journey = Journey::new("retention")?;
    let orphan = journey.root.join("home/.yi/checkpoints/oldorphan");
    std::fs::create_dir_all(orphan.join("objects"))?;
    std::fs::write(
        orphan.join("config"),
        format!(
            "[core]\n\tworktree = {}\n",
            journey.project().join("gone").display()
        ),
    )?;
    std::fs::File::open(&orphan)?.set_modified(aged(8))?;
    succeeded(
        &journey.yi(&["ask", "hello"])?,
        "ask on a home with an aged orphan",
    )?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while orphan.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !orphan.exists(),
        "the orphan a session start was meant to sweep stayed"
    );
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn an_exiting_ask_kills_its_background_bash_process_group() -> TestResult {
    let journey = Journey::new("exit-bash-job")?;
    let group = journey.project().join("job-pgid");
    let script = write_script(
        &journey,
        "bash",
        json!({"command": "echo $$ > job-pgid; exec sleep 300", "wait": 5}),
        "done",
    )?;
    succeeded(
        &journey.yi(&["ask", "--yolo", "--faux", &script, "start the command"])?,
        "headless ask",
    )?;
    let group = std::fs::read_to_string(group)?.trim().to_owned();
    #[expect(
        clippy::disallowed_methods,
        reason = "the probe asks the OS whether the job's process group outlived yi"
    )]
    let alive = Command::new("/bin/sh")
        .args(["-c", "kill -0 \"-$1\" 2>/dev/null", "sh", &group])
        .status()?
        .success();
    if alive {
        #[expect(
            clippy::disallowed_methods,
            reason = "a red probe leaves the machine as it found it"
        )]
        let _stopped = Command::new("/bin/sh")
            .args(["-c", "kill -KILL \"-$1\" 2>/dev/null", "sh", &group])
            .status();
        return Err(format!("background bash process group {group} remained alive").into());
    }
    journey.reclaim();
    Ok(())
}

const SEARCH_RESPONSE: &str =
    include_str!("../../ai/tests/fixtures/openrouter-web-search-deepseek-q0.response.json");

fn search_case(primary: &str, explicit_role: bool) -> TestResult {
    use std::io::BufReader;
    use std::net::TcpListener;
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;

    let journey = Journey::new("web-search")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let served = std::thread::spawn(move || -> std::io::Result<String> {
        let (stream, _) = listener.accept()?;
        let mut reader = BufReader::new(stream);
        let request = crate::loopback::read_request(&mut reader);
        write!(
            reader.into_inner(),
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{SEARCH_RESPONSE}",
            SEARCH_RESPONSE.len()
        )?;
        Ok(request)
    });
    let search_id = if explicit_role {
        "openai/gpt-5-nano"
    } else {
        "deepseek/deepseek-v4-flash"
    };
    let mut model = yi_runtime::resolve_model("openrouter", search_id)
        .ok_or("the bundled search model is missing")?;
    model.base_url = format!("http://127.0.0.1:{port}/api/v1");
    let mut renamed = model.clone();
    renamed.id = "deepseek/deepseek-v4-flash-search".to_owned();
    std::fs::create_dir_all(journey.root.join("home/.yi/catalog"))?;
    std::fs::write(
        journey.root.join("home/.yi/catalog/openrouter.json"),
        serde_json::to_string(&json!({
            "schema": 2,
            "openai-completions": {
                model.id.clone(): serde_json::to_value(model)?,
                renamed.id.clone(): serde_json::to_value(renamed)?,
            }
        }))?,
    )?;
    let mut config = json!({"kernel": {"prewarm": false}});
    if explicit_role {
        config["models"] = json!({"webSearch": "openrouter/deepseek/deepseek-v4-flash-search"});
        config["webSearch"] = json!({"engine": "non-default-engine"});
    }
    std::fs::write(
        journey.root.join("home/.yi/config.json"),
        serde_json::to_string(&config)?,
    )?;
    let call = faux_tool_call(
        "search-1",
        "web_search",
        json!({"query": "latest Rust", "count": 5})
            .as_object()
            .cloned()
            .unwrap_or_default(),
    );
    let script = journey.project().join("search.jsonl");
    std::fs::write(
        &script,
        [
            serde_json::to_string(&faux_assistant_message(vec![call], StopReason::ToolUse))?,
            serde_json::to_string(&faux_assistant_message(
                vec![faux_text("done")],
                StopReason::Stop,
            ))?,
        ]
        .join("\n"),
    )?;
    let output = journey
        .command(&[
            "ask",
            "--json",
            "--faux",
            &script.display().to_string(),
            "search",
        ])
        .arg("--model")
        .arg(primary)
        .env("OPENROUTER_API_KEY", "x")
        .stderr(Stdio::piped())
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let shown = succeeded(&output, "web search ask")?;
    assert!(shown.contains("<<<yi-external"), "{shown}\n{stderr}");
    assert!(
        shown.contains("https://doc.rust-lang.org/stable/releases.html"),
        "{shown}"
    );
    let stored = crate::session_files::session_text(&journey.root.join("home/sessions"))?;
    assert_eq!(
        stored.matches("\"cause\":\"tool:web_search\"").count(),
        1,
        "{stored}"
    );
    assert!(stored.contains("\"toolName\":\"web_search\""), "{stored}");
    assert!(stored.contains("0.007516407500000001"), "{stored}");

    let request = served.join().map_err(|_| "search server panicked")??;
    let body = crate::loopback::json_body(&request)?;
    let (model_id, engine) = if explicit_role {
        ("deepseek/deepseek-v4-flash-search", "non-default-engine")
    } else {
        ("deepseek/deepseek-v4-flash", "exa")
    };
    assert_eq!(body["model"], model_id);
    assert_eq!(body["stream"], false);
    assert_eq!(body["tools"][0]["type"], "openrouter:web_search");
    let parameters = &body["tools"][0]["parameters"];
    assert_eq!(parameters["engine"], engine);
    assert_eq!(parameters["mode"], "auto");
    assert_eq!(parameters["max_results"], 5);
    assert_eq!(parameters["max_total_results"], 5);
    assert_eq!(parameters["max_uses"], 1);
    journey.reclaim();
    Ok(())
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn web_search_offline_citations_and_cost() -> TestResult {
    search_case("openrouter/deepseek/deepseek-v4-flash", false)?;
    search_case("faux/faux-1", true)
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn invalid_web_search_config_is_refused_with_an_actionable_reason() -> TestResult {
    for (name, config, setting, fix) in [
        (
            "model",
            json!({"kernel": {"prewarm": false}, "models": {"webSearch": "faux/faux-1"}}),
            "models.webSearch",
            "choose an OpenRouter model or remove the setting",
        ),
        (
            "engine",
            json!({"kernel": {"prewarm": false}, "webSearch": {"engine": ""}}),
            "webSearch.engine",
            "choose a non-empty engine or remove the setting",
        ),
    ] {
        let journey = Journey::new(&format!("web-search-invalid-{name}"))?;
        std::fs::write(
            journey.root.join("home/.yi/config.json"),
            serde_json::to_string(&config)?,
        )?;
        let output = journey
            .command(&["ask", "hello"])
            .env("OPENROUTER_API_KEY", "x")
            .stderr(Stdio::piped())
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{name}: {stderr}");
        assert_eq!(stderr.matches(setting).count(), 1, "{name}: {stderr}");
        assert!(stderr.contains(fix), "{name}: {stderr}");
        journey.reclaim();
    }
    Ok(())
}

/// The real binary stops a message at its first failed write or command: an edit refused for a
/// file never read, then a command that exits non-zero, each leave the marker-writing command
/// queued behind them unrun, answered `not_run`, so no result reports on a change that never
/// landed. A marker file is the proof, as the wall's deny was proven.
#[test]
fn a_failed_edit_leaves_its_test_unrun() -> TestResult {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let journey = Journey::new("stop-at-first-failure")?;
    std::fs::write(journey.project().join("notes.txt"), "alpha\nbravo\n")?;
    let first = [
        (
            "e1",
            "edit",
            json!({"patch": "[notes.txt]\nPUT 2.=2:\n+BRAVO"}),
        ),
        ("b1", "bash", json!({"command": "touch ran-after-edit"})),
    ];
    let second = [
        ("b2", "bash", json!({"command": "exit 3"})),
        ("b3", "bash", json!({"command": "touch ran-after-exit"})),
    ];
    for (n, calls) in [first, second].into_iter().enumerate() {
        let blocks = calls
            .iter()
            .map(|(id, tool, args)| {
                faux_tool_call(id, tool, args.as_object().cloned().unwrap_or_default())
            })
            .collect();
        let lines = [
            serde_json::to_string(&faux_assistant_message(blocks, StopReason::ToolUse))?,
            serde_json::to_string(&faux_assistant_message(
                vec![faux_text("done")],
                StopReason::Stop,
            ))?,
        ];
        let script = journey.project().join(format!("script-{n}.jsonl"));
        std::fs::write(&script, lines.join("\n"))?;
        let script = script.display().to_string();
        let stdout = succeeded(
            &journey.yi(&[
                "ask",
                "--yolo",
                "--json",
                "--faux",
                &script,
                "edit then check",
            ])?,
            "headless ask",
        )?;
        let skipped = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event["type"] == "tool_execution_end")
            .filter(|event| event["result"]["details"]["errorKind"] == "not_run")
            .count();
        if skipped != 1 {
            return Err(format!("run {n}: {skipped} not_run results, wanted 1:\n{stdout}").into());
        }
    }
    for marker in ["ran-after-edit", "ran-after-exit"] {
        if journey.project().join(marker).exists() {
            return Err(format!("{marker} exists: the call behind a failure ran").into());
        }
    }
    journey.reclaim();
    Ok(())
}

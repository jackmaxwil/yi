use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

struct Workspace(PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Workspace {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = std::env::temp_dir().join(format!("yi-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("project"))?;
        std::fs::create_dir_all(dir.join("home"))?;
        Ok(Self(dir))
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn yi(&self, args: &[&str]) -> Result<Output, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "these surfaces are the spawned binary's argv, exit code, and stdout"
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
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn ask(workspace: &Workspace, prompt: &str, extra: &[&str]) -> Result<Output, Box<dyn Error>> {
    let mut args = vec!["ask", "--model", "faux/faux-1"];
    args.extend_from_slice(extra);
    args.push(prompt);
    workspace.yi(&args)
}

#[test]
fn continue_resumes_the_leaf_session() -> TestResult {
    let workspace = Workspace::new("continue")?;
    ask(&workspace, "first", &[])?;
    ask(&workspace, "second", &["--continue"])?;

    let listed: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    let sessions = listed.as_array().ok_or("list is not an array")?;
    assert_eq!(sessions.len(), 1, "--continue must not open a second file");

    let id = sessions
        .first()
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_str)
        .ok_or("no session id")?;
    let shown = stdout(&workspace.yi(&["sessions", "show", id])?);
    assert!(shown.contains("first"), "{shown}");
    assert!(shown.contains("second"), "{shown}");
    Ok(())
}

#[test]
fn fresh_ask_starts_its_own_session() -> TestResult {
    let workspace = Workspace::new("fresh")?;
    ask(&workspace, "first", &[])?;
    ask(&workspace, "second", &[])?;
    let listed: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    assert_eq!(listed.as_array().map(Vec::len), Some(2));
    Ok(())
}

#[test]
fn sessions_rm_removes_the_session() -> TestResult {
    let workspace = Workspace::new("rm")?;
    ask(&workspace, "only", &[])?;
    let listed: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    let id = listed
        .get(0)
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_str)
        .ok_or("no session id")?
        .to_owned();
    workspace.yi(&["sessions", "rm", &id])?;
    let after: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    assert_eq!(after.as_array().map(Vec::len), Some(0));
    Ok(())
}

#[test]
fn schema_validates_the_answer() -> TestResult {
    let workspace = Workspace::new("schema")?;
    let schema = r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#;

    // The faux provider echoes the prompt, so the prompt is the answer.
    let matching = ask(&workspace, r#"{"name":"yi"}"#, &["--schema", schema])?;
    assert_eq!(matching.status.code(), Some(0));
    let answer: Value = serde_json::from_str(stdout(&matching).trim())?;
    assert_eq!(answer.get("name").and_then(Value::as_str), Some("yi"));

    let mismatched = ask(&workspace, r#"{"other":1}"#, &["--schema", schema])?;
    assert_eq!(mismatched.status.code(), Some(3));
    assert!(stdout(&mismatched).trim().is_empty(), "no prose on failure");

    let prose = ask(&workspace, "not json at all", &["--schema", schema])?;
    assert_eq!(prose.status.code(), Some(3));
    Ok(())
}

#[test]
fn undo_restores_the_files_a_turn_changed() -> TestResult {
    let workspace = Workspace::new("undo")?;
    let kept = workspace.project().join("kept.txt");
    std::fs::write(&kept, "before\n")?;
    ask(&workspace, "start a turn", &[])?;

    std::fs::write(&kept, "after\n")?;
    std::fs::write(workspace.project().join("created.txt"), "new\n")?;

    let undone = workspace.yi(&["undo"])?;
    if git_missing(&undone) {
        return Ok(());
    }
    assert_eq!(undone.status.code(), Some(0), "{}", stdout(&undone));
    assert_eq!(std::fs::read_to_string(&kept)?, "before\n");
    assert!(!workspace.project().join("created.txt").exists());

    workspace.yi(&["undo"])?;
    assert_eq!(std::fs::read_to_string(&kept)?, "after\n");
    Ok(())
}

#[test]
fn undo_without_a_session_fails_loudly() -> TestResult {
    let workspace = Workspace::new("undo-empty")?;
    let output = workspace.yi(&["undo"])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no session"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Checkpoints are a no-op without git (design 5.3), and so is this test.
fn git_missing(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("git is unavailable")
}

#[test]
fn resuming_the_tui_replays_the_transcript() -> TestResult {
    let workspace = Workspace::new("tui-resume")?;
    ask(&workspace, "remembered prompt", &[])?;
    let listed: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    let id = listed
        .get(0)
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_str)
        .ok_or("no session id")?
        .to_owned();

    let keys = workspace.0.join("keys");
    std::fs::write(&keys, "wait 200\nkey ctrl-c\nkey ctrl-c\n")?;
    let frames = workspace.0.join("frames");
    let resumed = workspace.yi(&[
        "tui",
        "--headless",
        "--model",
        "faux/faux-1",
        "--session",
        &id,
        "--keys",
        &keys.display().to_string(),
        "--frames",
        &frames.display().to_string(),
    ])?;
    let first = std::fs::read_to_string(frames.join("0000.txt"))?;
    assert!(first.contains("remembered prompt"), "{first}");
    assert!(
        String::from_utf8_lossy(&resumed.stderr).contains(&format!("yi --session {id}")),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    Ok(())
}

#[test]
fn a_turn_records_both_checkpoints() -> TestResult {
    let workspace = Workspace::new("checkpoints")?;
    std::fs::write(workspace.project().join("kept.txt"), "before\n")?;
    ask(&workspace, "start a turn", &[])?;

    let listed = workspace.yi(&["undo", "--json", "list"])?;
    let parsed: Value = serde_json::from_str(stdout(&listed).trim())?;
    let recorded = parsed
        .get("checkpoints")
        .and_then(Value::as_array)
        .ok_or("no checkpoints array")?;
    if recorded.is_empty() {
        return Ok(());
    }
    let moments: Vec<&str> = recorded
        .iter()
        .filter_map(|entry| entry.get("at").and_then(Value::as_str))
        .collect();
    assert!(moments.contains(&"turnStart"), "{moments:?}");
    assert!(moments.contains(&"turnEnd"), "{moments:?}");
    Ok(())
}

fn write_config(workspace: &Workspace, config: &str) -> TestResult {
    let dir = workspace.0.join("home/.yi");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("config.json"), config)?;
    Ok(())
}

#[test]
fn the_primary_role_supplies_the_model() -> TestResult {
    let workspace = Workspace::new("role-primary")?;
    write_config(&workspace, r#"{"models":{"primary":"faux/faux-1"}}"#)?;
    let answered = workspace.yi(&["ask", "role check"])?;
    assert_eq!(
        answered.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    assert!(
        stdout(&answered).contains("faux: role check"),
        "{}",
        stdout(&answered)
    );
    Ok(())
}

#[test]
fn an_unknown_summarizer_role_warns_and_keeps_the_primary() -> TestResult {
    let workspace = Workspace::new("role-summarizer")?;
    write_config(
        &workspace,
        r#"{"models":{"primary":"faux/faux-1","summarizer":"nope/nope-1"}}"#,
    )?;
    let answered = workspace.yi(&["ask", "role check"])?;
    assert_eq!(answered.status.code(), Some(0));
    let complaint = String::from_utf8_lossy(&answered.stderr);
    assert!(
        complaint.contains("unknown summarizer model nope/nope-1"),
        "{complaint}"
    );
    assert!(
        stdout(&answered).contains("faux: role check"),
        "{}",
        stdout(&answered)
    );
    Ok(())
}

#[test]
fn a_misspelled_config_key_fails_naming_it() -> TestResult {
    let workspace = Workspace::new("config-strict")?;
    write_config(
        &workspace,
        r#"{"models":{"primary":"faux/faux-1"},"modle":"x"}"#,
    )?;
    let answered = workspace.yi(&["ask", "strict check"])?;
    assert_eq!(answered.status.code(), Some(2));
    let complaint = String::from_utf8_lossy(&answered.stderr);
    assert!(complaint.contains("config.json"), "{complaint}");
    assert!(complaint.contains("modle"), "{complaint}");
    Ok(())
}

#[test]
fn a_broken_config_file_fails_instead_of_reading_as_absent() -> TestResult {
    let workspace = Workspace::new("config-broken")?;
    write_config(&workspace, r#"{"models":{"primary":"faux/faux-1",}"#)?;
    let answered = workspace.yi(&["ask", "broken check"])?;
    assert_eq!(answered.status.code(), Some(2));
    let complaint = String::from_utf8_lossy(&answered.stderr);
    assert!(complaint.contains("config.json"), "{complaint}");
    Ok(())
}

/// `yi gate` is the dry run: the same decision the tool seam would make, with
/// the exit code carrying it for a script and `--json` for a reader.
#[test]
fn gate_answers_with_the_decision_and_the_exit_code() -> TestResult {
    let workspace = Workspace::new("gate")?;
    let allowed = workspace.yi(&["gate", "cargo check && git status"])?;
    assert_eq!(allowed.status.code(), Some(0));
    assert!(
        stdout(&allowed).starts_with("auto: allow"),
        "{}",
        stdout(&allowed)
    );

    let asked = workspace.yi(&["gate", "rm -rf target"])?;
    assert_eq!(asked.status.code(), Some(1));
    assert!(
        stdout(&asked).starts_with("auto: ask"),
        "{}",
        stdout(&asked)
    );

    let denied = workspace.yi(&["gate", "rm -rf /"])?;
    assert_eq!(denied.status.code(), Some(1));
    assert!(
        stdout(&denied).starts_with("auto: deny"),
        "{}",
        stdout(&denied)
    );

    let usage = workspace.yi(&["gate"])?;
    assert_eq!(usage.status.code(), Some(2));
    Ok(())
}

#[test]
fn gate_json_names_every_segment_and_its_class() -> TestResult {
    let workspace = Workspace::new("gate-json")?;
    let out = workspace.yi(&["gate", "--json", "cargo test && rm -rf target"])?;
    let report: Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(report["decision"], "ask");
    assert_eq!(report["mode"], "auto");
    assert_eq!(report["unparsed"], false);
    let segments = report["segments"].as_array().ok_or("segments")?;
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0]["class"], "safe");
    assert_eq!(segments[1]["class"], "destructive");
    assert_eq!(segments[1]["argv"][0], "rm");
    Ok(())
}

#[test]
fn gate_reads_the_mode_it_is_asked_about() -> TestResult {
    let workspace = Workspace::new("gate-mode")?;
    let yolo = workspace.yi(&["gate", "--yolo", "--json", "rm -rf node_modules"])?;
    let report: Value = serde_json::from_str(&stdout(&yolo))?;
    assert_eq!(report["decision"], "allow");
    assert_eq!(report["mode"], "yolo");

    let confirm = workspace.yi(&["gate", "--confirm", "--json", "ls -la"])?;
    let report: Value = serde_json::from_str(&stdout(&confirm))?;
    assert_eq!(report["decision"], "ask", "ask mode asks for every command");

    let yolo_catastrophic = workspace.yi(&["gate", "--yolo", "--json", "rm -rf /"])?;
    let report: Value = serde_json::from_str(&stdout(&yolo_catastrophic))?;
    assert_eq!(report["decision"], "deny", "yolo is not above the denylist");
    Ok(())
}

/// A key read into the transcript has already left the machine, so reading one
/// asks even though `cat` is otherwise a proven-safe verb.
#[test]
fn gate_asks_before_a_credential_is_read() -> TestResult {
    let workspace = Workspace::new("gate-credentials")?;
    let key = workspace.0.join("home/.ssh/id_rsa");
    let ordinary = workspace.0.join("home/notes.md");
    let asked = workspace.yi(&["gate", &format!("cat {}", key.display())])?;
    assert_eq!(asked.status.code(), Some(1), "{}", stdout(&asked));
    assert!(
        stdout(&asked).contains("credential store"),
        "{}",
        stdout(&asked)
    );

    let allowed = workspace.yi(&["gate", &format!("cat {}", ordinary.display())])?;
    assert_eq!(allowed.status.code(), Some(0), "{}", stdout(&allowed));
    Ok(())
}

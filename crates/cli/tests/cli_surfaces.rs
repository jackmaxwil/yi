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
        self.yi_env(args, &[])
    }

    fn yi_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Output, Box<dyn Error>> {
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
            .envs(env.iter().copied())
            .current_dir(self.project());
        Ok(command.output()?)
    }
}

impl Workspace {
    /// One `yi acp` process fed every frame at once; the contract is the response stream.
    fn acp(&self, args: &[&str], frames: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
        use std::io::Write;
        use std::process::Stdio;
        #[expect(
            clippy::disallowed_methods,
            reason = "these surfaces are the spawned binary's argv, exit code, and stdout"
        )]
        let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
            .arg("acp")
            .args(args)
            .arg("--session-dir")
            .arg(self.0.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project())
            .env("HOME", self.0.join("home"))
            .current_dir(self.project())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        {
            let mut stdin = child.stdin.take().ok_or("acp has no stdin")?;
            for frame in frames {
                serde_json::to_writer(&mut stdin, frame)?;
                stdin.write_all(b"\n")?;
            }
        }
        let output = child.wait_with_output()?;
        let mut responses = Vec::new();
        for line in stdout(&output).lines() {
            responses.push(serde_json::from_str::<Value>(line)?);
        }
        Ok(responses)
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

    let bad = workspace.project().join("bad.json");
    std::fs::write(&bad, "[1]")?;
    let malformed = ask(
        &workspace,
        r#"{"name":"yi"}"#,
        &["--schema", &bad.display().to_string()],
    )?;
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "a malformed schema is the caller's error, refused before the model runs"
    );
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("expected a schema object"),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );
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

/// D182 deleted the gates; a config written for them still loads, and says so once.
#[test]
fn a_config_that_still_names_gates_runs_and_says_the_key_is_gone() -> TestResult {
    let workspace = Workspace::new("config-gates")?;
    write_config(
        &workspace,
        r#"{"models":{"primary":"faux/faux-1"},"gates":{"artifact":false}}"#,
    )?;
    let answered = workspace.yi(&["ask", "gates check"])?;
    let stderr = String::from_utf8_lossy(&answered.stderr);
    assert_eq!(answered.status.code(), Some(0), "{stderr}");
    assert_eq!(stderr.matches("`gates`").count(), 1, "{stderr}");
    assert!(
        stdout(&answered).contains("faux: gates check"),
        "{}",
        stdout(&answered)
    );
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

#[test]
fn an_unknown_thinking_level_is_named_and_refused() -> TestResult {
    let workspace = Workspace::new("thinking-flag")?;
    let output = workspace.yi(&[
        "ask",
        "--model",
        "faux/faux-1",
        "--thinking",
        "extreme",
        "hi",
    ])?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("extreme"), "{stderr}");
    Ok(())
}

#[test]
fn an_unknown_thinking_level_in_the_config_is_named_and_refused() -> TestResult {
    let workspace = Workspace::new("thinking-config")?;
    std::fs::create_dir_all(workspace.0.join("home/.yi"))?;
    std::fs::write(
        workspace.0.join("home/.yi/config.json"),
        r#"{"model": "faux/faux-1", "thinking": "extreme"}"#,
    )?;
    let output = workspace.yi(&["ask", "hi"])?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config.json"), "{stderr}");
    Ok(())
}

#[test]
fn a_valid_thinking_level_is_accepted() -> TestResult {
    let workspace = Workspace::new("thinking-ok")?;
    let output = workspace.yi(&["ask", "--model", "faux/faux-1", "--thinking", "low", "hi"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Invariant: the proxy guards provider egress (E2), so a value ureq cannot
/// dial — `HTTPS_PROXY=https://…` is the spelling operators hit — fails a
/// provider run and leaves the offline faux path every gate uses alone.
#[test]
fn an_undialable_proxy_fails_a_provider_run_and_spares_the_faux_path() -> TestResult {
    let workspace = Workspace::new("proxy")?;
    let bad = [("HTTPS_PROXY", "ftp://proxy.corp:3128")];

    let offline = workspace.yi_env(&["ask", "--model", "faux/faux-1", "hi"], &bad)?;
    assert!(
        offline.status.success(),
        "the offline faux path died on a proxy it never dials: {}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert!(
        stdout(&offline).contains("faux: hi"),
        "{}",
        stdout(&offline)
    );

    let mut keyed = bad.to_vec();
    keyed.push(("ANTHROPIC_API_KEY", "unused-the-proxy-refusal-lands-first"));
    let provider = workspace.yi_env(
        &["ask", "--model", "anthropic/claude-haiku-4-5", "hi"],
        &keyed,
    )?;
    assert_eq!(
        provider.status.code(),
        Some(2),
        "a provider run must still fail closed on an undialable proxy"
    );
    let named = String::from_utf8_lossy(&provider.stderr).into_owned();
    assert!(named.contains("proxy.corp:3128"), "{named}");
    Ok(())
}

/// An unregistered verb falls through to the prompt catch-all, so a bad URL
/// must come back as the URL type's own refusal, never as an answer.
#[test]
fn fetch_serves_a_url_and_refuses_a_bad_one() -> TestResult {
    let workspace = Workspace::new("fetch")?;
    std::fs::write(workspace.project().join("note.txt"), "alpha\n")?;

    let served = workspace.yi(&["fetch", "local://note.txt"])?;
    assert_eq!(served.status.code(), Some(0));
    assert_eq!(stdout(&served), "alpha\n");

    let bad = workspace.yi(&["fetch", "not-a-url"])?;
    assert_eq!(bad.status.code(), Some(2));
    assert!(stdout(&bad).is_empty(), "{}", stdout(&bad));
    let named = String::from_utf8_lossy(&bad.stderr).into_owned();
    assert!(named.contains("has no scheme separator"), "{named}");
    Ok(())
}

#[test]
fn plans_config_takes_dir_and_names_every_other_key() -> TestResult {
    let workspace = Workspace::new("plans-config")?;
    let home = workspace.0.join("home");
    std::fs::create_dir_all(home.join(".yi"))?;
    std::fs::write(workspace.project().join("note.txt"), "read me")?;
    std::fs::write(
        home.join(".yi/config.json"),
        r#"{"plans":{"dir":"docs/plans"}}"#,
    )?;
    let accepted = workspace.yi(&["fetch", "local://note.txt"])?;
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "a config carrying only `dir` is accepted: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );

    for absent in ["folder", "mirror"] {
        std::fs::write(
            home.join(".yi/config.json"),
            format!(r#"{{"plans":{{"{absent}":"x"}}}}"#),
        )?;
        let typo = workspace.yi(&["fetch", "local://note.txt"])?;
        let named = String::from_utf8_lossy(&typo.stderr).into_owned();
        assert!(
            named.contains(absent),
            "a key the tree does not have is named, not ignored: {named}"
        );
    }
    Ok(())
}

#[test]
fn a_typed_prompt_lands_user_attributed_in_the_session_file() -> TestResult {
    let workspace = Workspace::new("attribution")?;
    ask(&workspace, "prove it", &[])?;
    let mut lines = String::new();
    for project in std::fs::read_dir(workspace.0.join("home/sessions"))? {
        for entry in std::fs::read_dir(project?.path())? {
            lines.push_str(&std::fs::read_to_string(entry?.path())?);
        }
    }
    assert!(lines.contains("prove it"), "{lines}");
    assert!(lines.contains(r#""attribution":"user""#), "{lines}");
    Ok(())
}

/// An ACP client shows the user whatever `session/new` says; "exit code 2" is not a reason.
#[test]
fn an_unknown_model_names_itself_over_acp() -> TestResult {
    let workspace = Workspace::new("acp-unknown-model")?;
    let frames = workspace.acp(
        &["--model", "openrouter/openai/gpt-6-astra"],
        &[
            serde_json::json!({"jsonrpc": "2.0", "id": "1", "method": "initialize",
                "params": {"protocolVersion": 2}}),
            serde_json::json!({"jsonrpc": "2.0", "id": "2", "method": "session/new",
                "params": {"cwd": workspace.project(), "mcpServers": []}}),
        ],
    )?;
    let reply = frames
        .iter()
        .find(|frame| frame["id"] == "2")
        .ok_or_else(|| format!("no reply to session/new in {frames:?}"))?;
    let message = reply["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("unknown model openrouter/openai/gpt-6-astra"),
        "the reason must reach the client, got: {reply}"
    );
    Ok(())
}

/// With no cache written yet, `yi catalog` reports the bundle and touches no network.
#[test]
fn catalog_reports_the_bundle_before_any_refresh() -> TestResult {
    let workspace = Workspace::new("catalog-bundled")?;
    let listed = workspace.yi(&["catalog"])?;
    assert_eq!(listed.status.code(), Some(0), "{}", stdout(&listed));
    let text = stdout(&listed);
    for provider in ["anthropic", "openai", "openrouter"] {
        assert!(text.contains(provider), "{text}");
    }
    assert_eq!(text.matches("bundled only").count(), 3, "{text}");
    let wrong = workspace.yi(&["catalog", "purge"])?;
    assert_eq!(wrong.status.code(), Some(2));
    Ok(())
}

/// A relative HOME planted a lane worktree beside the repository once; every surface refuses it.
#[test]
fn a_relative_home_is_refused_at_boot() -> TestResult {
    let workspace = Workspace::new("relative-home")?;
    let refused = workspace.yi_env(&["catalog"], &[("HOME", "../elsewhere")])?;
    assert_eq!(refused.status.code(), Some(2));
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(complaint.contains("HOME is relative"), "{complaint}");
    Ok(())
}

/// Rows 0023, 0025 and 0026 ran three builds that all said `yi 0.2.0`; the version a build
/// reports is the `version:` line of docs/ARCHITECTURE.md it was built from.
#[test]
fn version_prints_the_architecture_version() -> TestResult {
    let map = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/ARCHITECTURE.md"
    ))?;
    let version = map
        .lines()
        .find_map(|line| line.strip_prefix("version:"))
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or("docs/ARCHITECTURE.md has no version line")?;
    let workspace = Workspace::new("version")?;
    let printed = workspace.yi(&["--version"])?;
    assert_eq!(stdout(&printed).trim(), format!("yi {version}"));
    Ok(())
}

/// A headless drive is a harness: it claims no lane in a repository unless told `--lanes`,
/// so a run its harness kills leaves no orphan behind.
#[test]
fn a_headless_drive_claims_no_lane_unless_asked() -> TestResult {
    let workspace = Workspace::new("drive-no-lane")?;
    let project = workspace.project();
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "root",
        ][..],
    ] {
        #[expect(
            clippy::disallowed_methods,
            reason = "the journey needs a real repository"
        )]
        let done = Command::new("git")
            .args(args)
            .current_dir(&project)
            .output()?;
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
    }
    let script = workspace.0.join("quit.keys");
    std::fs::write(&script, "quit\n")?;
    let frames = workspace.0.join("frames");
    std::fs::create_dir_all(&frames)?;
    let keys = script.to_string_lossy().into_owned();
    let dir = frames.to_string_lossy().into_owned();
    let lanes = workspace.0.join("home/.yi/lanes");
    let quiet = workspace.yi(&[
        "tui",
        "--headless",
        "--keys",
        &keys,
        "--frames",
        &dir,
        "--model",
        "faux/faux-1",
    ])?;
    assert_eq!(
        quiet.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert!(!lanes.exists(), "a drive without --lanes claimed a lane");
    let claiming = workspace.yi(&[
        "tui",
        "--headless",
        "--lanes",
        "--keys",
        &keys,
        "--frames",
        &dir,
        "--model",
        "faux/faux-1",
    ])?;
    assert_eq!(
        claiming.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&claiming.stderr)
    );
    assert!(lanes.exists(), "--lanes should claim one");
    Ok(())
}

/// A python that cannot boot: the two kernel rows report it and `--fix` builds nothing,
/// so the doctor tests never pay a venv build in a temp HOME.
const NO_KERNEL: &[(&str, &str)] = &[("YI_KERNEL_PYTHON", "/nonexistent/python3")];

fn doctor_lines(output: &Output) -> Vec<String> {
    stdout(output).lines().map(str::to_owned).collect()
}

/// A dead socket file is the one repair `doctor --fix` makes without asking; a ledger row
/// whose root is gone is reported and left to the daemon that owns the file.
#[test]
fn doctor_reports_and_repairs_what_it_may() -> TestResult {
    let workspace = Workspace::new("doctor")?;
    let yi_dir = workspace.0.join("home/.yi");
    std::fs::create_dir_all(&yi_dir)?;
    std::fs::write(yi_dir.join("daemon.sock"), b"")?;
    std::fs::write(
        yi_dir.join("daemon.ledger.json"),
        r#"{"sessions":{"s-gone":{"cwd":"/nonexistent/yi-gone-root","unseen":0,"lastEventMs":1}}}"#,
    )?;
    let seen = workspace.yi_env(&["doctor"], NO_KERNEL)?;
    assert_eq!(seen.status.code(), Some(1), "{}", stdout(&seen));
    let lines = doctor_lines(&seen);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("FAIL  daemon-socket") && l.contains("dead socket file")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("FAIL  daemon-ledger") && l.contains("s-gone")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("ok    home")),
        "{lines:?}"
    );
    let fixed = workspace.yi_env(&["doctor", "--fix"], NO_KERNEL)?;
    let lines = doctor_lines(&fixed);
    assert!(
        lines.iter().any(|l| l.starts_with("fixed daemon-socket")),
        "{lines:?}"
    );
    assert!(
        !yi_dir.join("daemon.sock").exists(),
        "the dead socket file is gone"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("FAIL  daemon-ledger")),
        "a gone root is not doctor's to prune: {lines:?}"
    );
    assert_eq!(fixed.status.code(), Some(1));
    let json = workspace.yi_env(&["doctor", "--json"], NO_KERNEL)?;
    let rows: Value = serde_json::from_str(&stdout(&json))?;
    let names: Vec<&str> = rows
        .as_array()
        .ok_or("array")?
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "home",
            "config",
            "catalog",
            "python-runtime",
            "kernel-toolchain",
            "kernel-boot",
            "daemon-socket",
            "daemon-ledger",
            "lanes"
        ]
    );
    Ok(())
}

/// A config that will not parse is what `doctor` exists to say; it must not die of it first.
#[test]
fn doctor_runs_over_a_broken_config_and_names_the_key() -> TestResult {
    let workspace = Workspace::new("doctor-config")?;
    write_config(&workspace, r#"{"modle":"x"}"#)?;
    let seen = workspace.yi_env(&["doctor"], NO_KERNEL)?;
    assert_eq!(
        seen.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&seen.stderr)
    );
    let lines = doctor_lines(&seen);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("FAIL  config") && l.contains("modle")),
        "{lines:?}"
    );
    Ok(())
}

/// `doctor` reads the config before any session does, so its row is where a migrated key shows.
#[test]
fn doctor_names_a_config_key_the_migration_dropped() -> TestResult {
    let workspace = Workspace::new("doctor-migrated")?;
    write_config(&workspace, r#"{"gates":{"closure":false}}"#)?;
    let seen = workspace.yi_env(&["doctor"], NO_KERNEL)?;
    let lines = doctor_lines(&seen);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("ok    config") && l.contains("`gates`")),
        "{lines:?}"
    );
    Ok(())
}

/// With `telemetry.enabled`, a turn leaves one span file beside the session file, and
/// `yi stats telemetry <dir>` rolls every sidecar under a directory into one record.
#[test]
fn telemetry_writes_spans_beside_the_session_and_stats_rolls_them_up() -> TestResult {
    let workspace = Workspace::new("telemetry")?;
    write_config(&workspace, r#"{"telemetry":{"enabled":true}}"#)?;
    let answered = ask(&workspace, "span me", &[])?;
    assert_eq!(
        answered.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    let sessions = workspace.0.join("home/sessions");
    let mut sidecars = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out)
                } else if path.to_string_lossy().ends_with(".telemetry.jsonl") {
                    out.push(path)
                }
            }
        }
    }
    walk(&sessions, &mut sidecars);
    assert_eq!(
        sidecars.len(),
        1,
        "one sidecar beside the session file: {sidecars:?}"
    );
    let text = std::fs::read_to_string(&sidecars[0])?;
    let spans: Vec<Value> = text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert!(
        spans.iter().any(|s| s["span"] == "request"
            && s["provider"] == "faux"
            && s.get("ttftMs").is_some()),
        "{text}"
    );
    assert!(spans.iter().any(|s| s["span"] == "turn"), "{text}");
    let dir = sessions.to_string_lossy().into_owned();
    let rolled = workspace.yi(&["stats", "--json", "telemetry", &dir])?;
    assert_eq!(
        rolled.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&rolled.stderr)
    );
    let record: Value = serde_json::from_str(&stdout(&rolled))?;
    assert_eq!(record["requests"], 1, "{record}");
    assert_eq!(record["turns"], 1, "{record}");
    assert_eq!(record["files"], 1, "{record}");
    Ok(())
}

#[test]
fn memory_imports_lists_checks_and_forgets() -> TestResult {
    let workspace = Workspace::new("memory")?;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../runtime/tests/fixtures/memory/claude");
    let import = workspace.yi(&["memory", "import", &fixture.to_string_lossy()])?;
    assert_eq!(
        String::from_utf8_lossy(&import.stdout).trim(),
        "memory · imported 3 · updated 0 · skipped 0"
    );
    let list = workspace.yi(&["memory"])?;
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(
        text.contains("memory · 3 repo · 0 global · 1 unparsed"),
        "{text}"
    );
    assert!(
        text.contains("never-relax-linters            feedback  repo"),
        "{text}"
    );
    assert!(text.contains("! broken-quote"), "{text}");
    let check = workspace.yi(&["memory", "check"])?;
    assert_eq!(check.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("broken-quote.md:3: unclosed quote"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    let forget = workspace.yi(&["memory", "forget", "fgj-forge-cli"])?;
    assert_eq!(
        String::from_utf8_lossy(&forget.stdout).trim(),
        "memory · forgot fgj-forge-cli · repo"
    );
    let show = workspace.yi(&["memory", "show", "fgj-forge-cli"])?;
    assert_eq!(show.status.code(), Some(1));
    let usage = workspace.yi(&["memory", "frobnicate"])?;
    assert_eq!(usage.status.code(), Some(2));
    Ok(())
}

use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

struct Workspace(Scratch);

impl Workspace {
    fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-cli-{tag}"))?;
        std::fs::create_dir_all(dir.join("project"))?;
        dir.home()?;
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

/// D85's row for the login verbs (D191): a key piped on stdin lands 0600 in the
/// token store, an unknown provider names the profile path, and bare `yi logout`
/// clears even a token whose profile is gone. The login fast path answers before
/// the shared flags parse, so the binary is driven directly.
#[test]
fn login_logout_round_trip() -> TestResult {
    let workspace = Workspace::new("login")?;
    let home = workspace.0.join("home");
    let login = |args: &[&str], stdin_text: &str| -> Result<Output, Box<dyn Error>> {
        use std::io::Write;
        use std::process::Stdio;
        #[expect(
            clippy::disallowed_methods,
            reason = "the login fast path's contract is argv, stdin, exit code and files"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        let mut child = command
            .args(args)
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(stdin_text.as_bytes())?;
        Ok(child.wait_with_output()?)
    };

    let listed = login(&["login", "--list"], "")?;
    assert_eq!(listed.status.code(), Some(0));
    assert!(stdout(&listed).contains("openai"), "{}", stdout(&listed));

    let unknown = login(&["login", "bogus"], "")?;
    assert_eq!(unknown.status.code(), Some(2));
    let complaint = String::from_utf8_lossy(&unknown.stderr).into_owned();
    assert!(complaint.contains(".yi/oauth/bogus.json"), "{complaint}");

    // The documented path: the key comes from stdin, never argv.
    let saved = login(&["login", "openai"], "sk-test-pasted\n")?;
    assert_eq!(saved.status.code(), Some(0), "{:?}", saved);
    let token = home.join(".yi/providers/tokens/openai.json");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&token)?)?;
    assert_eq!(stored["kind"], "key");
    assert_eq!(stored["access"], "sk-test-pasted");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&token)?.permissions().mode() & 0o777,
            0o600
        );
    }

    // A token whose profile was never written still logs out.
    std::fs::write(
        home.join(".yi/providers/tokens/lonely.json"),
        r#"{"version":1,"kind":"key","access":"x"}"#,
    )?;
    let cleared = login(&["logout"], "")?;
    assert_eq!(cleared.status.code(), Some(0));
    assert!(!token.exists(), "openai token removed");
    assert!(!home.join(".yi/providers/tokens/lonely.json").exists());
    Ok(())
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

/// A review round's prompt is a whole diff; one argv string past Linux's 128 KiB cap is E2BIG, so
/// `yi ask -` reads the prompt from stdin, all of it, and an empty stdin is a usage error.
#[test]
fn ask_reads_a_prompt_past_the_argument_cap_from_stdin() -> TestResult {
    use std::io::Write;
    use std::process::Stdio;
    let workspace = Workspace::new("ask-stdin")?;
    let run = |text: &str| -> Result<Output, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the contract is the spawned binary's stdin, exit code and stdout"
        )]
        let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
            .args(["ask", "--model", "faux/faux-1", "--json", "-"])
            .arg("--session-dir")
            .arg(workspace.0.join("home/sessions"))
            .arg("--cwd")
            .arg(workspace.project())
            .env("HOME", workspace.0.join("home"))
            .current_dir(workspace.project())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(text.as_bytes())?;
        Ok(child.wait_with_output()?)
    };
    let prompt = format!(
        "diff --git a/r.rs b/r.rs\n{}end of the round",
        "+    let café = seats(finding, probes);\n".repeat(5_000)
    );
    assert!(prompt.len() > 128 * 1024, "{}", prompt.len());
    let answered = run(&prompt)?;
    assert_eq!(answered.status.code(), Some(0), "{answered:?}");
    let asked = stdout(&answered)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event.get("type").and_then(Value::as_str) == Some("agent_end"))
        .and_then(|end| end.pointer("/messages/0/content").cloned());
    assert_eq!(
        asked.as_ref().and_then(Value::as_str),
        Some(prompt.as_str()),
        "the model is asked all of stdin"
    );

    let empty = run("")?;
    assert_eq!(empty.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no prompt on stdin"));
    Ok(())
}

/// A `--schema` answer is the newest message holding a matching value. A reader narrated
/// "`lines[0]`" before a read, or answered and then wrapped up its todos in prose; both exited 3.
#[test]
fn a_schema_answer_is_the_newest_message_that_matches() -> TestResult {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason::{Stop, ToolUse};
    let workspace = Workspace::new("schema-newest")?;
    std::fs::write(workspace.project().join("r.rs"), "let lines = vec![1];\n")?;
    let json = r#"{"refuted": true, "reason": "r.rs never indexes past 0"}"#;
    let read = || {
        faux_tool_call(
            "c0",
            "read",
            json!({"path": "r.rs"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        )
    };
    let runs = [
        [
            (
                vec![faux_text("Let me read `lines[0]` first."), read()],
                ToolUse,
            ),
            (vec![faux_text(json)], Stop),
        ],
        [
            (vec![faux_text(json), read()], ToolUse),
            (vec![faux_text("The JSON {above} is my answer.")], Stop),
        ],
    ];
    let schema = r#"{"type":"object","required":["refuted","reason"],"properties":{"refuted":{"type":"boolean"},"reason":{"type":"string"}}}"#;
    for (n, run) in runs.into_iter().enumerate() {
        let lines = run
            .into_iter()
            .map(|(content, stop)| serde_json::to_string(&faux_assistant_message(content, stop)))
            .collect::<Result<Vec<_>, _>>()?;
        let script = workspace.project().join(format!("script-{n}.jsonl"));
        std::fs::write(&script, lines.join("\n"))?;
        let script = script.display().to_string();
        let answered = ask(
            &workspace,
            "break the claim",
            &["--faux", &script, "--schema", schema],
        )?;
        let said = String::from_utf8_lossy(&answered.stderr).into_owned();
        assert_eq!(answered.status.code(), Some(0), "run {n}: {said}");
        let value: Value = serde_json::from_str(stdout(&answered).trim())?;
        assert_eq!(value.get("refuted"), Some(&json!(true)), "run {n}: {value}");
    }
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
    let family = workspace.0.join("home/sessions/family");
    std::fs::create_dir_all(family.join(&id))?;
    std::fs::write(family.join(&id).join("note.json"), "{}")?;
    std::fs::create_dir_all(family.join("other"))?;
    let clock = workspace.0.join("home/sessions/schedules").join(&id);
    std::fs::create_dir_all(&clock)?;
    std::fs::write(clock.join("scheduled-jobs.json"), "{}")?;
    workspace.yi(&["sessions", "rm", &id])?;
    let after: Value =
        serde_json::from_str(&stdout(&workspace.yi(&["sessions", "--json", "list"])?))?;
    assert_eq!(after.as_array().map(Vec::len), Some(0));
    assert!(
        !family.join(&id).exists(),
        "the D242 board outlives its session"
    );
    assert!(
        family.join("other").exists(),
        "rm reached another session's board"
    );
    assert!(!clock.exists(), "a removed session's clocks stayed on disk");
    Ok(())
}

/// `list` scans the raw file for a name fact; `show` replays it through `SessionStore`,
/// which must carry the same name through (a session named after creation still shows it).
#[test]
fn show_json_carries_the_name_a_session_file_sets() -> TestResult {
    let workspace = Workspace::new("show-name")?;
    let cwd = workspace.project().display().to_string();
    let session_dir = workspace
        .0
        .join("home/sessions")
        .join(yi_runtime::session_store::session_directory_name(&cwd));
    std::fs::create_dir_all(&session_dir)?;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../types/tests/fixtures/v4-golden.jsonl");
    std::fs::copy(fixture, session_dir.join("1_fixture-a.jsonl"))?;

    let shown: Value = serde_json::from_str(&stdout(&workspace.yi(&[
        "sessions",
        "--json",
        "show",
        "fixture-a",
    ])?))?;
    assert_eq!(
        shown["session"]["name"].as_str(),
        Some("Golden Fixture v4"),
        "{shown}"
    );
    Ok(())
}

/// Dies with an old session's list reading differently after the todo merge: `yi todo` over a
/// recorded session prints what the pre-merge binary printed for it, byte for byte.
#[test]
fn yi_todo_reads_a_recorded_session_as_before_the_merge() -> TestResult {
    let workspace = Workspace::new("todo-before")?;
    let cwd = workspace.project().display().to_string();
    let session_dir = workspace
        .0
        .join("home/sessions")
        .join(yi_runtime::session_store::session_directory_name(&cwd));
    std::fs::create_dir_all(&session_dir)?;
    // The scrub masked every id and the loader refuses duplicates, so they are re-minted as a chain.
    let mut rows = Vec::new();
    let mut parent = Value::Null;
    for (index, line) in include_str!("../../tui/tests/fixtures/sessions/01a0d6e4.jsonl")
        .lines()
        .enumerate()
    {
        let mut row: Value = serde_json::from_str(line)?;
        let id = if index == 0 {
            json!("recorded")
        } else {
            json!(format!("e{index}"))
        };
        row["id"] = id.clone();
        if row.get("parentId").is_some() {
            row["parentId"] = std::mem::replace(&mut parent, id);
        } else if index > 0 {
            parent = id;
        }
        rows.push(row.to_string());
    }
    std::fs::write(session_dir.join("1_recorded.jsonl"), rows.join("\n") + "\n")?;
    let output = workspace.yi(&["todo"])?;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), include_str!("fixtures/todo-before.txt"));
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
    // A void review round's only evidence is this line: what the model said, cut loudly at 160.
    let said = String::from_utf8_lossy(&prose.stderr).into_owned();
    assert!(
        said.contains(r#"began: "faux: not json at all" (21 of 21 characters"#),
        "{said}"
    );
    for (fill, notice) in [
        (154, "(160 of 160 characters"),
        (155, "(160 of 161 characters"),
    ] {
        let long = ask(&workspace, &"x".repeat(fill), &["--schema", schema])?;
        let said = String::from_utf8_lossy(&long.stderr).into_owned();
        assert!(said.contains(notice), "{fill}: {said}");
    }

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
    let created = workspace.project().join("created.txt");
    std::fs::write(&kept, "before\n")?;
    let script = tool_script(
        &workspace,
        &[
            ("write", json!({"path": "kept.txt", "content": "after\n"})),
            ("write", json!({"path": "created.txt", "content": "new\n"})),
        ],
        Some("written"),
    )?;
    ask(&workspace, "start a turn", &["--faux", &script])?;
    assert_eq!(std::fs::read_to_string(&kept)?, "after\n");

    let undone = workspace.yi(&["undo"])?;
    if git_missing(&undone) {
        return Ok(());
    }
    assert_eq!(undone.status.code(), Some(0), "{}", stdout(&undone));
    assert_eq!(std::fs::read_to_string(&kept)?, "before\n");
    assert!(!created.exists());

    workspace.yi(&["undo"])?;
    assert_eq!(std::fs::read_to_string(&kept)?, "after\n");
    assert_eq!(std::fs::read_to_string(&created)?, "new\n");
    Ok(())
}

/// A hand edit after the turn is out of scope (b, c) or, on a path the turn wrote, kept and
/// named (d): `yi undo` moves only what the turn changed.
#[test]
fn undo_moves_only_what_the_turn_wrote() -> TestResult {
    let workspace = Workspace::new("undo-scope")?;
    let project = workspace.project();
    std::fs::write(project.join("b.txt"), "before\n")?;
    let script = tool_script(
        &workspace,
        &[
            ("write", json!({"path": "a.txt", "content": "turn\n"})),
            ("write", json!({"path": "d.txt", "content": "turn\n"})),
        ],
        Some("written"),
    )?;
    ask(&workspace, "write two files", &["--faux", &script])?;
    assert_eq!(std::fs::read_to_string(project.join("a.txt"))?, "turn\n");

    std::fs::write(project.join("b.txt"), "hand\n")?;
    std::fs::write(project.join("c.txt"), "mine\n")?;
    std::fs::write(project.join("d.txt"), "hand\n")?;

    let undone = workspace.yi(&["undo"])?;
    if git_missing(&undone) {
        return Ok(());
    }
    let said = stdout(&undone);
    assert_eq!(undone.status.code(), Some(0), "{said}");
    assert!(!project.join("a.txt").exists(), "{said}");
    assert_eq!(std::fs::read_to_string(project.join("b.txt"))?, "hand\n");
    assert_eq!(std::fs::read_to_string(project.join("c.txt"))?, "mine\n");
    assert_eq!(std::fs::read_to_string(project.join("d.txt"))?, "hand\n");
    assert!(
        said.contains("[kept 1 of 2") && said.contains("d.txt") && !said.contains("b.txt"),
        "{said}"
    );
    Ok(())
}

/// A process that dies mid-turn leaves a turn start with no turn end: undo says so and
/// restores every path changed since, the hand edit on b.txt included.
#[test]
fn undo_without_a_turn_end_restores_unscoped_and_says_so() -> TestResult {
    let workspace = Workspace::new("undo-unscoped")?;
    let project = workspace.project();
    std::fs::write(project.join("b.txt"), "before\n")?;
    let script = tool_script(
        &workspace,
        &[("write", json!({"path": "a.txt", "content": "turn\n"}))],
        Some("written"),
    )?;
    ask(&workspace, "write a file", &["--faux", &script])?;
    std::fs::write(project.join("b.txt"), "hand\n")?;

    let sessions = workspace.0.join("home/sessions");
    // `family/` and `kernels/` (#580) sit beside the transcript's directory.
    let file = std::fs::read_dir(&sessions)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .flat_map(|files| files.filter_map(Result::ok))
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .ok_or("no session file")?;
    let log = std::fs::read_to_string(&file)?;
    let (kept, last) = log.trim_end().rsplit_once('\n').ok_or("one-line session")?;
    assert!(last.contains(r#""at":"turnEnd""#), "{last}");
    std::fs::write(&file, format!("{kept}\n"))?;

    let undone = workspace.yi(&["undo"])?;
    if git_missing(&undone) {
        return Ok(());
    }
    let said = stdout(&undone);
    assert_eq!(undone.status.code(), Some(0), "{said}");
    assert!(
        said.contains(
            "[unscoped — no turn-end checkpoint pairs with this one, so every path changed \
             since it moved]"
        ),
        "{said}"
    );
    assert!(!project.join("a.txt").exists(), "{said}");
    assert_eq!(std::fs::read_to_string(project.join("b.txt"))?, "before\n");
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

/// Checkpoints are a no-op without git (design §7.7), and so is this test.
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

/// Invariant: the levers override rides an eval harness's own `--eval`, so the variable
/// alone is inert in a user's run, `--deadline` or not, and a refused file stops the run
/// before any model call (D220).
#[test]
fn yi_levers_is_read_only_under_the_eval_flag() -> TestResult {
    let workspace = Workspace::new("levers")?;
    let out_of_range = workspace.project().join("wide.json");
    std::fs::write(&out_of_range, r#"{"plan.width_max": 99}"#)?;
    let wide = out_of_range.display().to_string();
    let refused = workspace.yi_env(
        &["ask", "--model", "faux/faux-1", "--eval", "hi"],
        &[("YI_LEVERS", wide.as_str())],
    )?;
    assert_eq!(refused.status.code(), Some(1));
    let reason = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        reason.contains("plan.width_max wants an integer in 1..=16, not 99")
            && reason.contains("refusal:config"),
        "{reason}"
    );
    for extra in [&[][..], &["--deadline", "600"][..]] {
        let mut args = vec!["ask", "--model", "faux/faux-1"];
        args.extend_from_slice(extra);
        args.push("hi");
        let ran = workspace.yi_env(&args, &[("YI_LEVERS", wide.as_str())])?;
        let said = String::from_utf8_lossy(&ran.stderr).into_owned();
        assert!(!said.contains("YI_LEVERS"), "the file was read: {said}");
        assert!(!said.contains("levers:"), "the run was overridden: {said}");
    }
    let narrow = workspace.project().join("narrow.json");
    std::fs::write(&narrow, r#"{"plan.width_max": 4}"#)?;
    let announced = workspace.yi_env(
        &["ask", "--model", "faux/faux-1", "--eval", "hi"],
        &[("YI_LEVERS", narrow.display().to_string().as_str())],
    )?;
    let banner = String::from_utf8_lossy(&announced.stderr).into_owned();
    assert!(banner.contains("levers: this run reads"), "{banner}");
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
    for provider in [
        "anthropic",
        "openai",
        "openrouter",
        "openai-codex",
        "google",
    ] {
        assert!(text.contains(provider), "{text}");
    }
    // Every listed row is bundled-only before any refresh; the row count comes from the
    // binary's own output so a new provider fails on presence, not on a stale count.
    let rows = text.lines().filter(|line| !line.trim().is_empty()).count();
    assert_eq!(text.matches("bundled only").count(), rows, "{text}");
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

/// With no HOME every `~/.yi` path resolved against the working directory, so a session wrote
/// `.yi/` into whatever directory yi ran in; an unset HOME is refused like a relative one.
#[test]
fn an_unset_home_is_refused_and_writes_nothing_here() -> TestResult {
    let workspace = Workspace::new("unset-home")?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the contract is the spawned binary's exit code and what it wrote"
    )]
    let homeless = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_yi"))
            .args(args)
            .env_remove("HOME")
            .envs(NO_KERNEL.iter().copied())
            .current_dir(workspace.project())
            .output()
    };
    let refused = homeless(&["ask", "--model", "faux/faux-1", "hi"])?;
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(!workspace.project().join(".yi").exists(), "{complaint}");
    assert_eq!(refused.status.code(), Some(2), "{complaint}");
    assert!(complaint.contains("HOME is not set"), "{complaint}");
    // `doctor` runs before the check, so it is the one surface that reports it.
    let lines = doctor_lines(&homeless(&["doctor"])?);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("FAIL  home") && l.ends_with("HOME is not set")),
        "{lines:?}"
    );
    Ok(())
}

/// `catalog.enabled: false` turns the refresh off, so a week-old cache nothing will renew is
/// the configured state, not a fault the doctor fails on.
#[test]
fn doctor_passes_an_old_catalog_when_refresh_is_off() -> TestResult {
    let workspace = Workspace::new("doctor-catalog-off")?;
    write_config(&workspace, r#"{"catalog":{"enabled":false}}"#)?;
    let catalog = workspace.0.join("home/.yi/catalog");
    std::fs::create_dir_all(&catalog)?;
    let cache = catalog.join("openrouter.json");
    let entry = json!({"schema": 2, "openai-completions": {"probe/flash": {
        "id": "probe/flash", "name": "probe", "api": "openai-completions", "provider": "openrouter",
        "baseUrl": "http://openrouter.ai.invalid/api/v1", "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 100000, "maxTokens": 256}}});
    std::fs::write(&cache, entry.to_string())?;
    let week = std::time::Duration::from_secs(8 * 24 * 3600);
    let written = std::fs::metadata(&cache)?
        .modified()?
        .checked_sub(week)
        .ok_or("clock before the epoch")?;
    std::fs::File::options()
        .write(true)
        .open(&cache)?
        .set_modified(written)?;
    let json = workspace.yi_env(&["doctor", "--json"], NO_KERNEL)?;
    let rows: Value = serde_json::from_str(&stdout(&json))?;
    let row = rows
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "catalog"))
        .ok_or_else(|| format!("no catalog row: {rows}"))?;
    assert_eq!(row["status"], "ok", "{row}");
    assert_eq!(
        row["detail"],
        "refresh off (`catalog.enabled: false`); caches stay as last written"
    );
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
    std::fs::create_dir_all(yi_dir.join("catalog"))?;
    std::fs::write(
        yi_dir.join("catalog/openrouter.json"),
        r#"{"openai-completions":{"x/hand-edited":{"id":"x/hand-edited"}}}"#,
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
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("FAIL  catalog") && l.contains("x/hand-edited")),
        "a cache entry that does not load is named: {lines:?}"
    );
    // D209: where yi runs decides what a contained command and a placement can reach.
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("ok    host") && l.len() > "ok    host".len() + 2),
        "the doctor names the host environment: {lines:?}"
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
            "host",
            "home",
            "config",
            "classifier",
            "catalog",
            "python-runtime",
            "kernel-toolchain",
            "kernel-boot",
            "daemon-socket",
            "daemon-ledger",
            "lanes",
            "cache"
        ]
    );
    assert_eq!(rows[11]["detail"], "no session here", "{rows}");
    Ok(())
}

/// C5: the cache row replays the newest session and reports its notice without failing,
/// and never repairs the file, which may be the live session another process appends to.
#[test]
fn doctor_reports_a_cache_notice_and_leaves_the_session_file_alone() -> TestResult {
    let workspace = Workspace::new("doctor-cache")?;
    let dir =
        workspace
            .0
            .join("home/sessions")
            .join(yi_runtime::session_store::session_directory_name(
                &workspace.project().to_string_lossy(),
            ));
    std::fs::create_dir_all(&dir)?;
    let torn = format!(
        "{}{{\"kind\":\"entr",
        include_str!("../../runtime/tests/fixtures/cache/opus_no_marks.jsonl")
    );
    let file = dir.join("1790575150042_s.jsonl");
    std::fs::write(&file, &torn)?;
    let json = workspace.yi_env(&["doctor", "--json"], NO_KERNEL)?;
    let rows: Value = serde_json::from_str(&stdout(&json))?;
    let row = &rows[11];
    assert_eq!(row["name"], "cache", "{rows}");
    assert_eq!(row["status"], "ok", "{row}");
    let detail = row["detail"].as_str().ok_or("detail")?;
    assert!(
        detail.contains("nothing written or read 37") && detail.contains("[cache]"),
        "{detail}"
    );
    assert_eq!(std::fs::read_to_string(&file)?, torn, "the torn tail stays");
    Ok(())
}

/// A report-only doctor on a machine with no toolchain says what the build would fetch; the
/// dead proxy makes a regression fail fast instead of downloading uv.
#[test]
fn doctor_without_fix_reports_the_uv_fetch_instead_of_running_it() -> TestResult {
    let workspace = Workspace::new("doctor-no-toolchain")?;
    let dead = "http://127.0.0.1:9";
    let seen = workspace.yi_env(
        &["doctor"],
        &[("PATH", ""), ("ALL_PROXY", dead), ("HTTPS_PROXY", dead)],
    )?;
    let lines = doctor_lines(&seen);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("ok    kernel-toolchain") && l.contains("fetches uv")),
        "{lines:?}"
    );
    assert!(!workspace.0.join("home/.yi/uv").exists());
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

/// A `--faux` script: a bash call running `command`, then `after` if given.
fn bash_script(
    workspace: &Workspace,
    command: &str,
    after: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    tool_script(workspace, &[("bash", json!({"command": command}))], after)
}

/// A `--faux` script: one assistant message carrying every `(tool, args)` call, then `after`.
fn tool_script(
    workspace: &Workspace,
    calls: &[(&str, Value)],
    after: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let calls = calls
        .iter()
        .enumerate()
        .map(|(index, (tool, args))| {
            let id = format!("c{index}");
            faux_tool_call(&id, tool, args.as_object().cloned().unwrap_or_default())
        })
        .collect();
    let mut lines = vec![serde_json::to_string(&faux_assistant_message(
        calls,
        StopReason::ToolUse,
    ))?];
    if let Some(text) = after {
        let said = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
        lines.push(serde_json::to_string(&said)?);
    }
    let path = workspace.project().join("script.jsonl");
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path.display().to_string())
}

/// Dies with the last word started at `--deadline` and followed 90 s past it: the eval
/// harness kills `yi ask` at the deadline, so the answer and the lane release were lost.
#[test]
fn a_run_past_its_deadline_answers_and_exits_inside_it() -> TestResult {
    let workspace = Workspace::new("deadline-last-word")?;
    let script = bash_script(&workspace, "sleep 100", Some("the last word"))?;
    let started = std::time::Instant::now();
    let answered = ask(
        &workspace,
        "go",
        &["--faux", &script, "--json", "--deadline", "12"],
    )?;
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(12), "{took:?}");
    assert!(
        stdout(&answered).contains("the last word"),
        "{}",
        stdout(&answered)
    );
    assert_eq!(answered.status.code(), Some(0));
    Ok(())
}

/// Dies with the no-answer exit untried live: a run with no assistant text exits 1, and an
/// eval run exits 0 and says `no_answer` in its stream, since harbor reads 1 as a crash.
#[test]
fn a_run_with_no_text_exits_one_except_under_eval() -> TestResult {
    let workspace = Workspace::new("no-answer")?;
    let script = bash_script(&workspace, "true", None)?;
    let bare = ask(&workspace, "go", &["--faux", &script, "--json"])?;
    assert_eq!(
        bare.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    let eval = ask(&workspace, "go", &["--faux", &script, "--json", "--eval"])?;
    assert_eq!(
        eval.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&eval.stderr)
    );
    assert!(
        stdout(&eval).contains(r#""type":"no_answer""#),
        "{}",
        stdout(&eval)
    );
    Ok(())
}

/// Incident (#580): the contained bash tool's writable roots held the whole session corpus,
/// where every session's kernel snapshot sits and is loaded at that session's next boot.
#[cfg(target_os = "macos")]
#[test]
fn a_contained_bash_call_cannot_write_the_session_corpus() -> TestResult {
    if !std::path::Path::new("/usr/bin/sandbox-exec").is_file() {
        return Ok(());
    }
    let workspace = Workspace::new("corpus")?;
    // Outside every temp root, which the sandbox grants whole, or the test proves nothing.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let corpus = home.join(format!("yi-cli-corpus-{}", std::process::id()));
    std::fs::create_dir_all(&corpus)?;
    let planted = corpus.join("01other.kernel-state.dill");
    let script = tool_script(
        &workspace,
        &[(
            "bash",
            json!({"command": format!("touch '{}'", planted.display())}),
        )],
        Some("done"),
    )?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the spawned binary's session dir must sit outside tmp, unlike Workspace::yi's"
    )]
    let ran = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "ask",
            "--model",
            "faux/faux-1",
            "--faux",
            &script,
            "plant it",
        ])
        .arg("--session-dir")
        .arg(&corpus)
        .arg("--cwd")
        .arg(workspace.project())
        .env("HOME", workspace.0.join("home"))
        .current_dir(workspace.project())
        .output();
    let wrote = planted.is_file();
    let _ = std::fs::remove_dir_all(&corpus);
    let said = stdout(&ran?);
    assert!(
        !wrote,
        "a contained bash call wrote into the session corpus: {said}"
    );
    Ok(())
}

/// Every request body a loopback proxy standing in for openrouter.ai reads, each answered `ok`.
fn openrouter_proxy() -> Result<(u16, std::sync::mpsc::Receiver<Value>), Box<dyn Error>> {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (sender, bodies) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let (mut length, mut line) = (0usize, String::new());
            while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim().is_empty() {
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let _ = sender.send(serde_json::from_slice(&body).unwrap_or(Value::Null));
            let reply = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Ok((port, bodies))
}

/// A root session sends its own id as OpenRouter's affinity key, the one its children inherit
/// (D314): `build_session` hands the session id to the stream.
#[test]
fn a_root_session_sends_its_id_as_the_openrouter_session() -> TestResult {
    let workspace = Workspace::new("family-key")?;
    let catalog = workspace.0.join("home/.yi/catalog");
    std::fs::create_dir_all(&catalog)?;
    let entry = json!({"openai-completions": {"probe/flash": {
        "id": "probe/flash", "name": "probe", "api": "openai-completions", "provider": "openrouter",
        "baseUrl": "http://openrouter.ai.invalid/api/v1", "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 100000, "maxTokens": 256}}});
    std::fs::write(catalog.join("openrouter.json"), entry.to_string())?;
    let (port, bodies) = openrouter_proxy()?;
    let proxy = format!("http://127.0.0.1:{port}");
    let answered = workspace.yi_env(
        &["ask", "--model", "openrouter/probe/flash", "--json", "hi"],
        &[
            ("OPENROUTER_API_KEY", "sk-test"),
            ("HTTPS_PROXY", &proxy),
            NO_KERNEL[0],
        ],
    )?;
    let body = bodies
        .try_iter()
        .find(|body| body.get("messages").is_some())
        .ok_or_else(|| format!("no request: {}", String::from_utf8_lossy(&answered.stderr)))?;
    let session = files_under(&workspace.0.join("home/sessions"))
        .into_iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .find_map(|name| {
            let id = name.strip_suffix(".jsonl")?.split_once('_')?.1;
            (!id.contains('.')).then(|| id.to_owned())
        })
        .ok_or("no session file")?;
    assert_eq!(body["session_id"], session.as_str(), "{body}");
    Ok(())
}

fn files_under(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

mod config;
mod edits;
mod gates;
mod managed;
mod outputs;

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, PermissionBroker, PermissionMode, ProviderStream, SessionConfig};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput};
use yi_types::message::{AgentMessage, StopReason};

type TestResult = Result<(), Box<dyn Error>>;

struct Case {
    root: Scratch,
    home: PathBuf,
    cwd: PathBuf,
}

struct Outcome {
    text: String,
    is_error: bool,
}

#[derive(Default)]
struct Gate {
    mode: Option<PermissionMode>,
    sandbox: bool,
    rules: Vec<yi_runtime::ConfigRule>,
    cwd: Option<PathBuf>,
    child_of: Option<PathBuf>,
    holds: Vec<yi_permission::Hold>,
    journal: Option<Arc<dyn Fn(yi_types::permission::PermissionRecord) + Send + Sync>>,
    count_edit_changes: bool,
    fail_stub: bool,
    abort_after: Option<PathBuf>,
    kept: Vec<PathBuf>,
}

/// Returns its configured text, so a test sets the exact bytes a hook's `tool_response` holds.
struct FakeTool {
    name: &'static str,
    output: String,
    fail: bool,
}

impl Tool for FakeTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "fake tool for adapter integration"
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    fn execute(&self, _input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        let mut output = yi_tools::text_output(self.output.clone());
        output.is_error = self.fail;
        output
    }

    fn changes(&self, _input: &Map<String, Value>, _cwd: &Path) -> Vec<yi_tools::FileChange> {
        if self.name == "edit" {
            EDIT_CHANGES.fetch_add(1, Ordering::Relaxed);
        }
        Vec::new()
    }
}

static EDIT_CHANGES: AtomicU32 = AtomicU32::new(0);

fn faux_model() -> yi_types::model::Model {
    let zero = || serde_json::Number::from(0u64);
    yi_types::model::Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: yi_types::model::ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn command(text: &str) -> Value {
    json!({"type": "command", "command": text})
}

fn settings(event: &str, matcher: &str, handlers: Vec<Value>) -> Value {
    json!({"hooks": {(event): [{"matcher": matcher, "hooks": handlers}]}})
}

fn printing(output: &Value) -> Value {
    command(&format!("printf '%s' '{output}'"))
}

fn decision(decision: &str) -> Value {
    let reason = format!("{decision} reason");
    printing(&json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason
    }}))
}

fn touching(marker: &Path) -> Value {
    command(&format!("touch '{}'", marker.display()))
}

/// One write per input, so handlers that run at once never interleave their lines.
fn logging(path: &Path) -> Value {
    let path = path.display();
    command(&format!(
        "line=$(cat); printf '%s\\n' \"$line\" >> '{path}'"
    ))
}

fn write_json(path: &Path, value: &Value) -> TestResult {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(std::fs::write(path, serde_json::to_vec(value)?)?)
}

fn read_json(path: &Path) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn read_lines(path: &Path) -> Result<Vec<Value>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    let lines = text.lines().filter(|line| !line.trim().is_empty());
    Ok(lines.map(serde_json::from_str).collect::<Result<_, _>>()?)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude_hooks")
        .join(name)
}

fn asking(mode: PermissionMode) -> Gate {
    Gate {
        mode: Some(mode),
        ..Gate::default()
    }
}

impl Case {
    /// `config` is the user-scope settings file, which loads without a trust grant.
    fn new(label: &str, config: Value, enabled: bool) -> Result<Self, Box<dyn Error>> {
        let root = Scratch::new(&format!("yi-claude-hooks-{label}"))?;
        let home = root.join("home");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd)?;
        write_json(&home.join(".claude/settings.json"), &config)?;
        // SAFETY: nextest isolates each test in its own process.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::remove_var("YI_CLAUDE_HOOKS_FILES");
            std::env::remove_var("CLAUDE_PROJECT_DIR");
            std::env::remove_var("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB");
            if enabled {
                std::env::set_var("YI_CLAUDE_HOOKS", "1");
            } else {
                std::env::remove_var("YI_CLAUDE_HOOKS");
            }
        }
        Ok(Self { root, home, cwd })
    }

    fn user(&self, config: &Value) -> TestResult {
        write_json(&self.home.join(".claude/settings.json"), config)
    }

    fn trust(&self, project: &Path) -> TestResult {
        let sources = yi_runtime::ext::contributions(project, &self.home);
        Ok(yi_runtime::TrustGate::new(&self.home).grant(project, &sources)?)
    }

    async fn run(&self, tool: &str, args: Value) -> Result<Outcome, Box<dyn Error>> {
        self.run_gated(tool, args, Gate::default()).await
    }

    async fn run_gated(
        &self,
        tool: &str,
        args: Value,
        gate: Gate,
    ) -> Result<Outcome, Box<dyn Error>> {
        let mut outcomes = self.run_calls(vec![(tool, args)], gate, "x").await?;
        outcomes.pop().ok_or_else(|| "no tool result".into())
    }

    async fn run_calls(
        &self,
        calls: Vec<(&str, Value)>,
        gate: Gate,
        stub_output: &str,
    ) -> Result<Vec<Outcome>, Box<dyn Error>> {
        static SESSION: AtomicU32 = AtomicU32::new(0);
        let provider = Arc::new(ProviderStream::new(None));
        let mut tool_calls = Vec::new();
        for (index, (tool, args)) in calls.iter().enumerate() {
            let arguments = args.as_object().cloned().ok_or("args must be an object")?;
            tool_calls.push(faux_tool_call(&format!("call-{index}"), tool, arguments));
        }
        provider.queue_faux(vec![
            faux_assistant_message(tool_calls, StopReason::ToolUse),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]);
        let config = SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        };
        let mut session = AgentSession::new(config, provider);
        session.set_spills(self.root.join("spills"));
        let cwd = gate.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let sessions = self.root.join("sessions");
        let mut repo = JsonlRepo::new(sessions, cwd.to_string_lossy().into_owned());
        let n = SESSION.fetch_add(1, Ordering::Relaxed);
        let store = repo.create(CreateOptions {
            id: Some(format!("claude-hook-test-{n}")),
            parent_session_id: gate.child_of.as_ref().map(|_| "root-session".to_owned()),
            ..CreateOptions::default()
        })?;
        session.attach_store(store)?;
        let broker = gate.mode.map(|mode| {
            let origin = gate.child_of.clone().unwrap_or_else(|| cwd.clone());
            let events = session.events_sender();
            let mut broker = PermissionBroker::new(mode, origin, gate.rules, None, events);
            if gate.sandbox {
                let mut sandbox = yi_tools::Sandbox::for_workspace(&cwd, &self.home, None);
                sandbox.writable = vec![cwd.clone()];
                broker = broker.with_sandbox(Some(sandbox));
            }
            let broker = match &gate.child_of {
                Some(_) => broker.for_child(&yi_runtime::wall::Wall::default(), &cwd),
                None => broker,
            };
            for hold in gate.holds {
                broker.insert_hold(hold);
            }
            for dir in gate.kept {
                let grant = yi_permission::write_grant(&dir);
                broker.replay(
                    vec![yi_types::permission::SessionPermissionRule {
                        id: 0,
                        kind: grant.kind,
                        canonical: grant.canonical,
                        display_identity: grant.label,
                        decision: yi_types::permission::RuleDecision::Allow,
                        generation: 0,
                    }],
                    &yi_runtime::wall::Wall::default(),
                );
            }
            if let Some(journal) = gate.journal {
                broker.set_journal(journal);
            }
            broker
        });
        let mut tools = yi_tools::builtin_tools();
        for (name, output, fail) in [
            ("stub", stub_output, gate.fail_stub),
            ("ipython", "cell ok", false),
        ] {
            let output = output.to_owned();
            tools.push(Arc::new(FakeTool { name, output, fail }));
        }
        if gate.count_edit_changes {
            tools.retain(|tool| tool.name() != "edit");
            tools.push(Arc::new(FakeTool {
                name: "edit",
                output: "edited".to_owned(),
                fail: false,
            }));
        }
        session.use_tools(tools, cwd, broker.map(Arc::new));
        session.prompt("run the tools")?;
        if let Some(started) = &gate.abort_after {
            crate::support::wait_for_path(started, "the tool did not start").await?;
            session.abort();
        }
        session.wait_idle().await;
        let results = session
            .messages()
            .into_iter()
            .filter_map(|message| match message {
                AgentMessage::ToolResult {
                    content, is_error, ..
                } => Some(Outcome {
                    text: yi_types::message::join_text(&content, "\n"),
                    is_error,
                }),
                _ => None,
            });
        Ok(results.collect())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

#[tokio::test]
async fn pre_tool_use_exit_two_blocks_bash_and_returns_stderr() -> TestResult {
    let refusal = command("echo 'claude refusal text' >&2; exit 2");
    let case = Case::new(
        "exit-two",
        settings("PreToolUse", "Bash", vec![refusal]),
        true,
    )?;
    let marker = case.path("touched");
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run("bash", touch).await?;
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("claude refusal text"),
        "{}",
        outcome.text
    );
    assert!(!marker.exists(), "a denied command ran");
    Ok(())
}

#[tokio::test]
async fn a_deny_that_never_reads_stdin_still_denies_a_payload_past_the_pipe() -> TestResult {
    let refusal = command("echo 'too big to read' >&2; exit 2");
    let case = Case::new(
        "big-deny",
        settings("PreToolUse", "Write", vec![refusal]),
        true,
    )?;
    let args = json!({"path": "big.txt", "content": "y".repeat(256 * 1024)});
    let outcome = case.run("write", args).await?;
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains("too big to read"), "{}", outcome.text);
    assert!(
        !case.cwd.join("big.txt").exists(),
        "the denied write landed"
    );
    Ok(())
}

#[tokio::test]
async fn a_slow_hook_past_its_timeout_returns_within_the_bound() -> TestResult {
    let mut sleep = command("sleep 30");
    sleep["timeout"] = Value::from(1);
    let case = Case::new("slow", settings("PreToolUse", "Write", vec![sleep]), true)?;
    let started = std::time::Instant::now();
    let args = json!({"path": "slow.txt", "content": "z".repeat(256 * 1024)});
    let outcome = case.run("write", args).await?;
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
    assert!(
        outcome.text.contains("timed out after 1s"),
        "{}",
        outcome.text
    );
    assert!(
        !outcome.is_error,
        "a timed-out hook blocked: {}",
        outcome.text
    );
    Ok(())
}

#[tokio::test]
async fn tool_response_is_cut_past_its_cap_on_a_char_boundary() -> TestResult {
    let at = "a".repeat(65_536);
    let past = "a".repeat(65_537);
    let straddle = format!("{}é", "a".repeat(65_535));
    let cases = [
        ("at", &at, None),
        (
            "past",
            &past,
            Some((
                65_536,
                "kept 65536 of 65537 bytes of this tool_response field (RESPONSE_CAP 65536)",
            )),
        ),
        (
            "straddle",
            &straddle,
            Some((65_535, "kept 65535 of 65537 bytes")),
        ),
    ];
    for (label, output, cut) in cases {
        let case = Case::new(&format!("cap-{label}"), json!({}), true)?;
        let log = case.path("cap-input.jsonl");
        case.user(&settings(
            "PostToolUse",
            "mcp__yi__stub",
            vec![logging(&log)],
        ))?;
        (case.run_calls(vec![("stub", json!({}))], Gate::default(), output)).await?;
        let lines = read_lines(&log)?;
        let first = lines.first().ok_or("no post input")?;
        let response = first["tool_response"]
            .as_str()
            .ok_or("missing tool_response")?;
        match cut {
            None => assert_eq!(response, at.as_str(), "a response at the cap was cut"),
            Some((kept, note)) => {
                let (head, row) = response.split_once('\n').ok_or("no cut row")?;
                assert_eq!(head, "a".repeat(kept), "{label}");
                assert!(row.contains(note), "{label}: {row}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_hook_allow_never_lifts_a_configured_deny_rule() -> TestResult {
    let allow = settings("PreToolUse", "Bash", vec![decision("allow")]);
    let case = Case::new("allow-deny-rule", allow, true)?;
    let marker = case.cwd.join("ran");
    let deny = yi_runtime::ConfigRuleAction::Deny;
    let gate = Gate {
        rules: vec![yi_runtime::ConfigRule::new("bash", "*touch*", deny)?],
        ..asking(PermissionMode::Ask)
    };
    let touch = json!({"command": format!("touch {}", marker.display())});
    let outcome = case.run_gated("bash", touch, gate).await?;
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("denied by configured rule"),
        "{}",
        outcome.text
    );
    assert!(!marker.exists(), "a hook allow lifted a deny rule");
    Ok(())
}

#[tokio::test]
async fn a_hook_ask_is_put_to_the_broker_and_refused_headless() -> TestResult {
    let case = Case::new(
        "ask",
        settings("PreToolUse", "Bash", vec![decision("ask")]),
        true,
    )?;
    let gate = asking(PermissionMode::Auto);
    let outcome = case
        .run_gated("bash", json!({"command": "printf asked"}), gate)
        .await?;
    assert!(
        outcome.is_error,
        "an auto-allowed call skipped the ask: {}",
        outcome.text
    );
    let asked = "a Claude Code PreToolUse hook asks (ask reason)";
    assert!(outcome.text.contains(asked), "{}", outcome.text);
    assert!(!outcome.text.starts_with("asked"), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn handlers_combine_deny_over_ask_over_allow_and_silence_changes_nothing() -> TestResult {
    let silent = command("printf '{}'");
    let cases = [
        (
            "allow-silent",
            vec![decision("allow"), silent.clone()],
            None,
        ),
        (
            "silent-allow",
            vec![silent.clone(), decision("allow")],
            None,
        ),
        (
            "ask-allow",
            vec![decision("allow"), decision("ask")],
            Some("ask reason"),
        ),
        (
            "deny-all",
            vec![decision("allow"), decision("deny"), decision("ask")],
            Some("deny reason"),
        ),
    ];
    for (label, handlers, refused) in cases {
        let case = Case::new(label, settings("PreToolUse", "Bash", handlers), true)?;
        let marker = case.cwd.join("ran");
        let touch = json!({"command": format!("touch {}", marker.display())});
        let outcome = case
            .run_gated("bash", touch, asking(PermissionMode::Ask))
            .await?;
        match refused {
            None => assert!(
                marker.exists(),
                "{label}: the allow was lost: {}",
                outcome.text
            ),
            Some(reason) => {
                assert!(!marker.exists(), "{label}: ran");
                assert!(outcome.text.contains(reason), "{label}: {}", outcome.text);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn matchers_select_by_name_list_regex_and_star() -> TestResult {
    let case = Case::new("matchers", json!({}), true)?;
    let marker = |name: &str| case.cwd.join(name);
    let group =
        |matcher: &str, name: &str| json!({"matcher": matcher, "hooks": [touching(&marker(name))]});
    case.user(&json!({"hooks": {"PreToolUse": [
        group("Write", "write-ran"),
        group("*", "star-ran"),
        group("Edit|Bash", "list-ran"),
        group("^Ba", "regex-ran")
    ]}}))?;
    let outcome = case.run("bash", json!({"command": "printf ok"})).await?;
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(
        !marker("write-ran").exists(),
        "a Write matcher ran for Bash"
    );
    for name in ["star-ran", "list-ran", "regex-ran"] {
        assert!(marker(name).exists(), "{name} missing");
    }
    Ok(())
}

#[tokio::test]
async fn each_tool_reaches_its_hook_in_claude_codes_name_and_input_shape() -> TestResult {
    let case = Case::new("shapes", json!({}), true)?;
    let log = case.path("inputs.jsonl");
    case.user(&settings("PreToolUse", "*", vec![logging(&log)]))?;
    std::fs::write(case.cwd.join("a.txt"), "one\ntwo\n")?;
    let abs = |name: &str| case.cwd.join(name).to_string_lossy().into_owned();
    let patch = "[a.txt#ZZZZ]\nREM 1\n";
    let grep = json!({"pattern": "one", "path": ".", "include": "*.txt", "ignore_case": true, "count": true});
    let calls = vec![
        ("bash", json!({"command": "printf hi", "timeout_secs": 5})),
        ("read", json!({"path": "a.txt", "offset": 1, "limit": 2})),
        ("write", json!({"path": "b.txt", "content": "new"})),
        ("edit", json!({"patch": patch})),
        ("grep", grep),
        (
            "grep",
            json!({"pattern": "one", "replace": "uno", "apply": true, "path": "a.txt"}),
        ),
        ("ipython", json!({"code": "1 + 1"})),
    ];
    (case.run_calls(calls, Gate::default(), "x")).await?;
    let lines = read_lines(&log)?;
    let seen: Vec<(&Value, &Value)> = (lines.iter())
        .map(|line| (&line["tool_name"], &line["tool_input"]))
        .collect();
    let grep_input = json!({"pattern": "one", "path": case.cwd.to_string_lossy(), "glob": "*.txt", "-i": true, "output_mode": "count"});
    let rewrite = json!({"old_string": "one", "new_string": "uno", "file_path": abs("a.txt"), "replace_all": true});
    let expected = [
        (
            json!("Bash"),
            json!({"command": "printf hi", "timeout": 5000}),
        ),
        (
            json!("Read"),
            json!({"file_path": abs("a.txt"), "offset": 1, "limit": 2}),
        ),
        (
            json!("Write"),
            json!({"file_path": abs("b.txt"), "content": "new"}),
        ),
        (
            json!("Edit"),
            json!({"file_path": abs("a.txt"), "patch": patch}),
        ),
        (json!("Grep"), grep_input),
        (json!("Edit"), rewrite),
        (json!("mcp__yi__ipython"), json!({"code": "1 + 1"})),
    ];
    let expected: Vec<(&Value, &Value)> =
        expected.iter().map(|(name, input)| (name, input)).collect();
    assert_eq!(seen, expected);
    let ipython = lines.last().ok_or("no ipython input")?;
    assert_eq!(
        ipython["mcp_server"],
        json!({"name": "yi", "source": "sdk"})
    );
    let doc = read_json(&fixture("doc-pretooluse-bash.json"))?;
    let bash = lines.first().ok_or("no bash input")?;
    for (key, value) in bash.as_object().ok_or("not an object")? {
        let known = doc
            .get(key)
            .ok_or_else(|| format!("{key} is not in Claude Code's payload"))?;
        let shaped = std::mem::discriminant(value) == std::mem::discriminant(known);
        assert!(shaped, "{key}: {value} is not shaped like {known}");
    }
    for key in bash["tool_input"]
        .as_object()
        .ok_or("no tool_input")?
        .keys()
    {
        assert!(
            doc["tool_input"].get(key).is_some(),
            "Bash input {key} is not Claude Code's"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_failure_hook_that_blocks_adds_its_reason_to_the_failed_result() -> TestResult {
    let case = Case::new("failure", json!({}), true)?;
    let log = case.path("failure.jsonl");
    let block = printing(&json!({"decision": "block", "reason": "fail block"}));
    case.user(&json!({"hooks": {
        "PostToolUse": [{"matcher": "Bash", "hooks": [touching(&case.cwd.join("success-ran"))]}],
        "PostToolUseFailure": [{"matcher": "Bash", "hooks": [
            logging(&log), command("echo 'fail stderr' >&2; exit 2"), block
        ]}]
    }}))?;
    let outcome = case
        .run("bash", json!({"command": "printf boom; exit 7"}))
        .await?;
    assert!(
        outcome.text.starts_with("boom\nexit code: 7"),
        "{}",
        outcome.text
    );
    for row in [
        "[Claude Code hook feedback: fail stderr]",
        "[Claude Code hook feedback: fail block]",
    ] {
        assert!(outcome.text.contains(row), "{}", outcome.text);
    }
    assert!(
        !case.cwd.join("success-ran").exists(),
        "PostToolUse ran for a failure"
    );
    let lines = read_lines(&log)?;
    let input = lines.first().ok_or("no failure input")?;
    let error = input["error"].as_str().ok_or("no error")?;
    assert!(error.starts_with("Exit code 7\nboom"), "{error}");
    assert_eq!(input["is_interrupt"], false);
    assert!(input.get("tool_response").is_none());
    Ok(())
}

#[tokio::test]
async fn an_interrupted_tool_reaches_post_failure_with_is_interrupt() -> TestResult {
    let case = Case::new("interrupt", json!({}), true)?;
    let log = case.path("interrupt.jsonl");
    case.user(&settings("PostToolUseFailure", "Bash", vec![logging(&log)]))?;
    let started = case.path("started");
    let gate = Gate {
        abort_after: Some(started.clone()),
        ..Gate::default()
    };
    let command = format!("touch '{}'; sleep 5", started.display());
    case.run_gated("bash", json!({"command": command}), gate)
        .await?;
    let lines = read_lines(&log)?;
    let input = lines.first().ok_or("no interrupted failure input")?;
    assert_eq!(input["hook_event_name"], "PostToolUseFailure");
    assert_eq!(input["is_interrupt"], true);
    Ok(())
}

#[tokio::test]
async fn disable_all_hooks_follows_settings_precedence() -> TestResult {
    let case = Case::new("disable", json!({"disableAllHooks": true}), true)?;
    let marker = case.cwd.join("hook-ran");
    let hook = settings("PreToolUse", "*", vec![touching(&marker)]);
    write_json(&case.cwd.join(".claude/settings.json"), &hook)?;
    let plugin = case.path("plugin/hooks/hooks.json");
    write_json(&plugin, &hook)?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe { std::env::set_var("YI_CLAUDE_HOOKS_FILES", &plugin) };
    case.trust(&case.cwd)?;
    case.run("bash", json!({"command": "printf off"})).await?;
    assert!(
        !marker.exists(),
        "a user disableAllHooks left a project or plugin hook on"
    );
    let local = json!({"disableAllHooks": false});
    write_json(&case.cwd.join(".claude/settings.local.json"), &local)?;
    case.trust(&case.cwd)?;
    case.run("bash", json!({"command": "printf on"})).await?;
    assert!(
        marker.exists(),
        "a local false did not override the user's true"
    );
    Ok(())
}

#[tokio::test]
async fn an_untrusted_project_file_is_skipped_loudly_until_yi_trust_grants_it() -> TestResult {
    let case = Case::new("trust", json!({}), true)?;
    let marker = case.cwd.join("project-hook-ran");
    let hook = settings("PreToolUse", "Bash", vec![touching(&marker)]);
    write_json(&case.cwd.join(".claude/settings.json"), &hook)?;
    let first = case.run("bash", json!({"command": "printf one"})).await?;
    assert!(!marker.exists(), "an untrusted project hook ran");
    assert!(first.text.contains("run `yi trust` in"), "{}", first.text);
    case.trust(&case.cwd)?;
    let second = case.run("bash", json!({"command": "printf two"})).await?;
    assert!(
        marker.exists(),
        "a granted project hook did not run: {}",
        second.text
    );
    assert!(!second.text.contains("yi trust"), "{}", second.text);
    Ok(())
}

#[tokio::test]
async fn a_child_in_a_lane_runs_the_root_projects_hooks() -> TestResult {
    let case = Case::new("lane", json!({}), true)?;
    let (lane, log, seen) = (
        case.path("lane"),
        case.path("lane.jsonl"),
        case.path("lane-env"),
    );
    let project_hook = command(&format!(
        "cat >> '{0}'; echo >> '{0}'; printf '%s' \"$CLAUDE_PROJECT_DIR\" > '{1}'",
        log.display(),
        seen.display()
    ));
    let project = settings("PreToolUse", "*", vec![project_hook]);
    write_json(&case.cwd.join(".claude/settings.json"), &project)?;
    let lane_marker = case.path("lane-hook-ran");
    let own = settings("PreToolUse", "*", vec![touching(&lane_marker)]);
    write_json(&lane.join(".claude/settings.json"), &own)?;
    case.trust(&case.cwd)?;
    case.trust(&lane)?;
    let gate = Gate {
        cwd: Some(lane.clone()),
        child_of: Some(case.cwd.clone()),
        ..asking(PermissionMode::Yolo)
    };
    case.run_gated("bash", json!({"command": "printf child"}), gate)
        .await?;
    assert!(!lane_marker.exists(), "a child read its lane's settings");
    assert_eq!(std::fs::read_to_string(&seen)?, case.cwd.to_string_lossy());
    let lines = read_lines(&log)?;
    let input = lines.first().ok_or("no child input")?;
    assert_eq!(input["cwd"], lane.to_string_lossy().as_ref());
    let id = input["agent_id"].as_str();
    assert!(
        id.is_some_and(|id| id.starts_with("claude-hook-test-")),
        "{input}"
    );
    assert_eq!(input["permission_mode"], "bypassPermissions");
    Ok(())
}

#[tokio::test]
async fn plugin_root_and_project_dir_reach_a_real_plugins_hooks() -> TestResult {
    let case = Case::new("plugin-env", json!({}), true)?;
    let plugin = case.path("hookify");
    std::fs::create_dir_all(plugin.join("hooks"))?;
    std::fs::copy(
        fixture("hookify-hooks.json"),
        plugin.join("hooks/hooks.json"),
    )?;
    let seen = case.path("plugin-seen");
    let script = format!(
        "import os, sys\nopen({:?}, 'w').write(os.environ['CLAUDE_PLUGIN_ROOT'] + '|' + os.environ['CLAUDE_PROJECT_DIR'])\nsys.stderr.write('hookify says no')\nsys.exit(2)\n",
        seen.display().to_string()
    );
    std::fs::write(plugin.join("hooks/pretooluse.py"), script)?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe { std::env::set_var("YI_CLAUDE_HOOKS_FILES", plugin.join("hooks/hooks.json")) };
    let exec_seen = case.path("exec-seen");
    let check = format!(
        "printf '%s' \"$1|$CLAUDE_PROJECT_DIR\" > '{}'\n",
        exec_seen.display()
    );
    std::fs::write(case.cwd.join("check.sh"), check)?;
    let args = ["${CLAUDE_PROJECT_DIR}/check.sh", "${CLAUDE_PROJECT_DIR}"];
    let exec = json!({"type": "command", "command": "/bin/sh", "args": args});
    case.user(&settings("PreToolUse", "Bash", vec![exec]))?;
    let outcome = case.run("bash", json!({"command": "printf ran"})).await?;
    assert!(outcome.text.contains("hookify says no"), "{}", outcome.text);
    let (root, project) = (plugin.to_string_lossy(), case.cwd.to_string_lossy());
    assert_eq!(std::fs::read_to_string(&seen)?, format!("{root}|{project}"));
    assert_eq!(
        std::fs::read_to_string(&exec_seen)?,
        format!("{project}|{project}")
    );
    assert!(
        !outcome.text.contains("UserPromptSubmit"),
        "the plugin's non-tool hooks reached the tool result: {}",
        outcome.text
    );
    Ok(())
}

#[tokio::test]
async fn updated_input_is_applied_where_it_maps_back_and_refused_where_not() -> TestResult {
    let rewrite = |input: Value| {
        let output =
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "updatedInput": input}});
        settings("PreToolUse", "Bash|Edit", vec![printing(&output)])
    };
    let mapped = rewrite(json!({"command": "printf rewritten", "timeout": 5000}));
    let case = Case::new("rewrite", mapped, true)?;
    let outcome = case
        .run("bash", json!({"command": "printf original"}))
        .await?;
    assert!(outcome.text.starts_with("rewritten"), "{}", outcome.text);
    case.user(&rewrite(json!({"command": "printf odd", "timeout": 1500})))?;
    let refused = case
        .run("bash", json!({"command": "printf original"}))
        .await?;
    assert!(refused.is_error, "{}", refused.text);
    let why = "its updatedInput cannot be applied: timeout";
    assert!(refused.text.contains(why), "{}", refused.text);
    assert!(!refused.text.starts_with("odd") && !refused.text.starts_with("original"));
    std::fs::write(case.cwd.join("a.txt"), "one\n")?;
    let apply = json!({"pattern": "one", "replace": "uno", "apply": true, "path": "a.txt"});
    let refused = case.run("grep", apply).await?;
    assert!(
        refused
            .text
            .contains("a grep rewrite has no Claude Code input"),
        "{}",
        refused.text
    );
    assert_eq!(std::fs::read_to_string(case.cwd.join("a.txt"))?, "one\n");
    let malformed = printing(&json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "updatedInput": {"output_mode": true}
    }}));
    case.user(&settings("PreToolUse", "Grep", vec![malformed]))?;
    let refused = case
        .run("grep", json!({"pattern": "one", "path": "a.txt"}))
        .await?;
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("output_mode"), "{}", refused.text);
    Ok(())
}

#[tokio::test]
async fn additional_context_from_pre_and_post_rides_the_result() -> TestResult {
    let context = |event: &str, text: &str| {
        printing(
            &json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}),
        )
    };
    let config = json!({"hooks": {
        "PreToolUse": [{"matcher": "Bash", "hooks": [context("PreToolUse", "pre ctx")]}],
        "PostToolUse": [{"matcher": "Bash", "hooks": [context("PostToolUse", "post ctx")]}]
    }});
    let case = Case::new("context", config, true)?;
    let outcome = case.run("bash", json!({"command": "printf body"})).await?;
    let rows = "body\n[Claude Code hook context: pre ctx]\n[Claude Code hook context: post ctx]";
    assert!(outcome.text.starts_with(rows), "{}", outcome.text);
    Ok(())
}

#[tokio::test]
async fn post_tool_use_block_appends_hook_feedback() -> TestResult {
    let block = printing(&json!({"decision": "block", "reason": "post hook feedback"}));
    let case = Case::new("post-block", json!({}), true)?;
    let log = case.path("post-input.jsonl");
    case.user(&settings("PostToolUse", "Bash", vec![logging(&log), block]))?;
    let outcome = case
        .run("bash", json!({"command": "printf tool-output"}))
        .await?;
    assert!(!outcome.is_error, "{}", outcome.text);
    let fed = "tool-output\n[Claude Code hook feedback: post hook feedback]";
    assert!(outcome.text.starts_with(fed), "{}", outcome.text);
    let lines = read_lines(&log)?;
    let input = lines.first().ok_or("no post input")?;
    assert_eq!(input["hook_event_name"], "PostToolUse");
    let response =
        json!({"stdout": "tool-output", "stderr": "", "interrupted": false, "isImage": false});
    assert_eq!(input["tool_response"], response);
    Ok(())
}

#[tokio::test]
async fn json_deny_from_an_exec_form_plugin_hook_blocks_write() -> TestResult {
    let case = Case::new("json-deny", json!({}), true)?;
    let (plugin, input) = (case.path("plugin-hooks.json"), case.path("pre-input.json"));
    let deny = json!({"hookSpecificOutput": {"permissionDecision": "deny", "permissionDecisionReason": "write denied by JSON"}});
    let script = format!("cat > '{}'; printf '%s' '{deny}'", input.display());
    let exec = json!({"type": "command", "command": "/bin/sh", "args": ["-c", script]});
    write_json(&plugin, &settings("PreToolUse", "Write", vec![exec]))?;
    // SAFETY: nextest isolates each test in its own process.
    unsafe { std::env::set_var("YI_CLAUDE_HOOKS_FILES", &plugin) };
    let outcome = case
        .run("write", json!({"path": "denied.txt", "content": "secret"}))
        .await?;
    assert!(outcome.is_error, "{}", outcome.text);
    assert!(
        outcome.text.contains("write denied by JSON"),
        "{}",
        outcome.text
    );
    assert!(!case.cwd.join("denied.txt").exists());
    let input = read_json(&input)?;
    let id = input["session_id"].as_str();
    assert!(
        id.is_some_and(|id| id.starts_with("claude-hook-test-")),
        "{input}"
    );
    assert_eq!(input["tool_use_id"], "call-0");
    assert_eq!(input["cwd"], case.cwd.to_string_lossy().as_ref());
    assert!(input.get("agent_id").is_none() && input.get("permission_mode").is_none());
    let transcript = input["transcript_path"].as_str();
    assert!(
        transcript.is_some_and(|path| path.ends_with(".jsonl")),
        "{input}"
    );
    Ok(())
}

#[tokio::test]
async fn a_hook_allow_runs_contained_and_the_sandbox_still_refuses_an_escape() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let allow = settings("PreToolUse", "Bash", vec![decision("allow")]);
    let case = Case::new("allow-sandbox", allow, true)?;
    let escape = case.home.join("escape.txt");
    let gate = Gate {
        sandbox: true,
        ..asking(PermissionMode::Ask)
    };
    let command = format!("touch in-cwd-ok && printf x > {}", escape.display());
    let outcome = case
        .run_gated("bash", json!({"command": command}), gate)
        .await?;
    assert!(
        case.cwd.join("in-cwd-ok").exists(),
        "the allow did not run it: {}",
        outcome.text
    );
    assert!(
        !escape.exists(),
        "a hook allow escaped the sandbox: {}",
        outcome.text
    );
    assert!(
        outcome.text.contains("Operation not permitted"),
        "{}",
        outcome.text
    );
    Ok(())
}

#[tokio::test]
async fn a_hook_allow_never_widens_an_in_process_write_past_the_sandbox() -> TestResult {
    let allow = settings("PreToolUse", "Write", vec![decision("allow")]);
    let case = Case::new("allow-write", allow, true)?;
    let outside = case.home.join("outside.txt");
    let gate = || Gate {
        sandbox: true,
        ..asking(PermissionMode::Ask)
    };
    let args = json!({"path": outside.to_string_lossy(), "content": "x"});
    let outcome = case.run_gated("write", args, gate()).await?;
    assert!(
        !outside.exists(),
        "a hook allow wrote past the sandbox: {}",
        outcome.text
    );
    assert!(
        outcome.text.contains("does not widen the sandbox"),
        "{}",
        outcome.text
    );
    let args = json!({"path": "inside.txt", "content": "x"});
    let inside = case.run_gated("write", args, gate()).await?;
    assert!(case.cwd.join("inside.txt").exists(), "{}", inside.text);
    Ok(())
}

#[tokio::test]
async fn regex_matcher_maps_ipython_to_claudes_mcp_name() -> TestResult {
    let case = Case::new("regex-ipython", json!({}), true)?;
    let marker = case.cwd.join("ipython-hook-ran");
    case.user(&settings(
        "PreToolUse",
        "mcp__yi__.*",
        vec![touching(&marker)],
    ))?;
    let outcome = case.run("ipython", json!({"code": "1 + 1"})).await?;
    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(marker.exists(), "mcp__yi__.* did not match ipython");
    Ok(())
}

#[tokio::test]
async fn no_hook_runs_with_the_feature_off_and_one_runs_with_it_on() -> TestResult {
    for enabled in [false, true] {
        let case = Case::new("env", json!({}), enabled)?;
        let marker = case.cwd.join("env-hook-ran");
        case.user(&settings("PreToolUse", "*", vec![touching(&marker)]))?;
        case.run("bash", json!({"command": "printf env"})).await?;
        assert_eq!(marker.exists(), enabled, "YI_CLAUDE_HOOKS on: {enabled}");
    }
    Ok(())
}

#[tokio::test]
async fn hook_start_and_timeout_failures_are_reported_once_without_blocking() -> TestResult {
    let mut sleep = command("sleep 2");
    sleep["timeout"] = Value::from(1);
    let missing =
        json!({"type": "command", "command": "/definitely/missing/claude-hook", "args": []});
    let prompt = json!({"type": "prompt", "prompt": "judge"});
    let config = settings("PreToolUse", "Bash", vec![missing, sleep, prompt]);
    let case = Case::new("hook-errors", config, true)?;
    let calls = vec![
        ("bash", json!({"command": "printf one"})),
        ("bash", json!({"command": "printf two"})),
    ];
    let outcomes = case.run_calls(calls, Gate::default(), "x").await?;
    let [first, second] = outcomes.as_slice() else {
        return Err("expected two results".into());
    };
    for outcome in [first, second] {
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(outcome.text.contains("failed to start"), "{}", outcome.text);
        assert!(
            outcome.text.contains("timed out after 1s"),
            "{}",
            outcome.text
        );
    }
    assert!(
        first.text.contains("type prompt is not supported"),
        "{}",
        first.text
    );
    assert!(
        !second.text.contains("type prompt"),
        "a config notice repeated: {}",
        second.text
    );
    Ok(())
}

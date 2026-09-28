#![cfg(target_os = "macos")]

use crate::kernel_sandbox::uncovered;
use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, PermissionBroker, PermissionMode, ProviderStream, SessionConfig, builtin_tools,
};
use yi_tools::Sandbox;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
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

fn bash_call(id: &str, command: &str) -> AgentMessage {
    let mut arguments = Map::new();
    arguments.insert("command".to_owned(), json!(command));
    faux_assistant_message(
        vec![faux_tool_call(id, "bash", arguments)],
        StopReason::ToolUse,
    )
}

fn results(session: &AgentSession) -> Vec<String> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect()
}

/// One turn of queued bash calls under `sandbox`, with no asker: a question reads as a denial.
async fn run_contained(
    project: &Path,
    sandbox: Sandbox,
    commands: &[&str],
) -> Result<Vec<String>, Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(
        commands
            .iter()
            .enumerate()
            .map(|(index, command)| bash_call(&format!("call-{index}"), command))
            .collect(),
    );
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let broker = Arc::new(
        PermissionBroker::new(
            PermissionMode::Auto,
            project.to_path_buf(),
            Vec::new(),
            None,
            session.events_sender(),
        )
        .with_sandbox(Some(sandbox)),
    );
    session.use_tools(builtin_tools(), project.to_path_buf(), Some(broker));
    // One turn runs every call: the loop answers each tool result with the next queued message.
    session.prompt("do the thing")?;
    session.wait_idle().await;
    let results = results(&session);
    assert_eq!(results.len(), commands.len(), "{results:?}");
    Ok(results)
}

fn confined_to(project: &Path) -> Sandbox {
    Sandbox {
        writable: vec![project.to_path_buf()],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
        loopback: false,
    }
}

/// The production profile over a scratch tree, and a directory it does not cover: tmp is
/// writable there, so a probe under it would pass vacuously.
fn workspace(tag: &str) -> Result<(Scratch, PathBuf, Sandbox, PathBuf), Box<dyn Error>> {
    let root = Scratch::new(tag)?;
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?;
    let sandbox = Sandbox::for_workspace(&project, &home, None);
    let probe = uncovered(&sandbox, &home).ok_or("no writable directory outside the sandbox")?;
    Ok((root, project, sandbox, probe))
}

/// The dogfood command of 2026-09-27, its two paths moved to a writable dir and the probe dir.
fn dogfood(tmp: &Path, refused: &Path) -> String {
    format!(
        "yes | head -5; echo x > {}/sandbox_probe && echo wrote-tmp; echo y > {} && echo wrote-home",
        tmp.display(),
        refused.display()
    )
}

/// The seam, end to end: the broker contains an unknown command, the adapter
/// hands the sandbox to the tool, the command runs with no prompt and cannot
/// reach past the working tree, and the same command asks the second time
/// because containment already refused it once.
#[tokio::test]
async fn an_unknown_command_runs_contained_then_asks() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let escape = home.join("escaped.txt");
    let command = format!("printf x > {}", escape.display());
    let results = run_contained(&project, confined_to(&project), &[&command, &command]).await?;
    assert!(
        !escape.exists(),
        "the contained command must not write outside the tree"
    );
    assert!(
        !results[0].contains("Permission denied"),
        "containment runs instead of asking: {}",
        results[0]
    );
    assert!(
        results[0].contains(&format!(
            "the sandbox refused writing `{}`",
            escape.display()
        )),
        "a denial explains itself: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied"),
        "the second attempt asks rather than repeating the denial: {}",
        results[1]
    );
    Ok(())
}

/// The incident: the retry appended `&& git status | wc -l`, so an exact-text memory of the
/// refusal never matched and the second attempt was contained again instead of asking.
#[tokio::test]
async fn a_refused_program_asks_even_when_the_retry_text_differs() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam-scope")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let first = format!("mkdir {}", home.join("a").display());
    let second = format!("mkdir {} && ls", home.join("b").display());
    let results = run_contained(&project, confined_to(&project), &[&first, &second]).await?;
    assert!(
        results[0].contains(&format!("writing under `{}`", home.display())),
        "the hint names what will ask: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied") && !home.join("b").exists(),
        "a different command writing beside the refused path asks: {}",
        results[1]
    );
    Ok(())
}

/// The dogfood refusal: `yes` led the command, so the hint blamed `yes`, while the sandbox had
/// refused the redirect into the home directory.
#[tokio::test]
async fn the_hint_names_the_refused_path_not_the_first_program() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-path")?;
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let refused = probe.join(format!("yidog_probe-{}", std::process::id()));
    let results = run_contained(&project, sandbox, &[&dogfood(&tmp, &refused)]).await?;
    assert!(!refused.exists(), "the home write must be refused");
    assert!(
        results[0].contains(&format!("refused writing `{}`", refused.display()))
            && !results[0].contains("`yes`"),
        "the hint names the path the sandbox refused, not the first program: {}",
        results[0]
    );
    Ok(())
}

/// Remembering `yes` made the next harmless `yes` ask while the failing write ran contained
/// again; the memory follows the refused path instead.
#[tokio::test]
async fn a_retry_of_the_refused_write_asks_and_an_unrelated_echo_does_not() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-retry")?;
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let refused = probe.join(format!("yidog_retry-{}", std::process::id()));
    let unrelated = format!("yes | head -1 && echo hi > {}/unrelated", tmp.display());
    let retry = format!("echo y > {}", refused.display());
    let results = run_contained(
        &project,
        sandbox,
        &[&dogfood(&tmp, &refused), &unrelated, &retry],
    )
    .await?;
    assert!(
        !results[1].contains("Permission denied") && tmp.join("unrelated").exists(),
        "a call writing nowhere near the refused path still runs contained: {}",
        results[1]
    );
    assert!(
        results[2].contains("Permission denied") && !refused.exists(),
        "the retry of the refused write asks instead of failing contained again: {}",
        results[2]
    );
    Ok(())
}

/// `tail` exits 0, so the refusal of `touch` before it hid behind the pipe's exit code.
#[tokio::test]
async fn a_refusal_under_a_zero_exit_pipe_is_detected() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-pipe")?;
    let refused = probe.join(format!("yidog_touch-{}", std::process::id()));
    let command = format!("touch {} | tail -1", refused.display());
    let results = run_contained(&project, sandbox, &[&command]).await?;
    assert!(!refused.exists(), "the touch must be refused");
    assert!(
        results[0].contains(&format!("refused writing `{}`", refused.display())),
        "a refusal behind a zero exit still explains itself: {}",
        results[0]
    );
    Ok(())
}

/// The spellings a model reaches for second: `$HOME`, then the path quoted inside python. Both
/// ran contained again while the hint promised a question.
#[tokio::test]
async fn a_retry_spelled_through_home_or_python_asks() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (root, project, sandbox, probe) = workspace("yi-seam-spell")?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // Under a tmp HOME the probe is `/Users/Shared`, which no home spelling reaches.
    if home.as_deref() != Some(probe.as_path()) {
        return Ok(());
    }
    let tmp = root.join("yidog");
    std::fs::create_dir_all(&tmp)?;
    let name = format!("yidog_spell-{}", std::process::id());
    let refused = probe.join(&name);
    let via_home = format!("echo y > $HOME/{name}");
    let via_python = format!(
        "python3 -c \"open('{}','w').write('y')\"",
        refused.display()
    );
    let results = run_contained(
        &project,
        sandbox,
        &[&dogfood(&tmp, &refused), &via_home, &via_python],
    )
    .await?;
    for (retry, result) in [&via_home, &via_python].iter().zip(&results[1..]) {
        assert!(
            result.contains("Permission denied") && !refused.exists(),
            "{retry} asks instead of failing contained again: {result}"
        );
    }
    Ok(())
}

/// A refusal in the middle of long output: the reducer keeps the head and the tail, so a broker
/// reading the reduced text found nothing and the retry ran contained again.
#[tokio::test]
async fn a_refusal_deep_in_long_output_is_remembered() -> TestResult {
    if !Sandbox::available() {
        return Ok(());
    }
    let (_root, project, sandbox, probe) = workspace("yi-seam-long")?;
    let refused = probe.join(format!("yidog_long-{}.log", std::process::id()));
    let noise = |from: u32, to: u32| {
        format!(
            "seq -f 'test {from}-{to} case %g passed in 0.01s, nothing to report here' {from} {to} >&2"
        )
    };
    let first = format!(
        "{}; echo y > {}; {}",
        noise(1, 150),
        refused.display(),
        noise(152, 300)
    );
    let retry = format!("printf y > {}", refused.display());
    let results = run_contained(&project, sandbox, &[&first, &retry]).await?;
    let errno = format!("{}: Operation not permitted", refused.display());
    assert!(
        !results[0].contains(&errno) && results[0].contains("refused writing"),
        "the reducer cut the refusal line, and the hint read it from the raw output: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied") && !refused.exists(),
        "the retry asks: {}",
        results[1]
    );
    Ok(())
}

/// Containment is for what the classifier could not read; ordinary work still
/// runs free.
#[test]
fn a_safe_command_is_never_contained() -> TestResult {
    let dir = Scratch::new("yi-seam-safe")?;
    let report = yi_runtime::gate::explain("git status && ls", PermissionMode::Auto, &dir);
    assert_eq!(report.outcome(), "allow");
    assert_eq!(report.to_json()["sandboxed"], false);
    Ok(())
}

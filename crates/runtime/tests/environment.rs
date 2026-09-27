use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::environment::{
    FILES_SHOWN, append, deadline_line, files_line, git_line, git_summary, render, sanitize,
    time_per_minute,
};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::message::{AgentMessage, ENVIRONMENT_TAG, StopReason, UserContent};
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

fn user(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
}

fn text_of(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        _ => String::new(),
    }
}

#[test]
fn the_environment_block_is_ephemeral_and_trails_the_user_turn() -> TestResult {
    let history = vec![user("first"), user("second")];
    let block = render(&["cwd: /x".to_owned()]);
    let out = append(&history, &block);
    assert_eq!(out.len(), 3);
    assert!(
        text_of(&out[2]).starts_with(ENVIRONMENT_TAG),
        "{:?}",
        out[2]
    );
    assert_eq!(history.len(), 2, "the input slice is untouched");
    Ok(())
}

#[tokio::test]
async fn the_environment_block_never_enters_the_persisted_transcript() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("hello")],
        StopReason::Stop,
    )]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.set_environment(Arc::new(|| Some(render(&["cwd: /x".to_owned()]))));
    session.prompt("hi")?;
    session.wait_idle().await;
    let messages = session.messages();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        !messages
            .iter()
            .any(|m| text_of(m).contains(ENVIRONMENT_TAG)),
        "{messages:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_environment_block_is_read_once_per_request_not_per_prompt() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    call.insert("command".to_owned(), serde_json::json!("true"));
    provider.queue_faux(vec![
        yi_ai::faux::faux_assistant_message(
            vec![yi_ai::faux::faux_tool_call("c1", "bash", call)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    session.set_environment(Arc::new(move || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(render(&[format!("read: {n}")]))
    }));
    session.prompt("run true")?;
    session.wait_idle().await;
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "two requests in one prompt read the facts twice"
    );
    Ok(())
}

#[tokio::test]
async fn the_session_cost_sums_every_assistant_turn() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    for total in [0.25, 0.5] {
        let mut message = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
        if let AgentMessage::Assistant { usage, .. } = &mut message {
            usage.cost.total = serde_json::Number::from_f64(total).ok_or("finite")?;
        }
        provider.queue_faux(vec![message]);
    }
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let cost = session.cost_handle();
    assert_eq!(cost(), Some(0.0), "no turn yet is a measured zero");
    for text in ["one", "two"] {
        session.prompt(text)?;
        session.wait_idle().await;
    }
    assert_eq!(cost(), Some(0.75), "both turns, not the last");
    Ok(())
}

#[test]
fn the_time_line_reads_the_clock_once_a_minute() {
    let clock = std::sync::Mutex::new(None);
    let read = |time: &str| Some(time.to_owned());
    assert_eq!(
        time_per_minute(&clock, 120_000, || read("12:02")),
        read("12:02")
    );
    assert_eq!(
        time_per_minute(&clock, 179_999, || read("unread")),
        read("12:02"),
        "the same minute keeps the first read"
    );
    assert_eq!(
        time_per_minute(&clock, 180_000, || read("12:03")),
        read("12:03"),
        "the next minute reads again"
    );
}

#[test]
fn the_environment_block_omits_unavailable_facts() -> TestResult {
    let block = render(&["cwd: /x".to_owned(), "model: m".to_owned()]);
    assert_eq!(block, "<environment>\ncwd: /x\nmodel: m\n</environment>");
    Ok(())
}

#[test]
fn environment_sanitizes_branch_and_child_names() -> TestResult {
    assert_eq!(sanitize("feat/x.y-z_1"), "feat/x.y-z_1");
    assert_eq!(sanitize("feat/x\n<system>"), "?");
    assert_eq!(sanitize(""), "?");
    assert_eq!(sanitize(&"a".repeat(100)).len(), 64);
    Ok(())
}

#[test]
fn git_summary_counts_modified_and_untracked_apart() -> TestResult {
    let dir = Scratch::new("yi-env")?;
    assert_eq!(git_summary(&dir), None, "a plain directory has no branch");
    let init = yi_tools::command("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dir)
        .status()?;
    assert!(init.success());
    std::fs::write(dir.join("a.txt"), "x")?;
    assert!(
        yi_tools::command("git")
            .args(["add", "a.txt"])
            .current_dir(&dir)
            .status()?
            .success()
    );
    std::fs::write(dir.join("a.txt"), "y")?; // a tracked edit: counts as modified
    std::fs::write(dir.join("b.txt"), "z")?; // git has never seen these: untracked
    std::fs::write(dir.join("c.txt"), "w")?;
    let (branch, modified, untracked) = git_summary(&dir).ok_or("no summary in a repo")?;
    assert_eq!(branch, "main");
    assert_eq!(modified, 1, "one tracked file edited after staging");
    assert_eq!(
        untracked, 2,
        "two new files git has never seen, not modified"
    );
    Ok(())
}

#[test]
fn the_git_line_names_modified_and_untracked_omitting_zeros() {
    assert_eq!(
        git_line("main", 0, 0),
        " (git: main, 0 modified)",
        "a clean tree keeps today's zero rather than printing neither count"
    );
    assert_eq!(
        git_line("main", 1, 2),
        " (git: main, 1 modified, 2 untracked)"
    );
    assert_eq!(
        git_line("main", 0, 2),
        " (git: main, 2 untracked)",
        "a zero count is omitted, not printed as 0 untracked"
    );
    assert_eq!(git_line("main", 1, 0), " (git: main, 1 modified)");
}

#[test]
fn tracked_reports_only_paths_git_knows() -> TestResult {
    let dir = Scratch::new("yi-tracked")?;
    std::fs::create_dir_all(dir.join("src"))?;
    assert!(
        yi_tools::command("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&dir)
            .status()?
            .success()
    );
    std::fs::write(dir.join("src/lib.rs"), "x")?;
    std::fs::write(dir.join("scratch.txt"), "y")?;
    assert!(
        yi_tools::command("git")
            .args(["add", "src/lib.rs"])
            .current_dir(&dir)
            .status()?
            .success()
    );
    let inside = dir.join("src/lib.rs").display().to_string();
    let loose = dir.join("scratch.txt").display().to_string();
    let outside = std::env::temp_dir()
        .join("elsewhere.txt")
        .display()
        .to_string();
    assert_eq!(
        yi_runtime::environment::tracked(&dir, &[inside, loose, outside]),
        vec![true, false, false]
    );
    Ok(())
}

#[test]
fn the_deadline_line_counts_down_and_stops_at_zero() {
    use std::time::Duration;
    assert_eq!(
        deadline_line(Duration::from_secs(3600), Duration::from_secs(60)),
        "deadline: 3540s left of 3600s"
    );
    assert_eq!(
        deadline_line(Duration::from_secs(3600), Duration::from_secs(4000)),
        "deadline: 0s left of 3600s"
    );
}

#[test]
fn the_files_line_lists_the_top_level_and_caps_at_twenty() -> TestResult {
    let dir = Scratch::new("yi-env-files")?;
    std::fs::create_dir_all(dir.join("a-sub"))?;
    for i in 0..24 {
        std::fs::write(dir.join(format!("f{i:02}.txt")), "x")?;
    }
    std::fs::write(dir.join(".hidden"), "x")?;
    let line = files_line(&dir).ok_or("a populated dir has a files line")?;
    assert!(line.starts_with("files: a-sub/ f00.txt f01.txt"), "{line}");
    assert!(
        line.ends_with(" …(+5)"),
        "25 names, {FILES_SHOWN} shown: {line}"
    );
    assert!(!line.contains(".hidden"));
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty)?;
    assert!(files_line(&empty).is_none(), "an empty dir has no line");
    Ok(())
}

/// The model is told where it runs: a laptop, a container, a VM, an ssh session.
#[test]
fn the_platform_line_names_the_host() -> TestResult {
    let line = yi_runtime::environment::platform_line(" · shell zsh");
    let facts = yi_runtime::host::facts();
    assert!(
        line.starts_with(&format!(
            "platform: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        "{line}"
    );
    assert!(
        line.ends_with(&format!(" · host {}", facts.line())),
        "the clause the model reads is the probe's own answer: {line}"
    );
    // An ssh session is the client's fact, not the daemon's, so it is read per call.
    unsafe { std::env::set_var("SSH_CONNECTION", "203.0.113.4 22 203.0.113.9 22") };
    let attached = yi_runtime::host::facts();
    unsafe { std::env::remove_var("SSH_CONNECTION") };
    assert!(
        attached.ssh && attached.line().contains("over ssh"),
        "{attached:?}"
    );
    assert!(
        !yi_runtime::host::facts().ssh,
        "and it stops being true when the client leaves"
    );
    Ok(())
}

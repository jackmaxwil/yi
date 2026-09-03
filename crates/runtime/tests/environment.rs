use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::environment::{append, git_summary, render, sanitize};
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
fn git_summary_reads_branch_and_dirty_count() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    assert_eq!(git_summary(&dir), None, "a plain directory has no branch");
    let init = yi_tools::command("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&dir)
        .status()?;
    assert!(init.success());
    std::fs::write(dir.join("a.txt"), "x")?;
    let (branch, dirty) = git_summary(&dir).ok_or("no summary in a repo")?;
    assert_eq!(branch, "main");
    assert_eq!(dirty, 1);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn tracked_reports_only_paths_git_knows() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tracked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
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
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

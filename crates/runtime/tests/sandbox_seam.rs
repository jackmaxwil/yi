#![cfg(target_os = "macos")]

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, PermissionBroker, PermissionMode, ProviderStream, SessionConfig, builtin_tools,
};
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

/// The seam, end to end: the broker contains an unknown command, the adapter
/// hands the sandbox to the tool, the command runs with no prompt and cannot
/// reach past the working tree, and the same command asks the second time
/// because containment already refused it once.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_unknown_command_runs_contained_then_asks() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let escape = home.join("escaped.txt");
    let command = format!("printf x > {}", escape.display());

    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        bash_call("call-1", &command),
        bash_call("call-2", &command),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let sandbox = yi_tools::Sandbox {
        writable: vec![project.clone()],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
    };
    let broker = Arc::new(
        PermissionBroker::new(
            PermissionMode::Auto,
            project.clone(),
            Vec::new(),
            None,
            session.events_sender(),
        )
        .with_sandbox(Some(sandbox)),
    );
    session.use_tools(builtin_tools(), project.clone(), Some(broker));

    // One turn runs both calls: the loop answers the first tool result with the
    // next queued message, which repeats the same command.
    session.prompt("do the thing")?;
    session.wait_idle().await;
    let results = results(&session);
    assert!(
        !escape.exists(),
        "the contained command must not write outside the tree"
    );
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(
        !results[0].contains("Permission denied"),
        "containment runs instead of asking: {}",
        results[0]
    );
    assert!(
        results[0].contains("the sandbox refused this"),
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
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_refused_program_asks_even_when_the_retry_text_differs() -> TestResult {
    if !yi_tools::Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-seam-scope")?;
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&home)?;
    let first = format!("mkdir {}", home.join("a").display());
    let second = format!("mkdir {} && ls", home.join("b").display());

    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![
        bash_call("call-1", &first),
        bash_call("call-2", &second),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    let sandbox = yi_tools::Sandbox {
        writable: vec![project.clone()],
        deny_read: Vec::new(),
        deny_write: Vec::new(),
    };
    let broker = Arc::new(
        PermissionBroker::new(
            PermissionMode::Auto,
            project.clone(),
            Vec::new(),
            None,
            session.events_sender(),
        )
        .with_sandbox(Some(sandbox)),
    );
    session.use_tools(builtin_tools(), project.clone(), Some(broker));
    session.prompt("make the dirs")?;
    session.wait_idle().await;
    let results = results(&session);
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(
        results[0].contains("`mkdir` now needs permission"),
        "the hint names the scope that will ask: {}",
        results[0]
    );
    assert!(
        results[1].contains("Permission denied") && !home.join("b").exists(),
        "a different command using the refused program asks: {}",
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

use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use yi_runtime::permission::{CallOutcome, PermissionBroker};
use yi_runtime::{AskOutcome, Asker, PermissionMode};
use yi_tools::{Tool, ToolKind};

type TestResult = Result<(), Box<dyn Error>>;

fn broker(reply: AskOutcome) -> std::io::Result<(Arc<PermissionBroker>, Arc<Mutex<u32>>, Scratch)> {
    let cwd = Scratch::new("yi-ask-user")?;
    let asked = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&asked);
    let asker: Asker = Arc::new(move |_: &yi_runtime::PermissionAsk<'_>| {
        if let Ok(mut asked) = counter.lock() {
            *asked += 1;
        }
        reply
    });
    let (events, _receiver) = tokio::sync::broadcast::channel(64);
    let broker = PermissionBroker::new(
        PermissionMode::Auto,
        cwd.to_path_buf(),
        Vec::new(),
        Some(asker),
        events,
    );
    Ok((Arc::new(broker), asked, cwd))
}

fn args(command: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    args.insert("command".to_owned(), command.into());
    args
}

/// The production call site runs the gate on a blocking thread, so the test stands there too.
async fn decide(
    broker: &Arc<PermissionBroker>,
    command: &str,
) -> Result<CallOutcome, Box<dyn Error>> {
    let (broker, args) = (Arc::clone(broker), args(command));
    Ok(tokio::task::spawn_blocking(move || {
        broker.decide_call("bash", ToolKind::Exec, true, "call-1", &args, None)
    })
    .await?)
}

/// Dies with a removed reviewer still standing between the user and an auto-mode ask: the
/// deterministic ladder cannot prove `rm -rf`, so with no classifier the user is asked, once.
#[tokio::test]
async fn an_auto_mode_ask_nothing_settled_goes_straight_to_the_user() -> TestResult {
    let (denying, asked, _cwd) = broker(AskOutcome::Reject)?;
    let outcome = decide(&denying, "rm -rf build").await?;
    assert!(!outcome.allowed);
    assert_eq!(
        outcome.reason,
        "The user denied this call. `rm` is destructive; it cannot be undone by a checkpoint: rm -rf build"
    );
    assert_eq!(asked.lock().map(|asked| *asked).unwrap_or(0), 1);

    let (allowing, asked, _cwd) = broker(AskOutcome::AllowOnce)?;
    let outcome = decide(&allowing, "rm -rf build").await?;
    assert!(outcome.allowed);
    assert_eq!(outcome.reason, "allowed by user");
    assert_eq!(asked.lock().map(|asked| *asked).unwrap_or(0), 1);
    Ok(())
}

/// Dies with an unattended run ended by its own question: nobody answers a root's `ask_user`
/// there, so it gets the default to act on and the turn goes on.
#[test]
fn a_root_nobody_attends_is_handed_the_default_and_goes_on() -> TestResult {
    for unattended in [false, true] {
        let dir = Scratch::new(&format!("yi-alone-{unattended}"))?;
        let ask = yi_runtime::ask_user::AskUserTool::default().unattended(unattended);
        let mut input = serde_json::Map::new();
        input.insert("question".to_owned(), serde_json::json!("Which base?"));
        input.insert("options".to_owned(), serde_json::json!(["main", "dev"]));
        input.insert("default".to_owned(), serde_json::json!("main"));
        let output = ask.execute(input, &yi_tools::ToolContext::new(dir.to_path_buf()));
        let text = match output.result.content.first() {
            Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
            _ => String::new(),
        };
        assert_eq!(output.result.terminate == Some(true), !unattended, "{text}");
        assert_eq!(text.contains("Go on with main"), unattended, "{text}");
    }
    Ok(())
}

/// A question to the user ends the turn on its own: the model does not have to
/// guess at a stopping sentence, and the todo interception reads the call, not the prose.
#[test]
fn a_question_ends_the_turn_and_lists_its_options() -> TestResult {
    let tool = yi_runtime::ask_user::AskUserTool::default();
    let mut input = serde_json::Map::new();
    input.insert(
        "question".to_owned(),
        serde_json::json!("Which base branch should the lane land on?"),
    );
    input.insert(
        "options".to_owned(),
        serde_json::json!(["main", "release/1.2"]),
    );
    input.insert("default".to_owned(), serde_json::json!("main"));
    tool.validate(&input)?;
    let output = tool.execute(input, &yi_tools::ToolContext::new(std::env::temp_dir()));
    assert!(!output.is_error);
    assert_eq!(output.result.terminate, Some(true));
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    assert!(
        text.starts_with("Question for the user: Which base branch"),
        "{text}"
    );
    assert!(text.contains("  1. main\n  2. release/1.2"), "{text}");
    assert!(text.contains("Default if unanswered: main"), "{text}");
    let mut request = serde_json::Map::new();
    request.insert("request".to_owned(), serde_json::json!(1));
    assert!(
        tool.validate(&request).is_err(),
        "an old denial's request number is no question"
    );
    Ok(())
}

/// Dies with an object option read as nothing: a child's parent saw no choices when options
/// carried `label` and `description`, and the user a card with no trade-off.
#[test]
fn an_option_with_a_description_lists_both_and_one_without_a_label_is_skipped() -> TestResult {
    let tool = yi_runtime::ask_user::AskUserTool::default();
    let mut input = serde_json::Map::new();
    input.insert("question".to_owned(), serde_json::json!("Which look?"));
    input.insert(
        "options".to_owned(),
        serde_json::json!([
            {"label": "Quiet", "description": "One dim line."},
            {"description": "no label"},
            "Loud"
        ]),
    );
    tool.validate(&input)?;
    let output = tool.execute(input, &yi_tools::ToolContext::new(std::env::temp_dir()));
    let text = match output.result.content.first() {
        Some(yi_types::message::Content::Text { text, .. }) => text.clone(),
        _ => String::new(),
    };
    assert!(text.contains("  1. Quiet: One dim line."), "{text}");
    assert!(
        text.contains("  2. Loud"),
        "the card's key 2 is Loud: {text}"
    );
    Ok(())
}

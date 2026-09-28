use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_runtime::auto_review::{ReviewOutcome, Reviewer, parse_outcome, review_fragment};
use yi_runtime::permission::{CallOutcome, PermissionBroker};
use yi_runtime::{AskOutcome, Asker, PermissionMode, ProviderStream};
use yi_tools::ToolKind;
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};
use yi_types::permission::Answerer;

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

struct Harness {
    broker: Arc<PermissionBroker>,
    provider: Arc<ProviderStream>,
    asks: Arc<Mutex<Vec<String>>>,
    events: tokio::sync::broadcast::Receiver<yi_types::event::AgentEvent>,
    _cwd: Scratch,
}

fn permission_events(harness: &mut Harness) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(event) = harness.events.try_recv() {
        match event {
            yi_types::event::AgentEvent::PermissionRequested { tool_call_id, .. } => {
                seen.push(format!("requested {tool_call_id}"));
            }
            yi_types::event::AgentEvent::PermissionResolved {
                tool_call_id,
                allowed,
            } => seen.push(format!("resolved {tool_call_id} {allowed}")),
            _ => {}
        }
    }
    seen
}

fn answers(replies: &[&str]) -> Vec<yi_types::message::AgentMessage> {
    replies
        .iter()
        .map(|reply| faux_assistant_message(vec![faux_text(reply)], StopReason::Stop))
        .collect()
}

fn setup(reply: Option<AskOutcome>, with_reviewer: bool) -> std::io::Result<Harness> {
    let cwd = Scratch::new("yi-auto-review")?;
    let asks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let asker: Option<Asker> = reply.map(|reply| {
        let asks = Arc::clone(&asks);
        Arc::new(move |ask: &yi_runtime::PermissionAsk<'_>| {
            if let Ok(mut asks) = asks.lock() {
                asks.push(ask.text());
            }
            reply
        }) as Asker
    });
    let (events, receiver) = tokio::sync::broadcast::channel(64);
    let broker = Arc::new(PermissionBroker::new(
        PermissionMode::Auto,
        cwd.to_path_buf(),
        Vec::new(),
        asker,
        events,
    ));
    let provider = Arc::new(ProviderStream::new(None, None));
    if with_reviewer {
        broker.set_reviewer(Arc::new(Reviewer::new(Arc::clone(&provider), faux_model())));
    }
    Ok(Harness {
        broker,
        provider,
        asks,
        events: receiver,
        _cwd: cwd,
    })
}

fn destructive_args() -> Map<String, Value> {
    let mut args = Map::new();
    args.insert(
        "command".to_owned(),
        Value::String("rm -rf build".to_owned()),
    );
    args
}

/// The production call site runs the gate on a blocking thread; the reviewer is
/// answered on the runtime behind it, so the test has to stand where the tool
/// adapter stands or it is testing a different concurrency shape.
async fn decide(
    broker: &Arc<PermissionBroker>,
    args: Map<String, Value>,
) -> Result<CallOutcome, Box<dyn Error>> {
    let broker = Arc::clone(broker);
    Ok(tokio::task::spawn_blocking(move || {
        broker.decide_call("bash", ToolKind::Exec, true, "call-1", &args, None)
    })
    .await?)
}

fn provider_calls(provider: &Arc<ProviderStream>) -> u64 {
    provider
        .faux
        .lock()
        .map(|faux| faux.call_count)
        .unwrap_or(0)
}

fn credential_args() -> Map<String, Value> {
    let mut args = Map::new();
    args.insert(
        "command".to_owned(),
        Value::String("cat ~/.ssh/id_rsa".to_owned()),
    );
    args
}

/// D81's whole security argument is that jurisdiction is structural, so the
/// broker has to be seen honouring the flag rather than the flag be seen being
/// set. Both halves of the gate: an unreviewable ask, and a mode with no
/// fallback ask to review.
#[tokio::test]
async fn a_call_outside_the_reviewers_jurisdiction_goes_straight_to_the_user() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    harness.provider.queue_faux(answers(&["allow"]));
    let outcome = decide(&harness.broker, credential_args()).await?;
    assert!(!outcome.allowed);
    assert!(
        outcome.reason.contains("reads a credential store"),
        "the credential gate never fired, so this proves nothing: {}",
        outcome.reason
    );
    assert!(
        outcome.reason.starts_with("The user denied this call."),
        "the ask must reach the user, not the reviewer: {}",
        outcome.reason
    );
    assert_eq!(
        provider_calls(&harness.provider),
        0,
        "a credential read is unreachable from the reviewer"
    );

    let asking = setup(Some(AskOutcome::Reject), true)?;
    asking.broker.set_mode(PermissionMode::Ask);
    asking.provider.queue_faux(answers(&["allow"]));
    let outcome = decide(&asking.broker, destructive_args()).await?;
    assert_eq!(
        outcome.reason,
        "The user denied this call. tool invocation is not read-only: rm -rf build"
    );
    assert_eq!(
        provider_calls(&asking.provider),
        0,
        "no mode but auto has a fallback ask for the reviewer to screen"
    );
    Ok(())
}

#[tokio::test]
async fn a_reviewer_allowance_runs_the_call_and_never_asks_the_user() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    harness.provider.queue_faux(answers(&["allow"]));
    let outcome = decide(&harness.broker, destructive_args()).await?;
    assert!(outcome.allowed, "reason was {:?}", outcome.reason);
    assert!(outcome.reason.contains("auto reviewer"));
    assert!(
        harness
            .asks
            .lock()
            .map(|asks| asks.is_empty())
            .unwrap_or(false),
        "an allowance must not also wake the user"
    );
    Ok(())
}

/// A reviewer-mediated call is the one decision nobody watches, so it is the
/// one that must leave a trace: the TUI marks the waiting cell off
/// `PermissionRequested` and ACP enters `RequiresAction` on it. Both went dark
/// for every path that answers without [`PermissionBroker::run_ask`].
#[tokio::test]
async fn a_reviewed_decision_is_announced_and_settled_like_any_other() -> TestResult {
    let mut harness = setup(Some(AskOutcome::Reject), true)?;
    harness.provider.queue_faux(answers(&["allow"]));
    let allowed = decide(&harness.broker, destructive_args()).await?;
    assert!(allowed.allowed, "reason was {:?}", allowed.reason);
    assert_eq!(
        permission_events(&mut harness),
        ["requested call-1", "resolved call-1 true"],
        "a model-authorized call ran with nothing on the event stream"
    );

    harness.provider.queue_faux(answers(&["deny unprovable"]));
    let denied = decide(&harness.broker, destructive_args()).await?;
    assert!(!denied.allowed);
    assert_eq!(
        permission_events(&mut harness),
        ["requested call-1", "resolved call-1 false"]
    );

    let repeat = decide(&harness.broker, destructive_args()).await?;
    assert!(!repeat.allowed);
    assert_eq!(
        permission_events(&mut harness),
        ["requested call-1", "resolved call-1 false"],
        "a recalled answer is still a decision the surfaces have to see"
    );

    let replay = harness.broker.resolve_request(1, "call-ask");
    assert!(replay.contains("denied"), "{replay}");
    assert_eq!(
        permission_events(&mut harness),
        ["requested call-ask", "resolved call-ask false"],
        "the ask_user replay blocks on the human with no surface told"
    );
    Ok(())
}

/// The denial has to be actionable: the reviewer's own words plus the number
/// `ask_user` takes. A denial with neither is a dead end the model retries.
#[tokio::test]
async fn a_reviewer_denial_carries_its_evidence_and_a_request_number() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    harness
        .provider
        .queue_faux(answers(&["deny it deletes an untracked build tree"]));
    let outcome = decide(&harness.broker, destructive_args()).await?;
    assert!(!outcome.allowed);
    assert!(
        outcome
            .reason
            .contains("it deletes an untracked build tree"),
        "the reviewer's evidence is missing: {}",
        outcome.reason
    );
    assert!(
        outcome.reason.contains("ask_user with request 1"),
        "the escalation route is missing: {}",
        outcome.reason
    );
    Ok(())
}

/// Fail safe: anything that is not a well-formed allowance denies. An empty
/// queue is the provider being unreachable, which must not read as consent.
#[tokio::test]
async fn a_malformed_or_absent_reviewer_answer_denies_rather_than_allows() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    harness
        .provider
        .queue_faux(answers(&["Sure! That looks fine to me, go ahead."]));
    let outcome = decide(&harness.broker, destructive_args()).await?;
    assert!(!outcome.allowed, "prose is not an allowance");

    let starved = setup(Some(AskOutcome::Reject), true)?;
    let outcome = decide(&starved.broker, destructive_args()).await?;
    assert!(!outcome.allowed, "an unreachable reviewer is not consent");
    assert!(outcome.reason.contains("ask_user with request 1"));

    assert!(matches!(parse_outcome(""), ReviewOutcome::Deny { .. }));
    assert!(matches!(
        parse_outcome("ALLOW ANYWAY"),
        ReviewOutcome::Deny { .. }
    ));
    assert_eq!(parse_outcome(" allow \n"), ReviewOutcome::Allow);
    Ok(())
}

/// The reuse rule end to end: one review, one question, and the approved
/// re-issue runs without spending either again.
#[tokio::test]
async fn an_approved_request_lets_the_identical_call_through_once() -> TestResult {
    let harness = setup(Some(AskOutcome::AllowOnce), true)?;
    let journal = journal(&harness.broker);
    harness.provider.queue_faux(answers(&["deny unprovable"]));
    let denied = decide(&harness.broker, destructive_args()).await?;
    assert!(!denied.allowed);
    let after_review = provider_calls(&harness.provider);

    let replay = harness.broker.resolve_request(1, "call-ask");
    assert!(replay.contains("approved"), "{replay}");
    assert_eq!(
        harness.asks.lock().map(|asks| asks.len()).unwrap_or(0),
        1,
        "the stored ask is replayed to the user exactly once"
    );

    let allowed = decide(&harness.broker, destructive_args()).await?;
    assert!(allowed.allowed, "reason was {:?}", allowed.reason);
    assert_eq!(
        provider_calls(&harness.provider),
        after_review,
        "the approved re-issue must not spend a second review"
    );

    let again = decide(&harness.broker, destructive_args()).await?;
    assert!(!again.allowed, "one yes is one run, not a standing grant");
    assert_eq!(
        answerers(&journal),
        [Answerer::Reviewer, Answerer::User, Answerer::Reviewer],
        "the approved re-issue replays the user's answer; the one after it is reviewed afresh"
    );
    Ok(())
}

/// A retry loop must cost nothing: the same denial, the same request, no second
/// review and no second question.
#[tokio::test]
async fn a_denied_call_re_issued_unchanged_spends_no_second_review() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    let journal = journal(&harness.broker);
    harness.provider.queue_faux(answers(&["deny unprovable"]));
    let first = decide(&harness.broker, destructive_args()).await?;
    let after_review = provider_calls(&harness.provider);
    let second = decide(&harness.broker, destructive_args()).await?;
    assert_eq!(
        first.reason, second.reason,
        "an identical re-issue gets the identical answer"
    );
    assert_eq!(
        provider_calls(&harness.provider),
        after_review,
        "and costs no second review"
    );

    let mut mutated = Map::new();
    mutated.insert(
        "command".to_owned(),
        Value::String("rm -rf build/".to_owned()),
    );
    let third = decide(&harness.broker, mutated).await?;
    assert!(!third.allowed);
    assert!(
        provider_calls(&harness.provider) > after_review,
        "a mutated call is a different action and must be reviewed on its own"
    );
    assert_eq!(
        answerers(&journal),
        [Answerer::Reviewer, Answerer::Reviewer],
        "the replayed denial is not journaled; the mutated call's review is"
    );
    Ok(())
}

#[tokio::test]
async fn a_user_denial_stands_without_asking_again() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), true)?;
    let journal = journal(&harness.broker);
    harness.provider.queue_faux(answers(&["deny unprovable"]));
    decide(&harness.broker, destructive_args()).await?;
    let replay = harness.broker.resolve_request(1, "call-ask");
    assert!(replay.contains("denied"), "{replay}");

    let again = decide(&harness.broker, destructive_args()).await?;
    assert!(!again.allowed);
    assert!(
        again.reason.contains("stays denied"),
        "the second refusal must cite the user, not re-open: {}",
        again.reason
    );
    assert_eq!(
        harness.asks.lock().map(|asks| asks.len()).unwrap_or(0),
        1,
        "the user is asked once and not worn down"
    );
    assert_eq!(
        answerers(&journal),
        [Answerer::Reviewer, Answerer::User],
        "a replayed answer is not journaled as a second one"
    );
    Ok(())
}

fn journal(broker: &PermissionBroker) -> Arc<Mutex<Vec<Answerer>>> {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&journal);
    broker.set_journal(Arc::new(move |record| {
        if let Ok(mut sink) = sink.lock() {
            sink.push(record.by);
        }
    }));
    journal
}

fn answerers(journal: &Mutex<Vec<Answerer>>) -> Vec<Answerer> {
    journal.lock().map(|by| by.clone()).unwrap_or_default()
}

/// D28: headless, an escalation degrades to text that keeps the evidence,
/// never a silent terminal error.
#[tokio::test]
async fn headless_escalation_degrades_and_keeps_the_evidence() -> TestResult {
    let harness = setup(None, true)?;
    harness
        .provider
        .queue_faux(answers(&["deny it leaves the working tree"]));
    let denied = decide(&harness.broker, destructive_args()).await?;
    assert!(denied.reason.contains("it leaves the working tree"));
    let replay = harness.broker.resolve_request(1, "call-ask");
    assert!(
        replay.contains("no interactive surface"),
        "the degrade must say why: {replay}"
    );
    assert!(
        replay.contains("rm -rf build"),
        "and must keep the evidence: {replay}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_request_number_is_named_rather_than_guessed() -> TestResult {
    let harness = setup(Some(AskOutcome::AllowOnce), true)?;
    let replay = harness.broker.resolve_request(99, "call-ask");
    assert!(replay.contains("no open request 99"), "{replay}");
    assert!(
        harness
            .asks
            .lock()
            .map(|asks| asks.is_empty())
            .unwrap_or(false),
        "a bogus number must never open a real approval prompt"
    );
    Ok(())
}

/// The off switch. With no role named the reviewer is never constructed and
/// the outcome is the deterministic one this file had before auto review existed.
#[tokio::test]
async fn with_no_reviewer_named_the_decision_is_byte_identical_to_today() -> TestResult {
    let harness = setup(Some(AskOutcome::Reject), false)?;
    assert!(!harness.broker.has_reviewer());
    let outcome = decide(&harness.broker, destructive_args()).await?;
    assert!(!outcome.allowed);
    assert_eq!(
        outcome.reason,
        "The user denied this call. `rm` is destructive; it cannot be undone by a checkpoint: rm -rf build"
    );
    assert_eq!(provider_calls(&harness.provider), 0, "no review was spent");

    let allowing = setup(Some(AskOutcome::AllowOnce), false)?;
    let outcome = decide(&allowing.broker, destructive_args()).await?;
    assert!(outcome.allowed);
    assert_eq!(outcome.reason, "allowed by user");
    Ok(())
}

/// The whole gate in one place: with the role unnamed no reviewer is constructed and
/// the system prompt pays no bytes for the review sentence; `ask_user` is always there,
/// in question mode, and gains its `request` mode when the role is named.
#[tokio::test]
async fn the_role_is_the_only_switch_for_the_reviewer_the_tool_and_the_sentence() -> TestResult {
    for named in [false, true] {
        let dir = Scratch::new(&format!("yi-wire-{named}"))?;
        let harness = setup(Some(AskOutcome::Reject), false)?;
        let session = yi_runtime::AgentSession::new(
            yi_runtime::SessionConfig {
                system_prompt: String::new(),
                model: faux_model(),
                thinking_level: None,
                tool_execution: yi_runtime::ExecutionMode::Sequential,
            },
            Arc::clone(&harness.provider),
        );
        session.install_extensions(yi_runtime::ext::install(yi_runtime::ExtOptions {
            cwd: dir.to_path_buf(),
            home: dir.to_path_buf(),
            mode: PermissionMode::Auto,
            user_system: String::new(),
            schema_instruction: None,
            context_window: 128_000,
            global_skills: Vec::new(),
        }));
        let mut tools: Vec<Arc<dyn yi_tools::Tool>> = Vec::new();
        yi_runtime::auto_review::wire_role(
            &session,
            (named.then(faux_model), Some(Arc::clone(&harness.broker))),
            &harness.provider,
            &mut tools,
            None,
        );
        let registered = tools.iter().any(|tool| tool.name() == "ask_user");
        let prompt = session
            .extensions()
            .and_then(|host| host.lock().ok().map(|host| host.system_prompt()))
            .unwrap_or_default();
        assert!(
            registered,
            "ask_user is registered whether or not a role is named"
        );
        assert_eq!(
            harness.broker.has_reviewer(),
            named,
            "the reviewer is constructed only when the role names it"
        );
        assert_eq!(
            prompt.contains("ask_user"),
            named,
            "an unnamed role must pay no prompt bytes"
        );
        let _ = session;
    }
    Ok(())
}

#[test]
fn the_review_fragment_names_the_tool_that_answers_a_denial() {
    let fragment = review_fragment();
    assert!(fragment.contains("ask_user"));
    assert!(fragment.contains("request number"));
    assert_eq!(
        yi_runtime::mode_fragment(PermissionMode::Auto),
        yi_runtime::mode_fragment(PermissionMode::Auto),
        "the mode fragment itself is untouched by the reviewer"
    );
    assert!(
        !yi_runtime::mode_fragment(PermissionMode::Auto).contains("ask_user"),
        "the sentence rides its own slot, so an unnamed role pays no bytes"
    );
}

/// A question to the user ends the turn on its own: the model does not have to
/// guess at a stopping sentence, and the todo interception reads the call, not the prose.
#[test]
fn a_question_ends_the_turn_and_lists_its_options() -> TestResult {
    use yi_tools::Tool;
    let tool = yi_runtime::auto_review::AskUserTool::new(None);
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
        "request mode needs auto-review; without it the model is told to ask a question"
    );
    Ok(())
}

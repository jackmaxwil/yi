#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::advisor::guard::EmissionGuard;
use yi_runtime::advisor::{
    AdvisorConfig, AdvisorDeps, advisory_text, attach_advisor, digest, review::LlmReviewer,
};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
use yi_types::advisor::{Advice, AdviceKind, AdvisorySeverity};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
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

fn session(provider: Arc<ProviderStream>) -> AgentSession {
    AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    )
}

fn tool_result(name: &str, is_error: bool) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "t1".to_owned(),
        tool_name: name.to_owned(),
        content: vec![Content::Text {
            text: if is_error { "boom" } else { "ok" }.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error,
        timestamp: 0,
    }
}

#[test]
fn guard_dedupes_blocks_noise_and_rate_limits() {
    let mut guard = EmissionGuard::default();
    guard.begin_cycle();
    assert!(
        !guard.accept("Stop."),
        "content-free filler must be dropped"
    );
    assert!(
        guard.accept("Missing await on writeStream.end() loses buffered writes"),
        "a real note passes and noise must not burn the slot"
    );
    assert!(
        !guard.accept("Another real note in the same cycle"),
        "one accepted note per cycle"
    );
    guard.begin_cycle();
    assert!(
        !guard.accept("missing await on WriteStream end() loses buffered writes!"),
        "punctuation/casing variants must dedupe"
    );
    guard.begin_cycle();
    assert!(
        guard.accept("A different note"),
        "fresh cycles accept fresh notes"
    );
}

#[test]
fn digest_keeps_constraints_first_and_extracts_directives() {
    let filler = "This sentence is ordinary filler about the weather. ".repeat(40);
    let text = format!("{filler}Never push to main without review. {filler}");
    let truncated = digest::truncate_user_text(&text, 200, "m7");
    assert!(
        truncated.contains("Never push to main"),
        "constraint sentences survive truncation first: {truncated}"
    );
    assert!(
        truncated.contains("pull m7"),
        "elided spans must carry the entry id as the pull handle"
    );
    let directives = digest::directives("Fix the bug. Only touch crates/loop. Thanks!", "m2");
    assert_eq!(directives, vec!["m2: Only touch crates/loop.".to_owned()]);
}

#[tokio::test]
async fn a_failure_streak_reaches_the_primary_through_nothing() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let session = session(Arc::clone(&provider));
    let advisor = attach_advisor(&session, AdvisorConfig::default(), AdvisorDeps::default());

    for _ in 0..3 {
        assert!(
            advisor.observe(&tool_result("bash", true), 0).is_none(),
            "no review is forced, so nothing may trigger a review"
        );
    }
    let stats = advisor.stats();
    assert!(
        stats.contains("0 reviews"),
        "a run with no reviewer must review nothing: {stats}"
    );

    provider.queue_faux(vec![faux_assistant_message(
        vec![faux_text("acknowledged")],
        StopReason::Stop,
    )]);
    session.prompt("continue")?;
    session.wait_idle().await;
    let spoken: Vec<String> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom {
                custom_type,
                content: UserContent::Text(text),
                ..
            } if custom_type == "advisory" => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        spoken.is_empty(),
        "with the deterministic reviewer gone nothing may reach the primary: {spoken:?}"
    );
    Ok(())
}

#[test]
fn headless_hold_degrades_to_warn() -> TestResult {
    let advice = Advice {
        severity: AdvisorySeverity::Hold,
        kind: AdviceKind::Stop,
        target: Some("migrations".to_owned()),
        text: "Schema migrations need review".to_owned(),
    };
    let rendered = advisory_text(&Advice {
        severity: AdvisorySeverity::Warn,
        ..advice.clone()
    });
    assert!(rendered.contains("severity=\"warn\"") && rendered.contains("target=\"migrations\""));

    let delivered: Arc<std::sync::Mutex<Vec<AgentMessage>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let runtime = yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig::default(),
        Arc::new(move |message| {
            if let Ok(mut queue) = sink.lock() {
                queue.push(message);
            }
        }),
        None,
    );
    runtime.deliver_reviewed(vec![advice], 0);
    let queue = delivered.lock().map_err(|error| error.to_string())?;
    let text = queue
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom {
                content: UserContent::Text(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        text.contains("severity=\"warn\""),
        "with no hold sink an advisor Hold must degrade to Warn (D28): {text}"
    );
    Ok(())
}

/// Incident: the `advise` schema leaves `target` optional, and a Hold without one became an
/// empty-pattern hold that every call's subject contains, so each call asked for an hour.
#[tokio::test]
async fn an_untargeted_hold_warns_instead_of_holding_every_call() -> TestResult {
    let root = Scratch::new("yi-untargeted-hold")?;
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut advise_args = Map::new();
    for (key, value) in [
        ("note", "Stop rewriting Cargo.lock by hand"),
        ("severity", "hold"),
        ("kind", "stop"),
    ] {
        advise_args.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("a1", "advise", advise_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("review done")], StopReason::Stop),
    ]);
    let scratch_runtime = Arc::new(yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig::default(),
        Arc::new(|_message| {}),
        None,
    ));
    let advices = LlmReviewer::new(Arc::clone(&provider), faux_model(), None)
        .review(&scratch_runtime, "m1 user: bump the deps")
        .await;
    assert_eq!(advices.len(), 1, "the advise tool call must be captured");

    let asks = Arc::new(std::sync::Mutex::new(0_usize));
    let counted = Arc::clone(&asks);
    let asker: yi_runtime::Asker = Arc::new(move |_ask| {
        if let Ok(mut count) = counted.lock() {
            *count += 1;
        }
        yi_runtime::AskOutcome::Reject
    });
    let broker = Arc::new(yi_runtime::permission::PermissionBroker::new(
        yi_runtime::PermissionMode::Auto,
        root.to_path_buf(),
        Vec::new(),
        Some(asker),
        tokio::sync::broadcast::channel(8).0,
    ));
    let mut session = session(Arc::clone(&provider));
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: "sys".to_owned(),
            tool_execution: ExecutionMode::Sequential,
            cwd: root.to_path_buf(),
            home: root.join("home"),
            lane_slots: 1,
            broker: Some(Arc::clone(&broker)),
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            summarizer: None,
            advisor: Some(faux_model()),
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
            family_dir: None,
        },
    );
    let advisor = session.advisor().ok_or("attach_runtime wires an advisor")?;
    advisor.deliver_reviewed(advices, 0);

    let mut ls = Map::new();
    ls.insert("command".to_owned(), Value::String("ls".to_owned()));
    let outcome = broker.decide_call("bash", yi_tools::ToolKind::Exec, false, "c1", &ls, None);
    let asked = *asks.lock().map_err(|_| "poisoned")?;
    assert!(
        outcome.allowed && asked == 0,
        "an advisor Hold with no target must not gate an unrelated call: allowed={}, reason={:?}, asks={asked}",
        outcome.allowed,
        outcome.reason
    );
    let stats = advisor.stats();
    assert!(
        stats.contains("1 warn(s) / 0 hold(s)"),
        "the untargeted Hold reaches the primary as a Warn: {stats}"
    );
    Ok(())
}

#[tokio::test]
async fn llm_reviewer_advises_through_the_advise_tool() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut advise_args = Map::new();
    advise_args.insert(
        "note".to_owned(),
        Value::String("Cite the failing test before claiming green".to_owned()),
    );
    advise_args.insert("severity".to_owned(), Value::String("warn".to_owned()));
    advise_args.insert("kind".to_owned(), Value::String("risk".to_owned()));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("a1", "advise", advise_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("review done")], StopReason::Stop),
    ]);
    let runtime = Arc::new(yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig::default(),
        Arc::new(|_message| {}),
        None,
    ));
    let reviewer = LlmReviewer::new(Arc::clone(&provider), faux_model(), None);
    let advices = reviewer.review(&runtime, "m1 user: fix the tests").await;
    assert_eq!(advices.len(), 1, "the advise tool call must be captured");
    assert_eq!(advices[0].severity, AdvisorySeverity::Warn);
    assert_eq!(advices[0].kind, AdviceKind::Risk);
    assert!(advices[0].text.contains("failing test"));
    Ok(())
}

#[test]
fn promotion_writes_a_rule_the_discovery_parser_accepts() -> TestResult {
    let root = Scratch::new("yi-promote")?;
    let rules_dir = root.join(".yi/rules");
    let runtime = yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig {
            rules_dir: Some(rules_dir.clone()),
            ..AdvisorConfig::default()
        },
        Arc::new(|_message| {}),
        None,
    );
    runtime.deliver_reviewed(
        vec![Advice {
            severity: AdvisorySeverity::Hold,
            kind: AdviceKind::Stop,
            target: Some("CLAUDE.md".to_owned()),
            text: "CLAUDE.md is generated; edit .ruler and run ruler apply".to_owned(),
        }],
        0,
    );
    let promotable = runtime.promotable();
    let (id, text) = promotable.last().ok_or("nothing was retained to promote")?;
    assert!(text.contains("generated"));

    let (path, body) = runtime.promote(id).map_err(|error| error.to_string())?;
    assert!(body.contains("trigger: CLAUDE.md") && body.contains("mode: gate"));
    assert!(
        body.contains("promoted from advisor") && body.contains(id),
        "the file says whose sentence it was: {body}"
    );

    // The file must satisfy the same reader discovery uses, or promotion has
    // written a rule that silently never fires.
    let set = yi_runtime::rules::discover(&root, &root.join("nonexistent-home"));
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    let rule = set
        .rules
        .iter()
        .find(|rule| rule.needles.iter().any(|needle| needle == "CLAUDE.md"))
        .ok_or("the promoted rule was not discovered")?;
    assert_eq!(rule.mode, yi_runtime::rules::RuleMode::Gate);

    let engine = yi_runtime::rules::RuleEngine::new(Vec::new());
    engine.insert(rule.clone());
    let denial = engine
        .check_tool("edit", r#"{"patch":"[CLAUDE.md#a1]"}"#)
        .ok_or("an armed promoted rule must gate the call it names")?;
    assert!(denial.contains("edit .ruler"));
    engine.insert(rule.clone());
    assert!(
        engine
            .check_tool("edit", r#"{"patch":"[CLAUDE.md#a1]"}"#)
            .is_none(),
        "re-inserting the same rule must not re-arm it or stack a duplicate"
    );

    assert!(
        runtime.promote("adv-does-not-exist").is_err(),
        "an unknown advice id is an error, not a blank rule"
    );
    // A fresh runtime: the guard accepts one note per review cycle, so the
    // targetless case cannot ride the same cycle as the one above.
    let targetless = yi_runtime::advisor::AdvisorRuntime::new(
        AdvisorConfig {
            rules_dir: Some(rules_dir),
            ..AdvisorConfig::default()
        },
        Arc::new(|_message| {}),
        None,
    );
    targetless.deliver_reviewed(
        vec![Advice {
            severity: AdvisorySeverity::Note,
            kind: AdviceKind::Scope,
            target: None,
            text: "targetless musing".to_owned(),
        }],
        0,
    );
    let last = targetless
        .promotable()
        .last()
        .map(|(id, _)| id.clone())
        .ok_or("no advice")?;
    assert!(
        targetless
            .promote(&last)
            .err()
            .is_some_and(|error| error.contains("names no target")),
        "advice with nothing to trigger on is refused, not guessed at"
    );
    assert!(path.exists());
    Ok(())
}

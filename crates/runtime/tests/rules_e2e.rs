use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_runtime::goal::DeliverFn;
use yi_runtime::rules::{RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope, discover};
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::schedule::DeliveryMode;

type TestResult = Result<(), Box<dyn Error>>;

fn scratch(label: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let base = std::env::temp_dir().join(format!("yi-rules-{label}-{}", std::process::id()));
    let home = base.join("home");
    let cwd = base.join("cwd");
    std::fs::create_dir_all(home.join(".yi/rules"))?;
    std::fs::create_dir_all(cwd.join(".yi/rules"))?;
    Ok((home, cwd))
}

#[test]
fn discovery_parses_shadows_and_names_skips() -> TestResult {
    let (home, cwd) = scratch("discover")?;
    std::fs::write(
        home.join(".yi/rules/no-leak.md"),
        "---\ntrigger: Box::leak\nscope: tool:edit\nmode: gate\ngap: 3\n---\nNever leak; use Arc.\n",
    )?;
    std::fs::write(
        home.join(".yi/rules/shadowed.md"),
        "---\ntrigger: global-needle\n---\nglobal body\n",
    )?;
    std::fs::write(
        cwd.join(".yi/rules/shadowed.md"),
        "---\ntrigger: project-needle\n---\nproject body\n",
    )?;
    std::fs::write(cwd.join(".yi/rules/broken.md"), "no frontmatter at all\n")?;
    std::fs::write(
        cwd.join(".yi/rules/gate-on-text.md"),
        "---\ntrigger: x\nscope: text\nmode: gate\n---\nbody\n",
    )?;

    let set = discover(&cwd, &home);
    assert_eq!(set.rules.len(), 2, "no-leak + shadowed");
    let leak = set
        .rules
        .iter()
        .find(|rule| rule.name == "no-leak")
        .ok_or("no-leak missing")?;
    assert_eq!(leak.scope, RuleScope::Tool("edit".to_owned()));
    assert_eq!(leak.gap, RuleGap::AfterTurns(3));
    assert_eq!(leak.mode, RuleMode::Gate);
    assert_eq!(leak.body, "Never leak; use Arc.");

    let shadowed = set
        .rules
        .iter()
        .find(|rule| rule.name == "shadowed")
        .ok_or("shadowed missing")?;
    assert_eq!(shadowed.body, "project body", "project shadows global");

    assert_eq!(
        set.warnings.len(),
        2,
        "broken + gate-on-text are named skips"
    );
    assert!(
        set.warnings
            .iter()
            .any(|warning| warning.contains("broken.md"))
    );
    assert!(
        set.warnings
            .iter()
            .any(|warning| warning.contains("gate-on-text.md")
                && warning.contains("text cannot be denied"))
    );
    Ok(())
}

fn rule(name: &str, needle: &str, scope: RuleScope, gap: RuleGap, mode: RuleMode) -> RuleDoc {
    RuleDoc {
        name: name.to_owned(),
        body: format!("{name} body: the user's own words."),
        path: PathBuf::from(format!("/rules/{name}.md")),
        needles: vec![needle.to_owned()],
        scope,
        gap,
        mode,
    }
}

fn engine_with_sink(rules: Vec<RuleDoc>) -> (Arc<RuleEngine>, Arc<Mutex<Vec<String>>>) {
    let engine = Arc::new(RuleEngine::new(rules));
    let delivered: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delivered);
    let deliver: DeliverFn = Arc::new(move |message, _mode: DeliveryMode| {
        if let AgentMessage::Custom {
            custom_type,
            content: UserContent::Text(text),
            ..
        } = message
            && custom_type == "reminder"
            && let Ok(mut queue) = sink.lock()
        {
            queue.push(text);
        }
    });
    engine.set_deliver(deliver);
    (engine, delivered)
}

fn assistant_saying(text: &str) -> AgentEvent {
    AgentEvent::MessageEnd {
        message: AgentMessage::Assistant {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            api: "faux".to_owned(),
            provider: "faux".to_owned(),
            model: "faux-1".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::zero(),
            stop_reason: StopReason::Stop,
            raw_stop_reason: None,
            end_turn: None,
            deferred: None,
            error_message: None,
            timestamp: 0,
        },
    }
}

#[test]
fn gate_denies_once_with_the_body_then_lets_the_informed_retry_through() -> TestResult {
    let (engine, _delivered) = engine_with_sink(vec![rule(
        "no-leak",
        "Box::leak",
        RuleScope::AnyTool,
        RuleGap::Once,
        RuleMode::Gate,
    )]);
    let denial = engine
        .check_tool("edit", r#"{"patch":"let x = Box::leak(y);"}"#)
        .ok_or("first match must deny")?;
    assert!(
        denial.contains("no-leak body") && denial.contains("/rules/no-leak.md"),
        "denial carries the rule body and source as evidence: {denial}"
    );
    assert!(
        engine
            .check_tool("edit", r#"{"patch":"still Box::leak here"}"#)
            .is_none(),
        "once-gap: the informed retry proceeds"
    );
    Ok(())
}

#[test]
fn gate_with_turn_gap_rearms_after_the_gap() -> TestResult {
    let (engine, _delivered) = engine_with_sink(vec![rule(
        "no-force-push",
        "push --force",
        RuleScope::Tool("bash".to_owned()),
        RuleGap::AfterTurns(2),
        RuleMode::Gate,
    )]);
    assert!(
        engine
            .check_tool("bash", r#"{"command":"git push --force"}"#)
            .is_some()
    );
    assert!(
        engine
            .check_tool("bash", r#"{"command":"git push --force"}"#)
            .is_none(),
        "inside the gap"
    );
    engine.observe(&assistant_saying("turn one"));
    engine.observe(&assistant_saying("turn two"));
    assert!(
        engine
            .check_tool("bash", r#"{"command":"git push --force"}"#)
            .is_some(),
        "the gap elapsed in completed turns; the gate re-arms"
    );
    assert!(
        engine
            .check_tool("edit", r#"{"patch":"git push --force"}"#)
            .is_none(),
        "tool scope binds: an edit never matches a bash-scoped rule"
    );
    Ok(())
}

#[test]
fn prose_reminder_fires_verbatim_once_and_gap_rearms() -> TestResult {
    let (engine, delivered) = engine_with_sink(vec![rule(
        "no-mock-claims",
        "tests pass",
        RuleScope::Text,
        RuleGap::AfterTurns(2),
        RuleMode::Remind,
    )]);
    engine.observe(&assistant_saying("All tests pass, done."));
    engine.observe(&assistant_saying("Again: tests pass."));
    {
        let queue = delivered.lock().map_err(|_| "lock")?;
        assert_eq!(queue.len(), 1, "gap holds inside the window");
        assert!(
            queue[0].contains("no-mock-claims body"),
            "the reminder is the rule body verbatim: {}",
            queue[0]
        );
    }
    engine.observe(&assistant_saying("quiet turn"));
    engine.observe(&assistant_saying("tests pass once more"));
    let queue = delivered.lock().map_err(|_| "lock")?;
    assert_eq!(queue.len(), 2, "the gap elapsed; the rule re-arms");
    Ok(())
}

#[test]
fn tool_scoped_reminder_delivers_at_the_gate_without_denying() -> TestResult {
    let (engine, delivered) = engine_with_sink(vec![rule(
        "prefer-rg",
        "grep -r",
        RuleScope::Tool("bash".to_owned()),
        RuleGap::Once,
        RuleMode::Remind,
    )]);
    assert!(
        engine
            .check_tool("bash", r#"{"command":"grep -r foo ."}"#)
            .is_none(),
        "remind never denies"
    );
    let queue = delivered.lock().map_err(|_| "lock")?;
    assert_eq!(queue.len(), 1);
    assert!(queue[0].contains("prefer-rg body"));
    Ok(())
}

#[tokio::test]
async fn gate_rule_denies_through_the_real_adapter_before_execution() -> TestResult {
    use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_loop::ExecutionMode;
    use yi_runtime::{AgentSession, ProviderStream, SessionConfig};
    use yi_types::model::{Model, ModelCost};

    let zero = || serde_json::Number::from(0u64);
    let model = Model {
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
    };
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut call_args = serde_json::Map::new();
    let marker = std::env::temp_dir().join(format!("yi-rule-gate-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    call_args.insert(
        "command".to_owned(),
        serde_json::json!(format!("git push --force && touch {}", marker.display())),
    );
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", "bash", call_args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model,
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.set_rules_engine(Arc::new(RuleEngine::new(vec![rule(
        "no-force-push",
        "push --force",
        RuleScope::Tool("bash".to_owned()),
        RuleGap::Once,
        RuleMode::Gate,
    )])));
    session.use_tools(yi_tools::builtin_tools(), std::env::temp_dir(), None);
    let mut events = session.subscribe();
    session.prompt("push it")?;
    session.wait_idle().await;
    let mut denial = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            assert!(is_error, "the gate must deny");
            denial = Some(
                result
                    .content
                    .iter()
                    .map(|content| match content {
                        Content::Text { text, .. } => text.clone(),
                        _ => String::new(),
                    })
                    .collect::<String>(),
            );
        }
    }
    let denial = denial.ok_or("no tool result observed")?;
    assert!(
        denial.contains("Denied by rule `no-force-push`") && denial.contains("no-force-push body"),
        "{denial}"
    );
    assert!(
        !marker.exists(),
        "the command must never have executed: the gate sits before the spawn"
    );
    Ok(())
}

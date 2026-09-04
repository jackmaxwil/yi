use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yi_runtime::goal::DeliverFn;
use yi_runtime::rules::{
    RuleDoc, RuleEngine, RuleGap, RuleMode, RuleScope, discover, discover_armed,
};
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
                && warning.contains("cannot be denied"))
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
        paths: Vec::new(),
        after: 1,
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

#[test]
fn result_scope_fires_on_the_output_not_the_args() -> TestResult {
    let (engine, delivered) = engine_with_sink(vec![rule(
        "borrowck",
        "E0502",
        RuleScope::Result,
        RuleGap::Once,
        RuleMode::Remind,
    )]);
    assert!(
        engine
            .check_tool("bash", r#"{"command":"cargo test","path":"src/lib.rs"}"#)
            .is_none()
    );
    assert!(
        delivered.lock().map_err(|_| "lock")?.is_empty(),
        "args never carry the rustc code"
    );
    engine.check_result(
        "bash",
        r#"{"command":"cargo test","path":"src/lib.rs"}"#,
        "error[E0502]: cannot borrow `x` as mutable",
        true,
    );
    let queue = delivered.lock().map_err(|_| "lock")?;
    assert_eq!(queue.len(), 1);
    assert!(queue[0].contains("borrowck body"), "{}", queue[0]);
    Ok(())
}

#[test]
fn error_scope_skips_a_clean_result() -> TestResult {
    let (engine, delivered) = engine_with_sink(vec![rule(
        "borrowck",
        "E0502",
        RuleScope::Error,
        RuleGap::Once,
        RuleMode::Remind,
    )]);
    engine.check_result(
        "bash",
        "{}",
        "error[E0502]: mentioned in a passing log",
        false,
    );
    assert!(delivered.lock().map_err(|_| "lock")?.is_empty());
    engine.check_result("bash", "{}", "error[E0502]: cannot borrow", true);
    assert_eq!(delivered.lock().map_err(|_| "lock")?.len(), 1);
    Ok(())
}

#[test]
fn paths_and_the_needle() -> TestResult {
    let mut rust = rule(
        "borrowck",
        "E0502",
        RuleScope::Result,
        RuleGap::Once,
        RuleMode::Remind,
    );
    rust.paths = vec![yi_permission::PathGlob::new("**/*.rs")?];
    let (engine, delivered) = engine_with_sink(vec![rust]);
    engine.check_result(
        "read",
        r#"{"path":"notes.md"}"#,
        "error[E0502]: cannot borrow",
        true,
    );
    assert!(
        delivered.lock().map_err(|_| "lock")?.is_empty(),
        "wrong path"
    );
    engine.check_result(
        "read",
        r#"{"path":"src/borrow.rs"}"#,
        "error[E0502]: cannot borrow",
        true,
    );
    assert_eq!(delivered.lock().map_err(|_| "lock")?.len(), 1);
    Ok(())
}

#[test]
fn after_three_escalates_a_gate() -> TestResult {
    let mut doc = rule(
        "no-leak",
        "Box::leak",
        RuleScope::AnyTool,
        RuleGap::Once,
        RuleMode::Gate,
    );
    doc.after = 3;
    let (engine, _delivered) = engine_with_sink(vec![doc]);
    let args = r#"{"patch":"let x = Box::leak(y);"}"#;
    assert!(engine.check_tool("edit", args).is_none());
    assert!(engine.check_tool("edit", args).is_none());
    assert!(
        engine.check_tool("edit", args).is_some(),
        "third identical evidence denies"
    );
    Ok(())
}

#[test]
fn evidence_latch_is_per_path() -> TestResult {
    let (engine, delivered) = engine_with_sink(vec![rule(
        "borrowck",
        "E0502",
        RuleScope::Result,
        RuleGap::Once,
        RuleMode::Remind,
    )]);
    engine.check_result(
        "bash",
        r#"{"path":"a.rs"}"#,
        "error[E0502]: cannot borrow",
        true,
    );
    engine.check_result(
        "bash",
        r#"{"path":"a.rs"}"#,
        "error[E0502]: cannot borrow",
        true,
    );
    engine.check_result(
        "bash",
        r#"{"path":"b.rs"}"#,
        "error[E0502]: cannot borrow",
        true,
    );
    assert_eq!(
        delivered.lock().map_err(|_| "lock")?.len(),
        2,
        "same path latches; a new path is new evidence"
    );
    Ok(())
}

#[test]
fn skill_pointer_is_one_line_and_read_suppresses() -> TestResult {
    let mut doc = rule(
        "rust-borrowck",
        "E0502",
        RuleScope::Error,
        RuleGap::Once,
        RuleMode::Remind,
    );
    doc.body = "skill://rust-borrowck".to_owned();
    let (engine, delivered) = engine_with_sink(vec![doc]);
    engine.check_result("bash", "{}", "error[E0502]: cannot borrow", true);
    {
        let queue = delivered.lock().map_err(|_| "lock")?;
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue[0],
            "Relevant: skill://rust-borrowck (read before the next edit)"
        );
    }
    engine.rearm();
    engine.check_result(
        "read",
        r#"{"path":"/tmp/.yi/skills/rust-borrowck/SKILL.md"}"#,
        "body",
        false,
    );
    engine.check_result("bash", "{}", "error[E0502]: cannot borrow", true);
    assert_eq!(
        delivered.lock().map_err(|_| "lock")?.len(),
        1,
        "a read of the skill file spends the pointer"
    );
    Ok(())
}

#[test]
fn skill_trigger_compiles_when_the_user_did_not_claim_the_name() -> TestResult {
    let (home, cwd) = scratch("skill-rule")?;
    std::fs::create_dir_all(cwd.join(".yi/skills/rust-borrowck"))?;
    std::fs::write(
        cwd.join(".yi/skills/rust-borrowck/SKILL.md"),
        "---\nname: rust-borrowck\ntrigger: E0502\nscope: error\n---\nHow to fix borrows.\n",
    )?;
    let set = discover_armed(&cwd, &home);
    let rule = set
        .rules
        .iter()
        .find(|rule| rule.name == "rust-borrowck")
        .ok_or("skill did not compile")?;
    assert_eq!(rule.body, "skill://rust-borrowck");
    assert_eq!(rule.scope, RuleScope::Error);
    Ok(())
}

/// The D54 fire-lane corpus (`evals/fixtures/rules/lanes.jsonl`) run through the
/// real engine. The Python beside it scores the labels; this scores the matcher,
/// so a recall claim can never again be moved by editing a model of the engine.
#[test]
fn the_lane_fixture_runs_through_the_engine() -> TestResult {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/fixtures/rules/lanes.jsonl");
    let corpus = std::fs::read_to_string(&fixture)?;
    let mut should = 0;
    let mut comment_fp = 0;
    let mut args_fp = 0;
    for line in corpus.lines().filter(|line| !line.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line)?;
        let lane = row["lane"].as_str().ok_or("row has no lane")?;
        let haystack = row["haystack"].as_str().ok_or("row has no haystack")?;
        let needle = row["needle"].as_str().unwrap_or("E0502");
        let is_error = row["is_error"].as_bool().unwrap_or(false);
        let scope = match lane {
            "args" => RuleScope::AnyTool,
            "result" => RuleScope::Result,
            "error" => RuleScope::Error,
            "text" => RuleScope::Text,
            other => return Err(format!("unknown lane `{other}`").into()),
        };
        // A fresh engine per row: the latch is state, and rows are independent.
        let (engine, delivered) = engine_with_sink(vec![rule(
            "lane",
            needle,
            scope,
            RuleGap::Once,
            RuleMode::Remind,
        )]);
        match lane {
            "args" => {
                let _ = engine.check_tool("edit", haystack);
            }
            "result" => engine.check_result("bash", "{}", haystack, false),
            "error" => engine.check_result("bash", "{}", haystack, is_error),
            _ => engine.observe(&assistant_saying(haystack)),
        }
        let fired = delivered.lock().map_err(|_| "lock")?.len();
        if row["label"] == "should" {
            assert_eq!(fired, 1, "a should-fire row went silent: {line}");
            should += 1;
        } else if lane == "text" {
            comment_fp += fired;
        } else if lane == "args" {
            args_fp += fired;
        } else {
            assert_eq!(fired, 0, "a should-not row fired: {line}");
        }
    }
    assert_eq!(should, 2, "the corpus's should-fire rows");
    // The fixture's own notes: a literal substring cannot tell a needle in a
    // comment from one in an error. Pinned honestly rather than papered over.
    assert_eq!(comment_fp, 2, "the known text-lane comment false positives");
    assert_eq!(args_fp, 1, "the patch-comment coincidence in the args lane");
    Ok(())
}

#[test]
fn three_user_rules_all_deliver_in_one_scan() -> TestResult {
    let rules = ["no-leak", "prefer-arc", "read-the-drop"]
        .into_iter()
        .map(|name| {
            rule(
                name,
                "Box::leak",
                RuleScope::AnyTool,
                RuleGap::Once,
                RuleMode::Remind,
            )
        })
        .collect();
    let (engine, delivered) = engine_with_sink(rules);
    assert!(
        engine
            .check_tool("edit", r#"{"patch":"let x = Box::leak(y);"}"#)
            .is_none()
    );
    assert_eq!(
        delivered.lock().map_err(|_| "lock")?.len(),
        3,
        "the cap is for skill:// pointers; a user's own words are never dropped"
    );
    Ok(())
}

#[test]
fn the_third_skill_pointer_is_dropped_and_stays_armed() -> TestResult {
    let pointers = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            let mut doc = rule(
                name,
                "E0502",
                RuleScope::Error,
                RuleGap::Once,
                RuleMode::Remind,
            );
            doc.body = format!("skill://{name}");
            doc
        })
        .collect();
    let (engine, delivered) = engine_with_sink(pointers);
    engine.check_result("bash", "{}", "error[E0502]: cannot borrow", true);
    {
        let queue = delivered.lock().map_err(|_| "lock")?;
        assert_eq!(queue.len(), 2, "two pointers is the budget for one scan");
    }
    engine.check_result("bash", "{}", "error[E0502]: cannot borrow", true);
    let queue = delivered.lock().map_err(|_| "lock")?;
    assert_eq!(queue.len(), 3, "the dropped pointer was never latched");
    assert_eq!(
        queue[2],
        "Relevant: skill://gamma (read before the next edit)"
    );
    Ok(())
}

#[test]
fn after_three_counts_one_needle_through_a_changing_result() -> TestResult {
    let mut doc = rule(
        "borrowck",
        "E0502",
        RuleScope::Result,
        RuleGap::Once,
        RuleMode::Remind,
    );
    doc.after = 3;
    let (engine, delivered) = engine_with_sink(vec![doc]);
    let args = r#"{"path":"src/borrow.rs"}"#;
    let body = "error[E0502]: cannot borrow `x` as mutable";
    engine.check_result("bash", args, &format!("Compiling yi v0.1.0\n{body}"), false);
    engine.check_result("bash", args, &format!("Checking yi v0.1.0\n{body}"), false);
    assert!(
        delivered.lock().map_err(|_| "lock")?.is_empty(),
        "two sightings are under the threshold"
    );
    engine.check_result("bash", args, &format!("Building yi v0.1.0\n{body}"), false);
    assert_eq!(
        delivered.lock().map_err(|_| "lock")?.len(),
        1,
        "needle and path are the evidence; the lines above them are not"
    );
    Ok(())
}

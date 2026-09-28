//! The breakpoints' invariants over random requests (D295): at most the engine's slots, a
//! TTL that never rises along the prompt, no position past the history (so none in
//! `transient`), and no tail for a one-shot or a final. The same requests are then rendered
//! through both dialects, and the environment messages come out last and bare.

use std::error::Error;
use std::hash::{BuildHasher, RandomState};

use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};
use proptest::test_runner::{Config, RngSeed, TestCaseError, TestRunner};
use serde_json::{Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::breakpoints::{Breakpoints, CacheRoute, Engine, Position, Reuse, SLOTS, Ttl};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_ai::openai::{self, OpenAiOptions};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolChoice, ToolDef};

const CASES: u32 = 128;

fn model(api: &str, provider: &str, base_url: &str, id: &str) -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: api.to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 4_096,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
}

fn tool_result(id: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: "bash".to_owned(),
        content: vec![Content::Text {
            text: "ok".to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

fn message(kind: u8, index: usize) -> AgentMessage {
    match kind {
        0 => user(&format!("user {index}")),
        1 => faux_assistant_message(vec![faux_text(&format!("reply {index}"))], StopReason::Stop),
        2 => faux_assistant_message(
            vec![faux_tool_call(
                &format!("call-{index}"),
                "bash",
                serde_json::Map::new(),
            )],
            StopReason::ToolUse,
        ),
        3 => tool_result(&format!("call-{}", index.saturating_sub(1))),
        _ => faux_assistant_message(vec![faux_text("cut")], StopReason::Error),
    }
}

/// A random request: history kinds, environment count, reuse, engine and TTL choices.
#[derive(Clone, Debug)]
struct Case {
    kinds: Vec<u8>,
    transient: usize,
    reuse: Reuse,
    engine: Engine,
    hold_1h: bool,
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let reuse = prop_oneof![Just(Reuse::Loop), Just(Reuse::OneShot), Just(Reuse::Final)];
    let engine = prop_oneof![
        (1..=SLOTS, any::<bool>()).prop_map(|(slots, hour)| Engine::Breakpoint { slots, hour }),
        Just(Engine::Snapshot),
        Just(Engine::Prefix),
    ];
    (
        prop::collection::vec(0..5u8, 0..12),
        0..3usize,
        reuse,
        engine,
        any::<bool>(),
    )
        .prop_map(|(kinds, transient, reuse, engine, hold_1h)| Case {
            kinds,
            transient,
            reuse,
            engine,
            hold_1h,
        })
}

fn context(case: &Case) -> LlmContext {
    let tool = ToolDef {
        name: "bash".to_owned(),
        description: "run".to_owned(),
        parameters: json!({"type": "object", "properties": {}}),
        freeform: None,
    };
    let (tools, tool_choice) = match case.reuse {
        Reuse::Loop => (Some(vec![tool]), None),
        Reuse::OneShot => (None, None),
        Reuse::Final => (Some(vec![tool]), Some(ToolChoice::None)),
    };
    LlmContext {
        system_prompt: "be terse".to_owned(),
        messages: case
            .kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| message(*kind, index))
            .collect(),
        transient: (0..case.transient)
            .map(|turn| user(&format!("<environment>\nturn: {turn}\n</environment>")))
            .collect(),
        schema: None,
        tools,
        tool_choice,
    }
}

fn check_breakpoints(case: &Case, ctx: &LlmContext) -> Result<(), TestCaseError> {
    let stable_ttl = match case.engine {
        Engine::Breakpoint { hour: true, .. } if case.hold_1h => Ttl::Hour1,
        _ => Ttl::Min5,
    };
    let route = CacheRoute {
        engine: case.engine,
        stable_ttl,
    };
    let breakpoints = Breakpoints::build(&route, &ctx.messages, case.reuse);
    let marks: Vec<_> = breakpoints.marks().collect();
    let slots = match case.engine {
        Engine::Breakpoint { slots, .. } => slots,
        Engine::Snapshot => 1,
        Engine::Prefix => SLOTS,
    };
    proptest::prop_assert!(marks.len() <= slots, "{marks:?} over {slots} slots");
    proptest::prop_assert_eq!(
        marks.first().map(|mark| mark.position),
        Some(Position::Stable)
    );
    for pair in marks.windows(2) {
        proptest::prop_assert!(pair[1].ttl <= pair[0].ttl, "TTL rises: {marks:?}");
        proptest::prop_assert!(
            pair[1].position.index() > pair[0].position.index(),
            "positions out of prompt order: {marks:?}"
        );
    }
    for mark in &marks {
        if let Some(index) = mark.position.index() {
            proptest::prop_assert!(
                index < ctx.messages.len(),
                "position {index} past the history of {}: transient is in reach",
                ctx.messages.len()
            );
            proptest::prop_assert_eq!(case.reuse, Reuse::Loop, "a tail on {:?}", case.reuse);
        }
    }
    Ok(())
}

/// The environment count rendered last, none of them marked, and marked history messages
/// only on a loop request.
fn check_rendered(case: &Case, body: &Value, dialect: &str) -> Result<(), TestCaseError> {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let marked = |message: &Value| {
        message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()))
    };
    let split = messages.len().saturating_sub(case.transient);
    for (index, message) in messages.iter().enumerate().skip(split) {
        let text = message["content"]
            .as_str()
            .or_else(|| message["content"][0]["text"].as_str())
            .unwrap_or("");
        proptest::prop_assert!(
            text.starts_with("<environment>"),
            "{dialect}: message {index} is not the environment: {message}"
        );
        proptest::prop_assert!(
            !marked(message),
            "{dialect}: a marked environment: {message}"
        );
    }
    let history_marks = messages
        .iter()
        .take(split)
        .filter(|message| !matches!(message["role"].as_str(), Some("system" | "developer")))
        .filter(|message| marked(message))
        .count();
    proptest::prop_assert!(
        case.reuse == Reuse::Loop || history_marks == 0,
        "{dialect}: {history_marks} history marks on {:?}",
        case.reuse
    );
    proptest::prop_assert!(
        body.get("cache_control").is_none(),
        "{dialect}: a root mark"
    );
    Ok(())
}

#[test]
fn random_requests_hold_every_breakpoint_invariant_on_both_dialects() -> Result<(), Box<dyn Error>>
{
    let mut config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = CASES;
    }
    let seed = match config.rng_seed {
        RngSeed::Fixed(seed) => seed,
        RngSeed::Random => RandomState::new().hash_one("breakpoints"),
    };
    config.rng_seed = RngSeed::Fixed(seed);
    let replay = format!("PROPTEST_RNG_SEED={seed} replays this run");
    let direct = model(
        "anthropic-messages",
        "anthropic",
        "https://api.anthropic.com",
        "claude-opus-4-5",
    );
    let routed = model(
        "openai-completions",
        "openrouter",
        "https://openrouter.ai/api/v1",
        "anthropic/claude-haiku-4.5",
    );
    let mut runner = TestRunner::new(config);
    runner
        .run(&case_strategy(), |case| {
            let ctx = context(&case);
            check_breakpoints(&case, &ctx)?;
            let options = AnthropicOptions {
                cache_1h: case.hold_1h,
                ..AnthropicOptions::default()
            };
            check_rendered(
                &case,
                &anthropic::build_params(&direct, &ctx, &options),
                "anthropic",
            )?;
            check_rendered(
                &case,
                &openai::build_params(&routed, &ctx, &OpenAiOptions::default()),
                "openrouter",
            )
        })
        .map_err(|error| format!("{error}; {replay}"))?;
    Ok(())
}

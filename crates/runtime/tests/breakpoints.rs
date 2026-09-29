//! The breakpoints' invariants over random requests (D295): at most the engine's slots, a
//! TTL that never rises along the prompt, no position past the history (so none in
//! `transient`), no tail unless the caller said the request loops, and a system breakpoint
//! on every request. The same requests are then rendered through both dialects: the
//! environment messages come out last and bare, the system end is marked, and a loop
//! request's last mark sits on its last user-role message.

use std::error::Error;
use std::hash::{BuildHasher, RandomState};

use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};
use proptest::test_runner::{Config, RngSeed, TestCaseError, TestRunner};
use serde_json::{Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::breakpoints::{Breakpoints, CachePolicy, Encoded, Engine, Position, SLOTS, Ttl};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_ai::openai::{self, OpenAiOptions};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, Reuse, ToolChoice, ToolDef};

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

fn direct() -> Model {
    model(
        "anthropic-messages",
        "anthropic",
        "https://api.anthropic.com",
        "claude-opus-4-5",
    )
}

fn routed() -> Model {
    model(
        "openai-completions",
        "openrouter",
        "https://openrouter.ai/api/v1",
        "anthropic/claude-haiku-4.5",
    )
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

/// A random request: history kinds, environment count, the caller's reuse, whether a tool
/// table rides (independent of reuse), the engine and the TTL choice.
#[derive(Clone, Debug)]
struct Case {
    kinds: Vec<u8>,
    transient: usize,
    reuse: Reuse,
    tools: bool,
    engine: Engine,
    hold_1h: bool,
    /// `shared_through`, which may point past the history.
    shared: Option<usize>,
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let reuse = prop_oneof![
        Just(Reuse::Loop),
        Just(Reuse::OneShot),
        Just(Reuse::LastTurn),
        Just(Reuse::ReadOnly)
    ];
    let engine = prop_oneof![
        (1..=SLOTS, any::<bool>()).prop_map(|(slots, hour)| Engine::Breakpoint { slots, hour }),
        Just(Engine::Snapshot),
        Just(Engine::Prefix),
    ];
    (
        prop::collection::vec(0..5u8, 0..12),
        0..3usize,
        reuse,
        any::<bool>(),
        engine,
        any::<bool>(),
        proptest::option::of(0..14usize),
    )
        .prop_map(
            |(kinds, transient, reuse, tools, engine, hold_1h, shared)| Case {
                kinds,
                transient,
                reuse,
                tools,
                engine,
                hold_1h,
                shared,
            },
        )
}

fn tool() -> ToolDef {
    ToolDef {
        name: "bash".to_owned(),
        description: "run".to_owned(),
        parameters: json!({"type": "object", "properties": {}}),
        freeform: None,
    }
}

fn context(case: &Case) -> LlmContext {
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
        shared_through: None,
        reuse: case.reuse,
        tools: case.tools.then(|| vec![tool()]),
        tool_choice: (case.reuse == Reuse::LastTurn).then_some(ToolChoice::None),
    }
}

fn is_user_role(message: &AgentMessage) -> bool {
    matches!(
        message,
        AgentMessage::User { .. } | AgentMessage::ToolResult { .. }
    )
}

fn check_breakpoints(case: &Case, ctx: &LlmContext) -> Result<(), TestCaseError> {
    let stable_ttl = match case.engine {
        Engine::Breakpoint { hour: true, .. } if case.hold_1h => Ttl::Hour1,
        _ => Ttl::Min5,
    };
    let policy = CachePolicy {
        engine: case.engine,
        stable_ttl,
    };
    let breakpoints = Breakpoints::build(&policy, &ctx.messages, case.reuse, case.shared);
    let marks: Vec<_> = breakpoints.marks().collect();
    let slots = match case.engine {
        Engine::Breakpoint { slots, .. } => slots,
        Engine::Snapshot => 1,
        Engine::Prefix => SLOTS,
    };
    proptest::prop_assert!(marks.len() <= slots, "{marks:?} over {slots} slots");
    proptest::prop_assert!(
        marks
            .iter()
            .any(|mark| mark.position == Position::SystemEnd),
        "no system breakpoint: {marks:?}"
    );
    for pair in marks.windows(2) {
        proptest::prop_assert!(pair[1].ttl <= pair[0].ttl, "TTL rises: {marks:?}");
        proptest::prop_assert!(
            pair[1].position.rank() > pair[0].position.rank(),
            "positions out of prompt order: {marks:?}"
        );
    }
    let last_user = ctx.messages.iter().rposition(is_user_role);
    let tail = marks.iter().find_map(|mark| match mark.position {
        Position::Tail(index) => Some(index),
        _ => None,
    });
    for mark in &marks {
        if let Some(index) = mark.position.index() {
            proptest::prop_assert!(
                index < ctx.messages.len(),
                "position {index} past the history of {}: transient is in reach",
                ctx.messages.len()
            );
            proptest::prop_assert!(
                matches!(case.reuse, Reuse::Loop | Reuse::ReadOnly)
                    || mark.position == Position::SharedEnd(index),
                "a history mark on {:?}",
                case.reuse
            );
        }
    }
    // Three slots hold the system end, the tail and the shared run's end.
    if let Some(shared) = case.shared.filter(|shared| *shared < ctx.messages.len())
        && slots >= 3
    {
        proptest::prop_assert!(
            marks
                .iter()
                .any(|mark| mark.position.index() == Some(shared)),
            "shared_through {shared} is unmarked: {marks:?}"
        );
    }
    if case.reuse == Reuse::ReadOnly {
        proptest::prop_assert_eq!(tail, None, "a read-only request writes no tail");
    }
    if case.reuse == Reuse::Loop && slots >= 2 {
        proptest::prop_assert_eq!(tail, last_user, "the tail is the last user-role message");
    }
    Ok(())
}

/// The environment count rendered last, none of them marked, the system end marked, and
/// history marks only on a loop request, the last of them on the last user-role message.
fn check_rendered(case: &Case, body: &Encoded, dialect: &str) -> Result<(), TestCaseError> {
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
    let system_marked = match dialect {
        "anthropic" => body["system"]
            .as_array()
            .and_then(|blocks| blocks.last())
            .is_some_and(|block| block["cache_control"].is_object()),
        _ => messages.first().is_some_and(marked),
    };
    proptest::prop_assert!(system_marked, "{dialect}: no system breakpoint on the wire");
    let history: Vec<&Value> = messages
        .iter()
        .take(split)
        .filter(|message| !matches!(message["role"].as_str(), Some("system" | "developer")))
        .collect();
    let last_marked = history.iter().rposition(|message| marked(message));
    let last_user = history
        .iter()
        .rposition(|message| matches!(message["role"].as_str(), Some("user" | "tool")));
    match case.reuse {
        Reuse::Loop => proptest::prop_assert_eq!(
            last_marked,
            last_user,
            "{}: the last history mark is not on the last user-role message",
            dialect
        ),
        // The previous tail sits ahead of the reply that followed it, so never last.
        Reuse::ReadOnly => proptest::prop_assert!(
            last_marked.is_none() || last_marked < last_user,
            "{dialect}: a read-only request marked its own tail"
        ),
        Reuse::OneShot | Reuse::LastTurn => proptest::prop_assert!(
            last_marked.is_none(),
            "{dialect}: a history mark on {:?}",
            case.reuse
        ),
    }
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
    let (direct, routed) = (direct(), routed());
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

/// The caller says the request loops; an empty tool table does not turn that off (review
/// of #796, F1): a tool-less conversation still marks its previous tail and its tail.
#[test]
fn a_tool_less_loop_still_marks_its_previous_tail_and_tail() -> Result<(), Box<dyn Error>> {
    let ctx = LlmContext {
        system_prompt: "be terse".to_owned(),
        messages: vec![
            user("first"),
            faux_assistant_message(vec![faux_text("one")], StopReason::Stop),
            user("second"),
            faux_assistant_message(vec![faux_text("two")], StopReason::Stop),
            user("third"),
        ],
        transient: vec![user("<environment>\nturn: 3\n</environment>")],
        schema: None,
        shared_through: None,
        reuse: Reuse::Loop,
        tools: None,
        tool_choice: None,
    };
    let marked = |messages: &[Value]| -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                message["content"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()))
            })
            .map(|(index, _)| index)
            .collect()
    };
    let anthropic = anthropic::build_params(&direct(), &ctx, &AnthropicOptions::default());
    assert_eq!(
        marked(anthropic["messages"].as_array().ok_or("messages")?),
        [2, 4],
        "{anthropic}"
    );
    let routed = openai::build_params(&routed(), &ctx, &OpenAiOptions::default());
    assert_eq!(
        marked(routed["messages"].as_array().ok_or("messages")?),
        [0, 3, 5],
        "{routed}"
    );
    Ok(())
}

fn marked_messages(body: &Value) -> Vec<usize> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, message)| {
            message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()))
        })
        .map(|(index, _)| index)
        .collect()
}

/// A question-child over a partition a sibling sent (D309): message 0 is that partition, and
/// `shared_through` marks it on both wires, as a read of the sibling's entry. A one-request
/// child marks only the stable prefix and the partition; a looping one adds its previous tail
/// and tail, and the universal end gives way to stay within four.
#[test]
fn a_shared_partition_is_marked_on_both_wires() -> Result<(), Box<dyn Error>> {
    let partition = "<untrusted>\n1:red sun\n2:blue sky\n</untrusted>";
    let mut ctx = LlmContext {
        system_prompt: ["reader", "rules"].join(yi_types::model::SYSTEM_BLOCK_SEPARATOR),
        messages: vec![user(partition), user("Which line names the sky?")],
        transient: Vec::new(),
        schema: None,
        shared_through: Some(0),
        reuse: Reuse::OneShot,
        tools: None,
        tool_choice: None,
    };
    let exact = json!({"role": "user", "content": [
        {"type": "text", "text": partition, "cache_control": {"type": "ephemeral"}}]});
    let anthropic = anthropic::build_params(&direct(), &ctx, &AnthropicOptions::default());
    assert_eq!(marked_messages(&anthropic), [0], "{anthropic}");
    assert_eq!(anthropic["messages"][0].to_string(), exact.to_string());
    let routed_body = openai::build_params(&routed(), &ctx, &OpenAiOptions::default());
    assert_eq!(marked_messages(&routed_body), [0, 1], "{routed_body}");
    assert_eq!(routed_body["messages"][1].to_string(), exact.to_string());

    ctx.reuse = Reuse::Loop;
    ctx.tools = Some(vec![tool()]);
    ctx.messages.push(message(2, 2));
    ctx.messages.push(tool_result("call-2"));
    let anthropic = anthropic::build_params(&direct(), &ctx, &AnthropicOptions::default());
    assert_eq!(marked_messages(&anthropic), [0, 1, 3], "{anthropic}");
    assert!(
        anthropic["system"][0].get("cache_control").is_none(),
        "the universal end takes a fifth slot: {anthropic}"
    );
    let routed_body = openai::build_params(&routed(), &ctx, &OpenAiOptions::default());
    assert_eq!(marked_messages(&routed_body), [0, 1, 2, 4], "{routed_body}");
    Ok(())
}

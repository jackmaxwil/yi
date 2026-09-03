use serde_json::{Value, json};
use std::error::Error;
use yi_ai::anthropic::{AnthropicOptions, Mapper, Thinking, build_params};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

fn model() -> Model {
    let n = |v: u64| serde_json::Number::from(v);
    Model {
        id: "claude-opus-4-5".to_owned(),
        name: "Opus".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://api.anthropic.com".to_owned(),
        reasoning: true,
        input: vec!["text".to_owned(), "image".to_owned()],
        cost: ModelCost {
            input: serde_json::Number::from_f64(5.0)
                .ok_or("n")
                .unwrap_or_else(|_| n(0)),
            output: serde_json::Number::from_f64(25.0).unwrap_or_else(|| n(0)),
            cache_read: serde_json::Number::from_f64(0.5).unwrap_or_else(|| n(0)),
            cache_write: serde_json::Number::from_f64(6.25).unwrap_or_else(|| n(0)),
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn context() -> LlmContext {
    LlmContext {
        system_prompt: "be terse".to_owned(),
        messages: vec![AgentMessage::host_user(
            UserContent::Text("hi".to_owned()),
            0,
        )],
        tools: Some(vec![ToolDef {
            name: "bash".to_owned(),
            description: "run".to_owned(),
            parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}),
            freeform: None,
        }]),
        tool_choice: None,
    }
}

#[test]
fn build_params_places_cache_control_and_tools() -> Result<(), Box<dyn Error>> {
    let options = AnthropicOptions {
        cache: true,
        ..AnthropicOptions::default()
    };
    let params = build_params(&model(), &context(), &options);
    assert_eq!(params["model"], "claude-opus-4-5");
    assert_eq!(params["max_tokens"], 64_000);
    assert_eq!(params["stream"], true);
    assert_eq!(params["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(params["tools"][0]["input_schema"]["type"], "object");
    // Tools precede system, so the system breakpoint already caches them; the
    // freed slot pays for a second system block instead.
    assert!(params["tools"][0]["cache_control"].is_null());
    let last_user = params["messages"]
        .as_array()
        .ok_or("messages")?
        .last()
        .ok_or("last")?;
    let last_block = last_user["content"]
        .as_array()
        .ok_or("content")?
        .last()
        .ok_or("block")?;
    assert_eq!(last_block["cache_control"]["type"], "ephemeral");
    Ok(())
}

/// The assembled prompt carries its own block separators, and each block takes
/// a breakpoint: the universal prefix stays cached when the yard changes.
#[test]
fn system_blocks_split_on_the_separator() -> Result<(), Box<dyn Error>> {
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;
    let options = AnthropicOptions {
        cache: true,
        cache_1h: true,
        ..AnthropicOptions::default()
    };
    let mut context = context();
    context.system_prompt = ["identity", "mode", "yard", "extra"].join(SYSTEM_BLOCK_SEPARATOR);
    let params = build_params(&model(), &context, &options);
    let blocks = params["system"].as_array().ok_or("system")?;
    assert_eq!(
        blocks.len(),
        3,
        "four blocks fold into the three breakpoints"
    );
    assert_eq!(blocks[0]["text"], "identity");
    assert_eq!(blocks[2]["text"], "yard\n\nextra");
    assert_eq!(blocks[0]["cache_control"]["ttl"], "1h");
    Ok(())
}

#[test]
fn thinking_modes_shape_the_body() {
    let mut options = AnthropicOptions {
        thinking: Thinking::Adaptive {
            effort: Some("high".to_owned()),
        },
        ..AnthropicOptions::default()
    };
    let params = build_params(&model(), &context(), &options);
    assert_eq!(params["thinking"]["type"], "adaptive");
    assert_eq!(params["output_config"]["effort"], "high");

    options.thinking = Thinking::Budget { tokens: 0 };
    let params = build_params(&model(), &context(), &options);
    assert_eq!(params["thinking"]["budget_tokens"], 1024);
}

fn canned_events() -> Vec<Value> {
    vec![
        json!({"type":"message_start","message":{"id":"msg_1","model":"claude-opus-4-5","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":50,"cache_creation_input_tokens":10}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"run"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ca"}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"rgo test\"}"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42,"output_tokens_details":{"thinking_tokens":7}}}),
        json!({"type":"message_stop"}),
    ]
}

#[test]
fn mapper_replays_full_stream() -> Result<(), Box<dyn Error>> {
    let model = model();
    let mut mapper = Mapper::new(&model);
    let mut events = vec![mapper.start_event()];
    for payload in canned_events() {
        events.extend(mapper.push(&payload));
    }
    events.push(mapper.finish());

    let last = events.last().ok_or("no events")?;
    let AssistantMessageEvent::Done { reason, message } = last else {
        return Err(format!("expected done, got {last:?}").into());
    };
    assert_eq!(*reason, StopReason::ToolUse);
    let AgentMessage::Assistant {
        content,
        usage,
        response_id,
        raw_stop_reason,
        ..
    } = message
    else {
        return Err("not assistant".into());
    };
    assert_eq!(response_id.as_deref(), Some("msg_1"));
    assert_eq!(raw_stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(content.len(), 3);
    match &content[0] {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            assert_eq!(thinking, "hmm");
            assert_eq!(thinking_signature.as_deref(), Some("sig"));
        }
        other => return Err(format!("expected thinking: {other:?}").into()),
    }
    match &content[2] {
        Content::ToolCall {
            name, arguments, ..
        } => {
            assert_eq!(name, "bash");
            assert_eq!(arguments["cmd"], "cargo test");
        }
        other => return Err(format!("expected tool call: {other:?}").into()),
    }
    assert_eq!(usage.output, 42);
    assert_eq!(usage.reasoning, Some(7));
    assert_eq!(usage.total_tokens, 100 + 42 + 50 + 10);
    let total_cost = usage.cost.total.as_f64().ok_or("cost")?;
    assert!(total_cost > 0.0);
    Ok(())
}

#[test]
fn stream_without_stop_reason_fails() {
    let model = model();
    let mut mapper = Mapper::new(&model);
    let _ =
        mapper.push(&json!({"type":"message_start","message":{"id":"m","model":"x","usage":{}}}));
    let finished = mapper.finish();
    assert!(matches!(
        finished,
        AssistantMessageEvent::Error {
            reason: StopReason::Error,
            ..
        }
    ));
}

/// The 1h retention request is the one thing in the cache layout that no test
/// can confirm without the live API, so its failure has to be survivable.
#[test]
fn refusing_the_hour_long_cache_falls_back_to_the_default() -> Result<(), Box<dyn Error>> {
    use yi_ai::anthropic::{drop_cache_retention, is_cache_retention_rejection};
    assert!(is_cache_retention_rejection(
        "HTTP 400: {\"error\":{\"message\":\"unexpected value for cache_control.ttl\"}}"
    ));
    assert!(is_cache_retention_rejection(
        "HTTP 400: {\"error\":{\"message\":\"unsupported beta: extended-cache-ttl-2025-04-11\"}}"
    ));
    assert!(!is_cache_retention_rejection("HTTP 429: rate limited"));
    assert!(!is_cache_retention_rejection(
        "HTTP 400: max_tokens is too large"
    ));

    let options = AnthropicOptions {
        cache: true,
        cache_1h: true,
        ..AnthropicOptions::default()
    };
    let mut params = build_params(&model(), &context(), &options);
    assert_eq!(params["system"][0]["cache_control"]["ttl"], "1h");
    drop_cache_retention(&mut params);
    assert!(params["system"][0]["cache_control"]["ttl"].is_null());
    assert_eq!(
        params["system"][0]["cache_control"]["type"], "ephemeral",
        "the breakpoint survives; only its retention changes"
    );
    Ok(())
}

#[test]
fn a_message_start_without_a_usage_object_leaves_the_turn_unknown() -> Result<(), Box<dyn Error>> {
    let model = model();
    let mut without = Mapper::new(&model);
    let _ = without.push(&json!({"type":"message_start","message":{"id":"m","model":"x"}}));
    let _ = without.push(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
    let _ = without.push(&json!({"type":"message_stop"}));
    let mut with = Mapper::new(&model);
    let _ = with.push(&json!({"type":"message_start","message":{"id":"m","model":"x","usage":{}}}));
    let _ = with.push(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
    let _ = with.push(&json!({"type":"message_stop"}));

    let flag = |event: AssistantMessageEvent| match event {
        AssistantMessageEvent::Done {
            message: AgentMessage::Assistant { usage, .. },
            ..
        } => Ok(usage.unknown),
        other => Err(format!("expected an assistant done: {other:?}")),
    };
    assert!(
        flag(without.finish())?,
        "no usage object is not a free turn"
    );
    assert!(
        !flag(with.finish())?,
        "an empty usage object is a reported zero, which is known"
    );
    Ok(())
}

/// The fourth cache breakpoint must stay on the last persisted user block: on the
/// trailing environment block, turn N+1 never matches turn N and history re-bills.
#[test]
fn the_breakpoint_stays_ahead_of_a_trailing_environment_block() -> Result<(), Box<dyn Error>> {
    let options = AnthropicOptions {
        cache: true,
        ..AnthropicOptions::default()
    };
    let mut ctx = context();
    ctx.messages.push(AgentMessage::host_user(
        UserContent::Text("<environment>\ncwd: /x\n</environment>".to_owned()),
        0,
    ));
    let params = build_params(&model(), &ctx, &options);
    let messages = params["messages"].as_array().ok_or("messages")?;
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(
        messages[0]["content"][0]["cache_control"]["type"], "ephemeral",
        "{messages:?}"
    );
    assert!(
        messages[1]["content"][0].get("cache_control").is_none(),
        "{messages:?}"
    );
    Ok(())
}

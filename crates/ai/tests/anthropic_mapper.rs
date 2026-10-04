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
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "be terse".to_owned(),
        messages: vec![AgentMessage::user_input(
            UserContent::Text("hi".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
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
    let params = build_params(&model(), &context(), &AnthropicOptions::default());
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

/// The assembled prompt carries its own block separators; the one stable mark sits on the
/// last block, so tools and the whole system prompt are one entry (D295), and it is the
/// mark that carries the interactive session's hour.
#[test]
fn system_blocks_split_on_the_separator() -> Result<(), Box<dyn Error>> {
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;
    let options = AnthropicOptions {
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
    // The universal block keeps its own breakpoint until the system prompt is constant: it is
    // the entry every session and child of the same identity reads across attaches.
    assert_eq!(blocks[0]["cache_control"]["ttl"], "1h", "{blocks:?}");
    assert!(blocks[1].get("cache_control").is_none(), "{blocks:?}");
    assert_eq!(blocks[2]["cache_control"]["ttl"], "1h");
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

/// The tail mark stays on the last persisted user block: the environment rides
/// `transient`, rendered after every mark, so turn N+1 reads turn N instead of re-billing.
#[test]
fn the_breakpoint_stays_ahead_of_a_trailing_environment_block() -> Result<(), Box<dyn Error>> {
    let mut ctx = context();
    ctx.transient.push(AgentMessage::user_input(
        UserContent::Text("<environment>\ncwd: /x\n</environment>".to_owned()),
        0,
    ));
    let params = build_params(&model(), &ctx, &AnthropicOptions::default());
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

fn image_turns(count: usize, image: &str) -> LlmContext {
    let messages = (0..count)
        .map(|turn| {
            AgentMessage::user_input(
                UserContent::Blocks(vec![
                    Content::Text {
                        text: format!("image {turn}"),
                        text_signature: None,
                    },
                    Content::Image {
                        data: image.to_owned(),
                        mime_type: "image/png".to_owned(),
                    },
                ]),
                0,
            )
        })
        .collect();
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: String::new(),
        messages,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: None,
        tool_choice: None,
    }
}

/// The label of each turn whose image the request still carries.
fn sent_images(params: &Value) -> Vec<String> {
    params["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["content"].as_array())
        .filter(|blocks| blocks.iter().any(|block| block["type"] == "image"))
        .filter_map(|blocks| blocks.first()?["text"].as_str().map(str::to_owned))
        .collect()
}

/// Incident: past 20 images a Claude request must hold every image to 2000 px, and every
/// resent image counts, so an image-heavy session was refused on every later turn.
#[test]
fn a_claude_request_keeps_its_newest_twenty_images() -> Result<(), Box<dyn Error>> {
    let params = build_params(
        &model(),
        &image_turns(25, crate::images::PNG),
        &AnthropicOptions::default(),
    );
    let sent = sent_images(&params);
    assert_eq!(sent.len(), 20, "{sent:?}");
    assert_eq!(sent.first().map(String::as_str), Some("image 5"));
    assert_eq!(
        params.to_string().matches("earlier image omitted").count(),
        5
    );
    Ok(())
}

/// Incident: every resent image rides every request, and a Claude request is capped at 32 MB.
#[test]
fn a_claude_request_keeps_its_images_under_the_request_cap() -> Result<(), Box<dyn Error>> {
    let params = build_params(
        &model(),
        &image_turns(4, &crate::images::png_at_cap()),
        &AnthropicOptions::default(),
    );
    let sent = sent_images(&params);
    assert!(
        params.to_string().len() < 32_000_000,
        "{}",
        params.to_string().len()
    );
    assert_eq!(sent.last().map(String::as_str), Some("image 3"), "{sent:?}");
    Ok(())
}

/// Incident: a PNG signature over four zero bytes passed the kernel's header check, and a
/// session saved with it sent the provider an image it refuses on every later request (#860).
#[test]
fn a_refused_image_in_history_is_sent_as_a_placeholder() -> Result<(), Box<dyn Error>> {
    let mut ctx = image_turns(3, crate::images::PNG);
    let AgentMessage::User {
        content: UserContent::Blocks(blocks),
        ..
    } = ctx.messages.get_mut(1).ok_or("turn 1")?
    else {
        return Err("turn 1 is not blocks".into());
    };
    *blocks.get_mut(1).ok_or("image")? = Content::Image {
        data: "iVBORw0KGgoAAAAA".to_owned(),
        mime_type: "image/png".to_owned(),
    };
    let params = build_params(&model(), &ctx, &AnthropicOptions::default());
    assert_eq!(sent_images(&params), ["image 0", "image 2"]);
    let turn = params["messages"][1]["content"].to_string();
    assert!(
        turn.contains(
            "[image omitted: image/png, 0 KB of base64; the image has no whole header for its type"
        ) && turn.contains("re-encode it in ipython"),
        "{turn}"
    );
    Ok(())
}

/// A kernel cell's tool call and its result: text, then the image it attached.
fn image_exchange(api: &str, id: &str) -> Vec<AgentMessage> {
    vec![
        AgentMessage::Assistant {
            content: vec![Content::ToolCall {
                id: id.to_owned(),
                name: "ipython".to_owned(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
                namespace: None,
            }],
            api: api.to_owned(),
            provider: "test".to_owned(),
            model: "m".to_owned(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: yi_types::message::Usage::zero(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        },
        AgentMessage::ToolResult {
            tool_call_id: id.to_owned(),
            tool_name: "ipython".to_owned(),
            content: vec![
                Content::Text {
                    text: "attached".to_owned(),
                    text_signature: None,
                },
                Content::Image {
                    data: crate::images::PNG.to_owned(),
                    mime_type: "image/png".to_owned(),
                },
            ],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        },
    ]
}

#[test]
fn a_tool_result_image_rides_inside_the_tool_result_block() -> Result<(), Box<dyn Error>> {
    let mut ctx = context();
    ctx.messages
        .extend(image_exchange("anthropic-messages", "toolu_1"));
    let params = build_params(&model(), &ctx, &AnthropicOptions::default());
    let messages = params["messages"].as_array().ok_or("messages")?;
    // The tool result is the request's tail, so its block carries the tail mark.
    let mut last = messages.last().ok_or("last")?["content"].clone();
    assert_eq!(last[0]["cache_control"]["type"], "ephemeral", "{last}");
    last[0]
        .as_object_mut()
        .ok_or("block")?
        .remove("cache_control");
    assert_eq!(
        last,
        json!([{"type": "tool_result", "tool_use_id": "toolu_1", "is_error": false, "content": [
            {"type": "text", "text": "attached"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": crate::images::PNG}},
        ]}])
    );
    Ok(())
}

fn assistant(content: Vec<Content>, stop_reason: StopReason) -> AgentMessage {
    AgentMessage::Assistant {
        content,
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "claude-opus-4-5".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// Sessions written before interrupts answered every call hold a turn of three calls with one
/// result and an empty aborted turn; resuming one must still send a request Anthropic accepts.
#[test]
fn a_call_an_old_interrupt_left_unanswered_is_sent_as_an_error_result() -> Result<(), Box<dyn Error>>
{
    let mut ctx = context();
    let calls = ["toolu_0", "toolu_1", "toolu_2"].map(|id| Content::ToolCall {
        id: id.to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::Map::from_iter([("cmd".to_owned(), json!("make test"))]),
        thought_signature: None,
        namespace: None,
    });
    ctx.messages.extend([
        assistant(calls.to_vec(), StopReason::ToolUse),
        AgentMessage::ToolResult {
            tool_call_id: "toolu_0".to_owned(),
            tool_name: "bash".to_owned(),
            content: vec![Content::Text {
                text: "Operation aborted".to_owned(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: true,
            timestamp: 0,
        },
        assistant(Vec::new(), StopReason::Aborted),
        AgentMessage::user_input(UserContent::Text("carry on".to_owned()), 0),
    ]);
    let params = build_params(&model(), &ctx, &AnthropicOptions::default());
    let messages = params["messages"].as_array().ok_or("messages")?;
    assert_eq!(
        messages.get(2).ok_or("the turn after the calls")?["content"],
        json!([
            {"type": "tool_result", "tool_use_id": "toolu_0", "content": "Operation aborted", "is_error": true},
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "No result provided", "is_error": true},
            {"type": "tool_result", "tool_use_id": "toolu_2", "content": "No result provided", "is_error": true},
        ]),
        "every call is answered in the turn right after it: {messages:?}"
    );
    Ok(())
}

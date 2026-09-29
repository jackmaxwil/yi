use serde_json::{Value, json};
use std::error::Error;
use yi_ai::openai::OpenAiOptions;
use yi_ai::openai_responses::{EventMapper, build_params};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Effort, LlmContext, Model, ModelCost, ToolDef};

type TestResult = Result<(), Box<dyn Error>>;

fn model(reasoning: bool) -> Model {
    let n =
        |v: f64| serde_json::Number::from_f64(v).unwrap_or_else(|| serde_json::Number::from(0u64));
    Model {
        id: "gpt-5.6-luna".to_owned(),
        name: "Luna".to_owned(),
        api: "openai-responses".to_owned(),
        provider: "openai".to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        reasoning,
        input: vec!["text".to_owned(), "image".to_owned()],
        cost: ModelCost {
            input: n(0.2),
            output: n(1.2),
            cache_read: n(0.02),
            cache_write: n(0.0),
            tiers: None,
        },
        context_window: 272_000,
        max_tokens: 128_000,
        compat: Some(json!({"supportsExplicitPromptCacheMode": true})),
        thinking_level_map: Some(json!({"off": "none", "high": "high"})),
        headers: None,
    }
}

fn context() -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "be terse".to_owned(),
        messages: vec![AgentMessage::host_user(
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
            parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
            freeform: None,
        }]),
        tool_choice: None,
    }
}

fn canned() -> Vec<Value> {
    vec![
        json!({"type":"response.created","response":{"id":"resp_1"}}),
        json!({"type":"response.ping"}),
        json!({"type":"response.reasoning_text.delta","delta":"think"}),
        json!({"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","encrypted_content":"enc"}}),
        json!({"type":"response.output_text.delta","delta":"hello "}),
        json!({"type":"response.output_text.delta","delta":"world"}),
        json!({"type":"response.output_item.added","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"bash","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"cmd\":"}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"\"ls\"}"}),
        json!({"type":"response.function_call_arguments.done","item_id":"fc_1","call_id":"call_1","name":"bash","arguments":"{\"cmd\":\"ls\"}"}),
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":60},"output_tokens_details":{"reasoning_tokens":5}}}}),
    ]
}

/// A write is billed at its own rate and is not uncached input; leaving it
/// inside `input` overstates the cold part of the prompt by the write.
#[test]
fn cache_write_tokens_come_off_input() -> TestResult {
    let model = model(true);
    let mut mapper = EventMapper::new(&model);
    let mut payloads = canned();
    payloads.pop();
    payloads.push(
        json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
        "usage":{"input_tokens":100,"output_tokens":20,
                 "input_tokens_details":{"cached_tokens":60,"cache_write_tokens":10}}}}),
    );
    for payload in payloads {
        let _ = mapper.push(&payload);
    }
    let events = mapper.finish();
    let Some(AssistantMessageEvent::Done { message, .. }) = events.last() else {
        return Err(format!("expected done: {:?}", events.last()).into());
    };
    let AgentMessage::Assistant { usage, .. } = message else {
        return Err("not assistant".into());
    };
    assert_eq!(
        (usage.input, usage.cache_read, usage.cache_write),
        (30, 60, 10)
    );
    assert_eq!(usage.total_tokens, 120);
    Ok(())
}

#[test]
fn responses_params_are_not_chat_completions() -> TestResult {
    let options = OpenAiOptions {
        max_tokens: Some(4096),
        reasoning_effort: Some(Effort::High),
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    let params = build_params(&model(true), &context(), &options);
    assert!(params.get("messages").is_none());
    assert_eq!(params["store"], false);
    assert_eq!(params["stream"], true);
    assert_eq!(params["max_output_tokens"], 4096);
    assert_eq!(params["prompt_cache_key"], "session-1");
    assert!(params.get("prompt_cache_retention").is_none());
    assert_eq!(params["reasoning"]["effort"], "high");
    assert_eq!(params["include"][0], "reasoning.encrypted_content");
    assert_eq!(params["tools"][0]["type"], "function");
    assert_eq!(params["tools"][0]["name"], "bash");
    assert!(params["tools"][0].get("function").is_none());
    assert!(params.get("previous_response_id").is_none());
    assert_eq!(params["input"][0]["role"], "developer");
    assert_eq!(params["input"][1]["content"][0]["type"], "input_text");
    assert_eq!(params["input"][1]["content"][0]["text"], "hi");
    Ok(())
}

#[test]
fn tool_result_is_a_function_call_output_item() -> TestResult {
    let mut ctx = context();
    ctx.messages.push(AgentMessage::Assistant {
        content: vec![Content::ToolCall {
            id: "call_1|fc_1".to_owned(),
            name: "bash".to_owned(),
            arguments: {
                let mut map = serde_json::Map::new();
                map.insert("cmd".to_owned(), json!("ls"));
                map
            },
            thought_signature: None,
            namespace: None,
        }],
        api: "openai-responses".to_owned(),
        provider: "openai".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
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
    });
    ctx.messages.push(AgentMessage::ToolResult {
        tool_call_id: "call_1|fc_1".to_owned(),
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
    });
    let params = build_params(&model(true), &ctx, &OpenAiOptions::default());
    let input = params["input"].as_array().ok_or("input array")?;
    let output = input
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .ok_or("missing function_call_output")?;
    assert_eq!(output["call_id"], "call_1");
    assert_eq!(output["output"], "ok");
    Ok(())
}

#[test]
fn mapper_replays_typed_sse_without_done_sentinel() -> TestResult {
    let model = model(true);
    let mut mapper = EventMapper::new(&model);
    let mut events = vec![mapper.start_event()];
    for payload in canned() {
        events.extend(mapper.push(&payload));
    }
    events.extend(mapper.finish());
    let last = events.last().ok_or("empty")?;
    let AssistantMessageEvent::Done { reason, message } = last else {
        return Err(format!("expected done: {last:?}").into());
    };
    assert_eq!(*reason, StopReason::ToolUse);
    let AgentMessage::Assistant {
        content,
        usage,
        response_id,
        ..
    } = message
    else {
        return Err("not assistant".into());
    };
    assert_eq!(response_id.as_deref(), Some("resp_1"));
    match &content[0] {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            assert_eq!(thinking, "think");
            assert!(
                thinking_signature
                    .as_deref()
                    .is_some_and(|signature| signature.contains("encrypted_content")),
                "{thinking_signature:?}"
            );
        }
        other => return Err(format!("expected thinking: {other:?}").into()),
    }
    match &content[1] {
        Content::Text { text, .. } => assert_eq!(text, "hello world"),
        other => return Err(format!("expected text: {other:?}").into()),
    }
    match &content[2] {
        Content::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            assert_eq!(id, "call_1|fc_1");
            assert_eq!(name, "bash");
            assert_eq!(arguments["cmd"], "ls");
        }
        other => return Err(format!("expected tool call: {other:?}").into()),
    }
    assert_eq!(usage.input, 40);
    assert_eq!(usage.cache_read, 60);
    assert_eq!(usage.reasoning, Some(5));
    Ok(())
}

#[test]
fn missing_completed_event_is_an_error() {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let _ = mapper.push(&json!({"type":"response.output_text.delta","delta":"hi"}));
    let events = mapper.finish();
    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: AgentMessage::Assistant {
                error_message: Some(message),
                ..
            }
        }) if message == "Stream ended without response.completed"
    ));
}

#[test]
fn failed_event_is_the_turn_error() {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let events = mapper.push(&json!({
        "type": "error",
        "error": {"code": "server_error", "message": "The model failed to generate a response."}
    }));
    assert!(matches!(
        events.first(),
        Some(AssistantMessageEvent::Error {
            error: AgentMessage::Assistant {
                error_message: Some(message),
                ..
            },
            ..
        }) if message == "The model failed to generate a response."
    ));
    assert!(mapper.finish().is_empty());
}

#[test]
fn failed_response_event_reads_nested_error() {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let events = mapper.push(&json!({
        "type": "response.failed",
        "response": {
            "id": "resp_failed",
            "status": "failed",
            "error": {
                "code": "insufficient_quota",
                "message": "You exceeded your current quota"
            }
        }
    }));
    assert!(matches!(
        events.first(),
        Some(AssistantMessageEvent::Error {
            error: AgentMessage::Assistant {
                error_message: Some(message),
                ..
            },
            ..
        }) if message == "You exceeded your current quota"
    ));
    assert!(mapper.finish().is_empty());
}

#[test]
fn incomplete_is_a_length_terminal() -> TestResult {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let _ = mapper.push(&json!({"type":"response.output_text.delta","delta":"partial"}));
    let _ = mapper.push(&json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}
        }
    }));
    let events = mapper.finish();
    let last = events.last().ok_or("empty")?;
    let AssistantMessageEvent::Done { reason, message } = last else {
        return Err(format!("expected done: {last:?}").into());
    };
    assert_eq!(*reason, StopReason::Length);
    let AgentMessage::Assistant { response_id, .. } = message else {
        return Err("not assistant".into());
    };
    assert_eq!(response_id.as_deref(), Some("resp_incomplete"));
    Ok(())
}

fn done_tool_call(events: &[AssistantMessageEvent]) -> Result<Vec<Content>, Box<dyn Error>> {
    let AssistantMessageEvent::Done { message, .. } = events.last().ok_or("empty")? else {
        return Err("expected done".into());
    };
    let AgentMessage::Assistant { content, .. } = message else {
        return Err("not assistant".into());
    };
    Ok(content.clone())
}

#[test]
fn dropped_item_added_still_yields_a_dispatchable_tool_call() -> TestResult {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let _ = mapper.push(
        &json!({"type":"response.function_call_arguments.delta","item_id":"fc_9","delta":"{\"cmd\":"}),
    );
    let _ = mapper.push(
        &json!({"type":"response.function_call_arguments.delta","item_id":"fc_9","delta":"\"ls\"}"}),
    );
    let _ = mapper.push(
        &json!({"type":"response.function_call_arguments.done","item_id":"fc_9","call_id":"call_9","name":"bash","arguments":"{\"cmd\":\"ls\"}"}),
    );
    let _ = mapper.push(&json!({"type":"response.completed","response":{"id":"r9"}}));
    let content = done_tool_call(&mapper.finish())?;
    match content.as_slice() {
        [
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            },
        ] => {
            assert_eq!(id, "call_9|fc_9");
            assert_eq!(name, "bash");
            assert_eq!(arguments["cmd"], "ls");
            Ok(())
        }
        other => Err(format!("expected one tool call: {other:?}").into()),
    }
}

#[test]
fn id_less_argument_deltas_share_one_tool_call() -> TestResult {
    let model = model(false);
    let mut mapper = EventMapper::new(&model);
    let _ =
        mapper.push(&json!({"type":"response.function_call_arguments.delta","delta":"{\"cmd\":"}));
    let _ =
        mapper.push(&json!({"type":"response.function_call_arguments.delta","delta":"\"ls\"}"}));
    let _ = mapper.push(
        &json!({"type":"response.function_call_arguments.done","name":"bash","arguments":"{\"cmd\":\"ls\"}"}),
    );
    let _ = mapper.push(&json!({"type":"response.completed","response":{"id":"r10"}}));
    let content = done_tool_call(&mapper.finish())?;
    let calls: Vec<_> = content
        .iter()
        .filter(|block| matches!(block, Content::ToolCall { .. }))
        .collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    match calls.first() {
        Some(Content::ToolCall {
            name, arguments, ..
        }) => {
            assert_eq!(name, "bash");
            assert_eq!(arguments["cmd"], "ls");
            Ok(())
        }
        other => Err(format!("expected tool call: {other:?}").into()),
    }
}

#[test]
fn the_clamp_picks_the_level_and_the_map_names_it() -> TestResult {
    let mut luna = model(true);
    luna.thinking_level_map = Some(json!({
        "off": "none", "minimal": null, "low": "low", "medium": "medium",
        "high": "high", "xhigh": null, "max": null,
    }));
    for (requested, expected) in [(Effort::Max, "high"), (Effort::Minimal, "low")] {
        let params = build_params(
            &luna,
            &context(),
            &OpenAiOptions {
                reasoning_effort: Some(luna.clamp_effort(requested)),
                ..OpenAiOptions::default()
            },
        );
        assert_eq!(params["reasoning"]["effort"], expected);
    }
    Ok(())
}

#[test]
fn a_rejected_level_reaching_the_wire_sends_no_reasoning() -> TestResult {
    let mut luna = model(true);
    luna.thinking_level_map = Some(json!({"max": null}));
    let params = build_params(
        &luna,
        &context(),
        &OpenAiOptions {
            reasoning_effort: Some(Effort::Max),
            ..OpenAiOptions::default()
        },
    );
    assert!(params.get("reasoning").is_none());
    Ok(())
}

#[test]
fn reasoning_is_omitted_when_the_model_supports_no_level() -> TestResult {
    let mut luna = model(true);
    luna.thinking_level_map = Some(json!({
        "off": null, "minimal": null, "low": null, "medium": null,
        "high": null, "xhigh": null, "max": null,
    }));
    let params = build_params(
        &luna,
        &context(),
        &OpenAiOptions {
            reasoning_effort: Some(Effort::High),
            ..OpenAiOptions::default()
        },
    );
    assert!(params.get("reasoning").is_none());
    assert!(params.get("include").is_none());
    Ok(())
}

/// C8: a freeform tool goes on the wire as a custom tool with its grammar;
/// its history replays as custom_tool_call items and custom outputs, and
/// plain function tools are untouched beside it.
#[test]
fn freeform_tool_serializes_as_custom_with_grammar() -> TestResult {
    let options = OpenAiOptions {
        max_tokens: Some(1024),
        ..OpenAiOptions::default()
    };
    let mut context = context();
    if let Some(tools) = &mut context.tools {
        tools.push(ToolDef {
            name: "edit".to_owned(),
            description: "patch".to_owned(),
            parameters: json!({"type":"object"}),
            freeform: Some(yi_types::model::FreeformFormat {
                definition: "start: body".to_owned(),
            }),
        });
    }
    let mut arguments = serde_json::Map::new();
    arguments.insert("patch".to_owned(), json!("[a.rs#1A2B]\nPUT 1.=1:\n+x"));
    context.messages.push(AgentMessage::Assistant {
        content: vec![Content::ToolCall {
            id: "call_9".to_owned(),
            name: "edit".to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
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
    });
    context.messages.push(AgentMessage::ToolResult {
        tool_call_id: "call_9".to_owned(),
        tool_name: "edit".to_owned(),
        content: vec![Content::Text {
            text: "updated".to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    });
    let params = build_params(&model(false), &context, &options);
    assert_eq!(params["tools"][0]["type"], "function");
    assert_eq!(params["tools"][1]["type"], "custom");
    assert_eq!(params["tools"][1]["format"]["syntax"], "lark");
    assert_eq!(params["tools"][1]["format"]["definition"], "start: body");
    let input = params["input"].as_array().ok_or("input not array")?;
    let call = input
        .iter()
        .find(|item| item["type"] == "custom_tool_call")
        .ok_or("no custom_tool_call replay")?;
    assert_eq!(call["input"], "[a.rs#1A2B]\nPUT 1.=1:\n+x");
    assert!(
        input
            .iter()
            .any(|item| item["type"] == "custom_tool_call_output"),
        "freeform result replays as custom output"
    );
    Ok(())
}

/// A streamed custom tool call lands as one tool call whose raw input
/// becomes the `patch` argument, no JSON parsing involved.
#[test]
fn custom_tool_call_stream_maps_to_patch_argument() -> TestResult {
    let mut mapper = EventMapper::new(&model(false));
    let mut events = Vec::new();
    for payload in [
        json!({"type":"response.created","response":{"id":"resp_9"}}),
        json!({"type":"response.output_item.added","item":{"type":"custom_tool_call","call_id":"call_7","name":"edit","input":""}}),
        json!({"type":"response.custom_tool_call_input.delta","call_id":"call_7","delta":"[a.rs#1A2B]\n"}),
        json!({"type":"response.custom_tool_call_input.delta","call_id":"call_7","delta":"PUT 1.=1:\n+y"}),
        json!({"type":"response.output_item.done","item":{"type":"custom_tool_call","call_id":"call_7","name":"edit","input":"[a.rs#1A2B]\nPUT 1.=1:\n+y"}}),
        json!({"type":"response.completed","response":{"id":"resp_9","usage":{"input_tokens":1,"output_tokens":1}}}),
    ] {
        events.extend(mapper.push(&payload));
    }
    events.extend(mapper.finish());
    let done = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::Done { message, .. } => Some(message),
            _ => None,
        })
        .next_back()
        .ok_or("no done event")?;
    let AgentMessage::Assistant { content, .. } = done else {
        return Err("not assistant".into());
    };
    let Some(Content::ToolCall {
        name, arguments, ..
    }) = content
        .iter()
        .find(|block| matches!(block, Content::ToolCall { .. }))
    else {
        return Err("no tool call".into());
    };
    assert_eq!(name, "edit");
    assert_eq!(
        arguments.get("patch").and_then(Value::as_str),
        Some("[a.rs#1A2B]\nPUT 1.=1:\n+y")
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
                    data: "iVBORw0KGgo=".to_owned(),
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
fn a_tool_result_image_is_an_input_image_in_the_output() -> TestResult {
    let mut ctx = context();
    ctx.messages
        .extend(image_exchange("openai-responses", "call_1|fc_1"));
    let params = build_params(&model(true), &ctx, &OpenAiOptions::default());
    let input = params["input"].as_array().ok_or("input array")?;
    assert_eq!(
        input.last().ok_or("last")?,
        &json!({"type": "function_call_output", "call_id": "call_1", "output": [
            {"type": "input_text", "text": "attached"},
            {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,iVBORw0KGgo="},
        ]})
    );
    Ok(())
}

/// The system prompt's block separator reaches the Responses wire as a paragraph break, in
/// the `input` system message and in codex's top-level `instructions`, never as a raw
/// `\u{1d}` (#743, the #771 follow-up).
#[test]
fn the_responses_wire_carries_no_raw_block_separator() -> TestResult {
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;
    let mut ctx = context();
    ctx.system_prompt = ["identity", "mode", "yard"].join(SYSTEM_BLOCK_SEPARATOR);
    let params = build_params(&model(true), &ctx, &OpenAiOptions::default());
    assert_eq!(params["input"][0]["content"], "identity\n\nmode\n\nyard");
    let codex = Model {
        provider: "openai-codex".to_owned(),
        ..model(true)
    };
    let params = build_params(&codex, &ctx, &OpenAiOptions::default());
    assert_eq!(params["instructions"], "identity\n\nmode\n\nyard");
    assert!(
        !params.to_string().contains(SYSTEM_BLOCK_SEPARATOR),
        "{params}"
    );
    Ok(())
}

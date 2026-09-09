use serde_json::{Value, json};
use std::error::Error;
use yi_ai::openai::{ChunkMapper, OpenAiOptions, build_params, normalize_openai_tool_call_id};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Effort, LlmContext, Model, ModelCost, ToolDef};

fn model(reasoning: bool) -> Model {
    let n =
        |v: f64| serde_json::Number::from_f64(v).unwrap_or_else(|| serde_json::Number::from(0u64));
    Model {
        id: "gpt-5.2".to_owned(),
        name: "GPT".to_owned(),
        api: "openai-completions".to_owned(),
        provider: "openai".to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        reasoning,
        input: vec!["text".to_owned(), "image".to_owned()],
        cost: ModelCost {
            input: n(1.25),
            output: n(10.0),
            cache_read: n(0.125),
            cache_write: n(0.0),
            tiers: None,
        },
        context_window: 400_000,
        max_tokens: 128_000,
        compat: None,
        thinking_level_map: Some(json!({"off": "none", "high": "high"})),
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
            parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
            freeform: None,
        }]),
        tool_choice: None,
    }
}

#[test]
fn build_params_openai_shape() -> Result<(), Box<dyn Error>> {
    let options = OpenAiOptions {
        max_tokens: Some(4096),
        reasoning_effort: Some(Effort::High),
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    let params = build_params(&model(true), &context(), &options);
    assert_eq!(params["store"], false);
    assert_eq!(params["stream_options"]["include_usage"], true);
    assert_eq!(params["max_completion_tokens"], 4096);
    assert_eq!(params["reasoning_effort"], "high");
    assert_eq!(params["prompt_cache_key"], "session-1");
    assert_eq!(params["messages"][0]["role"], "developer");
    assert_eq!(params["tools"][0]["function"]["name"], "bash");
    let non_reasoning = build_params(&model(false), &context(), &OpenAiOptions::default());
    assert_eq!(non_reasoning["messages"][0]["role"], "system");
    Ok(())
}

/// Retention is free where it is accepted and a dead turn where it is not, so
/// the parameter follows the id prefix and the catalog flag, never the host alone.
#[test]
fn extended_retention_follows_the_id_prefix_and_the_catalog_flag() {
    let params = build_params(&model(false), &context(), &OpenAiOptions::default());
    assert_eq!(params["prompt_cache_retention"], "24h");
    let mut plain = model(false);
    plain.id = "gpt-5".to_owned();
    let params = build_params(&plain, &context(), &OpenAiOptions::default());
    assert!(params.get("prompt_cache_retention").is_none());
    let mut explicit = model(false);
    explicit.id = "gpt-5.6-luna".to_owned();
    explicit.compat = Some(json!({"supportsExplicitPromptCacheMode": true}));
    let params = build_params(&explicit, &context(), &OpenAiOptions::default());
    assert!(params.get("prompt_cache_retention").is_none());
    let mut routed = model(false);
    routed.base_url = "https://openrouter.ai/api/v1".to_owned();
    let params = build_params(&routed, &context(), &OpenAiOptions::default());
    assert!(params.get("prompt_cache_retention").is_none());
}

#[test]
fn id_normalization_handles_pipe_ids() {
    let long_id = format!("call_abc|{}", "x".repeat(400));
    let normalized = normalize_openai_tool_call_id(&long_id);
    assert!(normalized.len() <= 40);
    assert!(normalized.starts_with("call_abc"));
    assert_eq!(normalize_openai_tool_call_id("short"), "short");
}

fn chunks() -> Vec<Value> {
    vec![
        json!({"id":"chatcmpl-1","choices":[{"delta":{"reasoning_content":"think"}}]}),
        json!({"id":"chatcmpl-1","choices":[{"delta":{"content":"hello "}}]}),
        json!({"id":"chatcmpl-1","choices":[{"delta":{"content":"world"}}]}),
        json!({"id":"chatcmpl-1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"cmd\":"}}]}}]}),
        json!({"id":"chatcmpl-1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}),
        json!({"id":"chatcmpl-1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        json!({"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":60},"completion_tokens_details":{"reasoning_tokens":5}}}),
    ]
}

#[test]
fn chunk_mapper_replays_stream() -> Result<(), Box<dyn Error>> {
    let model = model(true);
    let mut mapper = ChunkMapper::new(&model);
    let mut events = vec![mapper.start_event()];
    for chunk in chunks() {
        events.extend(mapper.push_chunk(&chunk));
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
    assert_eq!(response_id.as_deref(), Some("chatcmpl-1"));
    assert_eq!(content.len(), 3);
    match &content[0] {
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            assert_eq!(thinking, "think");
            assert_eq!(thinking_signature.as_deref(), Some("reasoning_content"));
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
            assert_eq!(id, "call_1");
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
fn missing_finish_reason_is_an_error() {
    let model = model(false);
    let mut mapper = ChunkMapper::new(&model);
    let _ = mapper.push_chunk(&json!({"id":"c","choices":[{"delta":{"content":"hi"}}]}));
    let events = mapper.finish();
    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Error {
            reason: StopReason::Error,
            ..
        })
    ));
}

#[test]
fn an_absent_usage_object_is_unknown_not_free() -> Result<(), Box<dyn Error>> {
    let model = model(false);
    let mut mapper = ChunkMapper::new(&model);
    for chunk in [
        json!({"id":"chatcmpl-2","choices":[{"delta":{"content":"hi"}}]}),
        json!({"id":"chatcmpl-2","choices":[{"delta":{},"finish_reason":"stop"}]}),
    ] {
        let _ = mapper.push_chunk(&chunk);
    }
    let events = mapper.finish();
    let Some(AssistantMessageEvent::Done { message, .. }) = events.last() else {
        return Err(format!("expected done: {events:?}").into());
    };
    let AgentMessage::Assistant { usage, .. } = message else {
        return Err("not assistant".into());
    };
    assert!(
        usage.unknown,
        "a stream with no usage chunk is not a free turn"
    );
    assert_eq!(usage.total_tokens, 0);
    Ok(())
}

#[test]
fn a_present_usage_object_is_known() -> Result<(), Box<dyn Error>> {
    let model = model(false);
    let mut mapper = ChunkMapper::new(&model);
    for chunk in [
        json!({"id":"chatcmpl-3","choices":[{"delta":{},"finish_reason":"stop"}]}),
        json!({"id":"chatcmpl-3","choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0}}),
    ] {
        let _ = mapper.push_chunk(&chunk);
    }
    let events = mapper.finish();
    let Some(AssistantMessageEvent::Done { message, .. }) = events.last() else {
        return Err(format!("expected done: {events:?}").into());
    };
    let AgentMessage::Assistant { usage, .. } = message else {
        return Err("not assistant".into());
    };
    assert!(!usage.unknown, "a reported zero is a known free turn");
    Ok(())
}

/// D163: a stream dropped at the reasoning budget has no finish reason; marked cut, the mapper
/// ends it as a length `Done` that keeps the generation id, never as an error.
#[test]
fn a_cut_stream_finishes_as_a_length_done_with_its_id() -> Result<(), Box<dyn Error>> {
    let model = model(true);
    let mut mapper = ChunkMapper::new(&model);
    let mut events = vec![mapper.start_event()];
    events.extend(mapper.push_chunk(&json!({
        "id": "gen-cut-1",
        "choices": [{"index": 0, "delta": {"reasoning": "the router must rise at y=2, no, y=3, "}}]
    })));
    mapper.cut();
    events.extend(mapper.finish());
    let last = events.last().ok_or("empty")?;
    let AssistantMessageEvent::Done { reason, message } = last else {
        return Err(format!("a cut ends as done, not an error: {last:?}").into());
    };
    assert_eq!(*reason, StopReason::Length);
    let AgentMessage::Assistant {
        response_id, usage, ..
    } = message
    else {
        return Err("not assistant".into());
    };
    assert_eq!(response_id.as_deref(), Some("gen-cut-1"));
    assert!(usage.unknown, "no usage chunk came; the record settles it");
    Ok(())
}

/// #331: a stream that dies after its chunks reported usage keeps that usage on the error
/// turn, so the loop's retry rule and the ledger both read what the drop cost.
#[test]
fn a_stream_that_dies_mid_turn_keeps_the_usage_it_reported() -> Result<(), Box<dyn Error>> {
    let model = model(true);
    let mut mapper = ChunkMapper::new(&model);
    let _ = mapper.start_event();
    let _ = mapper.push_chunk(&json!({
        "id": "gen-dead-1",
        "choices": [{"index": 0, "delta": {"reasoning": "the router must rise"}}],
        "usage": {"prompt_tokens": 500, "completion_tokens": 40, "completion_tokens_details": {"reasoning_tokens": 40}}
    }));
    let event = mapper.fail("Network connection lost. (upstream Wafer, code 502)");
    let AssistantMessageEvent::Error { reason, error } = event else {
        return Err("expected an error event".into());
    };
    assert_eq!(reason, StopReason::Error);
    let AgentMessage::Assistant {
        usage,
        response_id,
        error_message,
        ..
    } = error
    else {
        return Err("not assistant".into());
    };
    assert_eq!(response_id.as_deref(), Some("gen-dead-1"));
    assert_eq!((usage.input, usage.output, usage.unknown), (500, 40, false));
    assert!(error_message.as_deref().unwrap_or("").contains("502"));
    Ok(())
}

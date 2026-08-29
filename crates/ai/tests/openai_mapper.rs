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
        messages: vec![AgentMessage::User {
            content: UserContent::Text("hi".to_owned()),
            timestamp: 0,
        }],
        tools: Some(vec![ToolDef {
            name: "bash".to_owned(),
            description: "run".to_owned(),
            parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
        }]),
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

use serde_json::json;
use std::error::Error;
use yi_ai::catalog::Catalog;
use yi_ai::openai::{ChunkMapper, OpenAiOptions, build_params};
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{Effort, LlmContext, Model};

type TestResult = Result<(), Box<dyn Error>>;

const TARGET: &str = "deepseek/deepseek-v4-flash-0731";

fn target_model() -> Result<Model, Box<dyn Error>> {
    Catalog::bundled()
        .get("openrouter", TARGET)
        .cloned()
        .ok_or_else(|| format!("bundled catalog is missing openrouter/{TARGET}").into())
}

fn history_context() -> LlmContext {
    LlmContext {
        system_prompt: "be terse".to_owned(),
        messages: vec![
            AgentMessage::host_user(UserContent::Text("hi".to_owned()), 0),
            AgentMessage::Assistant {
                content: vec![Content::Text {
                    text: "hello".to_owned(),
                    text_signature: None,
                }],
                api: "openai-completions".to_owned(),
                provider: "openrouter".to_owned(),
                model: TARGET.to_owned(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::zero(),
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            },
            AgentMessage::host_user(UserContent::Text("again".to_owned()), 0),
        ],
        tools: None,
        tool_choice: None,
    }
}

#[test]
fn bundled_catalog_resolves_the_dev_target_model() -> TestResult {
    let model = target_model()?;
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.base_url, "https://openrouter.ai/api/v1");
    assert!(model.reasoning);
    Ok(())
}

#[test]
fn build_params_honors_the_openrouter_compat_flags() -> TestResult {
    let model = target_model()?;
    let params = build_params(
        &model,
        &history_context(),
        &OpenAiOptions {
            reasoning_effort: Some(Effort::High),
            session_id: Some("session-1".to_owned()),
            ..OpenAiOptions::default()
        },
    );

    assert_eq!(params["messages"][0]["role"], "system");
    assert_eq!(params["reasoning"], json!({"effort": "high"}));
    assert!(params.get("reasoning_effort").is_none());
    assert!(params.get("store").is_none());
    assert!(params.get("prompt_cache_key").is_none());
    assert!(
        params.get("cache_control").is_none(),
        "an automatic-cache route gets no breakpoint"
    );
    let assistant = &params["messages"][2];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["reasoning_content"], "");
    Ok(())
}

/// OpenRouter only forwards a breakpoint it is told about, and an Anthropic
/// route without one re-bills the whole prefix every turn.
#[test]
fn anthropic_routed_models_get_a_root_breakpoint() -> TestResult {
    let model = model("anthropic/claude-haiku-4.5")?;
    let params = build_params(&model, &history_context(), &OpenAiOptions::default());
    assert_eq!(params["cache_control"], json!({"type": "ephemeral"}));
    Ok(())
}

#[test]
fn build_params_defaults_reasoning_effort_to_none_when_unset() -> TestResult {
    let model = target_model()?;
    let params = build_params(&model, &history_context(), &OpenAiOptions::default());
    assert_eq!(params["reasoning"], json!({"effort": "none"}));
    Ok(())
}

#[test]
fn chunk_mapper_captures_openrouter_reasoning_deltas_and_cached_usage() -> TestResult {
    let model = target_model()?;
    let mut mapper = ChunkMapper::new(&model);
    for chunk in [
        json!({"id": "gen-1", "choices": [{"delta": {"reasoning": "thinking about it"}}]}),
        json!({"id": "gen-1", "choices": [{"delta": {"content": "the answer"}}]}),
        json!({"id": "gen-1", "choices": [{"delta": {}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 100, "completion_tokens": 20, "cost": 0.000123,
                          "prompt_tokens_details": {"cached_tokens": 60}}}),
    ] {
        mapper.push_chunk(&chunk);
    }
    let events = mapper.finish();
    let Some(yi_types::event::AssistantMessageEvent::Done { reason, message }) = events.last()
    else {
        return Err(format!("expected done event: {:?}", events.last()).into());
    };
    let AgentMessage::Assistant { content, usage, .. } = message else {
        return Err("expected assistant message".into());
    };
    assert_eq!(*reason, StopReason::Stop);
    assert!(content.iter().any(|block| matches!(
        block,
        Content::Thinking { thinking, thinking_signature, .. }
            if thinking == "thinking about it" && thinking_signature.as_deref() == Some("reasoning")
    )));
    assert!(content.iter().any(|block| matches!(
        block,
        Content::Text { text, .. } if text == "the answer"
    )));
    assert_eq!(usage.cache_read, 60);
    assert_eq!(usage.input, 40);
    assert_eq!(usage.output, 20);
    assert_eq!(
        usage.cost.total.as_f64(),
        Some(0.000123),
        "the account charge outranks the catalog estimate"
    );
    Ok(())
}

fn model(id: &str) -> Result<Model, Box<dyn Error>> {
    Catalog::bundled()
        .get("openrouter", id)
        .cloned()
        .ok_or_else(|| format!("bundled catalog is missing openrouter/{id}").into())
}

#[test]
fn build_params_omits_reasoning_for_a_mandatory_reasoning_model() -> TestResult {
    let model = model("z-ai/glm-5.3-flash")?;
    let params = build_params(&model, &history_context(), &OpenAiOptions::default());
    assert!(params.get("reasoning").is_none());
    Ok(())
}

#[test]
fn build_params_maps_efforts_the_model_does_not_support() -> TestResult {
    let model = model("z-ai/glm-5.3-flash")?;
    let params = build_params(
        &model,
        &history_context(),
        &OpenAiOptions {
            reasoning_effort: Some(Effort::Medium),
            ..OpenAiOptions::default()
        },
    );
    assert_eq!(params["reasoning"], json!({"effort": "low"}));
    Ok(())
}

#[test]
fn a_null_non_off_level_clamps_instead_of_being_sent_verbatim() -> TestResult {
    let mut model = model("z-ai/glm-5.3-flash")?;
    model.thinking_level_map = Some(json!({
        "off": null, "minimal": null, "low": "low", "medium": null,
        "high": "high", "xhigh": null, "max": null,
    }));
    assert_eq!(model.supported_efforts(), vec![Effort::Low, Effort::High]);
    let params = build_params(
        &model,
        &history_context(),
        &OpenAiOptions {
            reasoning_effort: Some(model.clamp_effort(Effort::XHigh)),
            ..OpenAiOptions::default()
        },
    );
    assert_eq!(params["reasoning"], json!({"effort": "high"}));
    Ok(())
}

#[test]
fn openrouter_requests_deprioritise_slow_upstreams_unless_the_config_says_otherwise() -> TestResult
{
    let model = target_model()?;
    let context = history_context();
    let params = build_params(&model, &context, &OpenAiOptions::default());
    assert_eq!(
        params["provider"],
        json!({"preferred_min_throughput": {"p50": 20}, "preferred_max_latency": {"p50": 10}})
    );
    assert!(params["provider"].get("sort").is_none());
    let options = OpenAiOptions {
        routing: Some(json!({"sort": "price", "ignore": ["wafer"]})),
        ..OpenAiOptions::default()
    };
    let params = build_params(&model, &context, &options);
    assert_eq!(
        params["provider"],
        json!({"sort": "price", "ignore": ["wafer"]})
    );
    let cleared = OpenAiOptions {
        routing: Some(json!({})),
        ..OpenAiOptions::default()
    };
    assert_eq!(
        build_params(&model, &context, &cleared)["provider"],
        json!({})
    );
    let mut elsewhere = model;
    elsewhere.base_url = "https://api.openai.com/v1".to_owned();
    assert!(
        build_params(&elsewhere, &context, &OpenAiOptions::default())
            .get("provider")
            .is_none()
    );
    Ok(())
}

/// Row 0028 paid twice the catalog on an upstream nothing recorded: every chunk names the
/// upstream OpenRouter routed to, and the message keeps it once.
#[test]
fn a_chunk_that_names_its_upstream_leaves_one_upstream_diagnostic() -> TestResult {
    let model = target_model()?;
    let mut mapper = ChunkMapper::new(&model);
    for chunk in [
        json!({"id": "gen-1", "provider": "Z.AI", "choices": [{"delta": {"content": "the answer"}}]}),
        json!({"id": "gen-1", "provider": "Z.AI", "choices": [{"delta": {}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 10, "completion_tokens": 2}}),
    ] {
        let _ = mapper.push_chunk(&chunk);
    }
    let Some(yi_types::event::AssistantMessageEvent::Done { message, .. }) = mapper.finish().pop()
    else {
        return Err("expected a done event".into());
    };
    let AgentMessage::Assistant { diagnostics, .. } = message else {
        return Err("expected an assistant message".into());
    };
    let notes = diagnostics.ok_or("the upstream is kept as a diagnostic")?;
    let upstreams: Vec<_> = notes
        .iter()
        .filter(|note| note.diagnostic_type == "upstream")
        .map(|note| {
            note.details
                .as_ref()
                .and_then(|details| details.get("provider"))
        })
        .collect();
    assert_eq!(upstreams, [Some(&json!("Z.AI"))], "{notes:?}");
    Ok(())
}

#[test]
fn a_mid_stream_error_chunk_names_the_upstream_and_the_code() -> TestResult {
    let model = target_model()?;
    let mut mapper = ChunkMapper::new(&model);
    let _ =
        mapper.push_chunk(&json!({"id": "gen-1", "choices": [{"delta": {"reasoning": "hmm"}}]}));
    let events = mapper.push_chunk(&json!({
        "id": "gen-1", "provider": "Wafer",
        "error": {"code": 502, "message": "upstream closed the stream", "metadata": {"error_type": "provider_timeout", "provider_code": "E_TIMEOUT"}},
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error", "native_finish_reason": null}]
    }));
    let _ = events;
    let done = mapper.finish();
    let message = match done.last() {
        Some(yi_types::event::AssistantMessageEvent::Error { error, .. }) => error.clone(),
        Some(yi_types::event::AssistantMessageEvent::Done { message, .. }) => message.clone(),
        other => return Err(format!("no terminal event: {other:?}").into()),
    };
    let AgentMessage::Assistant {
        stop_reason,
        error_message,
        ..
    } = message
    else {
        return Err("not an assistant message".into());
    };
    assert_eq!(stop_reason, StopReason::Error);
    let text = error_message.ok_or("an error chunk carries its message")?;
    assert_eq!(
        text,
        "upstream closed the stream (upstream Wafer, code 502, provider_timeout)"
    );
    Ok(())
}

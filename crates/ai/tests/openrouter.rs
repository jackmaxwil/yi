use serde_json::json;
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use yi_ai::catalog::Catalog;
use yi_ai::openai::{ChunkMapper, OpenAiOptions, build_params};
use yi_ai::request::ProxyConfig;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{
    AgentMessage, Content, RAW_STOP_IN_BAND_ERROR, StopReason, Usage, UserContent,
};
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

/// One HTTP/1.1 exchange on the loopback proxy: the request is read whole and `reply`
/// answers it as a sized body.
fn answer(listener: &TcpListener, content_type: &str, reply: &str) -> std::io::Result<()> {
    let (stream, _) = listener.accept()?;
    let mut reader = BufReader::new(stream);
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    write!(
        reader.into_inner(),
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    )
}

/// D175: an in-band error chunk ends the stream with no usage chunk and no transport fault,
/// so the turn is settled from the generation record like a dropped one. Settlement exists
/// only on an `openrouter.ai` base URL; this one cannot resolve, and the loopback proxy
/// answers both the stream and the record.
#[tokio::test]
async fn an_in_band_error_chunk_is_settled_from_the_record() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let stream_body: String = [
        json!({"id": "gen-1", "provider": "Wafer", "choices": [{"index": 0, "delta": {"reasoning": "hmm"}}]}),
        json!({"id": "gen-1", "provider": "Wafer",
               "error": {"code": 502, "message": "Internal Server Error", "metadata": {"error_type": "server_error"}},
               "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}]}),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .collect();
    let record = json!({"data": {"tokens_prompt": 5200, "tokens_completion": 900,
        "native_tokens_reasoning": 900, "total_cost": 0.0004, "provider_name": "Wafer"}})
    .to_string();
    // not joined: a turn left unsettled never asks for the record, so the second accept waits
    std::thread::spawn(move || -> std::io::Result<()> {
        answer(&listener, "text/event-stream", &stream_body)?;
        answer(&listener, "application/json", &record)
    });

    let mut model = target_model()?;
    model.base_url = "http://openrouter.ai.invalid/api/v1".to_owned();
    let options = OpenAiOptions {
        proxy: ProxyConfig::from_values(Some(&format!("http://127.0.0.1:{port}")), None, None)?,
        ..OpenAiOptions::default()
    };
    let mut events = yi_ai::openai::stream(&model, &history_context(), &options, "sk-test");
    let mut last = None;
    while let Some(event) = events.recv().await {
        last = Some(event);
    }
    let Some(AssistantMessageEvent::Error { error, .. }) = last else {
        return Err(format!("expected an error event: {last:?}").into());
    };
    let AgentMessage::Assistant {
        usage,
        error_message,
        raw_stop_reason,
        ..
    } = error
    else {
        return Err("not an assistant message".into());
    };
    assert_eq!(raw_stop_reason.as_deref(), Some(RAW_STOP_IN_BAND_ERROR));
    assert_eq!(
        error_message.as_deref(),
        Some("Internal Server Error (upstream Wafer, code 502, server_error)")
    );
    assert!(
        !usage.unknown,
        "the in-band error turn is settled from the record"
    );
    assert_eq!((usage.input, usage.output), (5200, 900));
    assert_eq!(usage.cost.total.as_f64(), Some(0.0004));
    Ok(())
}

//! Moonshot's chat wire: thinking as its own body field, the session as the cache key, the
//! reasoning echoed across tool turns, and Yi's own name.

use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::openai::{ChunkMapper, OpenAiOptions, build_params};
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{Effort, LlmContext, Model};

use crate::loopback_server::serve;

type TestResult = Result<(), Box<dyn Error>>;

const BASE: &str = "https://api.kimi.ai/coding/v1";

/// The entry as Kimi's `/models` lists it, through the same mapping a refresh writes.
pub(crate) fn kimi_model(entry: Value) -> Result<Model, Box<dyn Error>> {
    let id = entry["id"].as_str().ok_or("entry has no id")?.to_owned();
    let (written, _) = yi_ai::kimi::catalog(&json!({"data": [entry]}), BASE)?;
    let model = written["openai-completions"][id.as_str()].clone();
    Ok(serde_json::from_value(model)?)
}

fn k3_1() -> Result<Model, Box<dyn Error>> {
    kimi_model(json!({
        "id": "k3.1-pro", "display_name": "K3.1 Pro", "limit": {"context": 1048576},
        "reasoning": {"type": "only", "effort": {"valid": ["low", "high", "max"], "default": "high"}},
    }))
}

fn context(messages: Vec<AgentMessage>) -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "be terse".to_owned(),
        messages,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: None,
        tool_choice: None,
    }
}

fn hi() -> Vec<AgentMessage> {
    vec![AgentMessage::user_input(
        UserContent::Text("hi".to_owned()),
        0,
    )]
}

fn body(model: &Model, effort: Option<Effort>, messages: Vec<AgentMessage>) -> Value {
    let options = OpenAiOptions {
        reasoning_effort: effort,
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    build_params(model, &context(messages), &options).into_value()
}

fn thinking(body: &Value) -> Option<&Value> {
    body.get("thinking")
}

/// Kimi Code's trait maps an effort to `{type: enabled, effort}`, off to `{type: disabled}` and
/// a bare on to `{type: enabled}` (llm-kimi/trait.ts:89-103); an always-thinking model never
/// gets the disable it would refuse.
#[test]
fn a_moonshot_route_sends_thinking_not_reasoning_effort_and_the_session_cache_key() -> TestResult {
    let only = k3_1()?;
    let high = body(&only, Some(Effort::High), hi());
    assert_eq!(
        thinking(&high),
        Some(&json!({"type": "enabled", "effort": "high"}))
    );
    assert_eq!(high.get("reasoning_effort"), None);
    assert_eq!(high.get("reasoning"), None);
    assert_eq!(high["prompt_cache_key"], "session-1");
    assert_eq!(
        thinking(&body(&only, None, hi())),
        None,
        "`only` cannot be disabled"
    );

    let both = kimi_model(json!({
        "id": "kimi-toggle", "limit": {"context": 128000}, "reasoning": {"type": "both"},
    }))?;
    assert_eq!(
        thinking(&body(&both, None, hi())),
        Some(&json!({"type": "disabled"}))
    );
    let on = body(&both, Some(both.clamp_effort(Effort::High)), hi());
    assert_eq!(
        thinking(&on),
        Some(&json!({"type": "enabled"})),
        "no effort list, no effort"
    );
    assert_eq!(on.get("reasoning_effort"), None);
    Ok(())
}

fn assistant(content: Vec<Content>, stop: StopReason) -> AgentMessage {
    AgentMessage::Assistant {
        content,
        api: "openai-completions".to_owned(),
        provider: "kimi".to_owned(),
        model: "k3.1-pro".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::zero(),
        stop_reason: stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn call(id: &str) -> Content {
    let mut arguments = Map::new();
    arguments.insert("path".to_owned(), json!("README.md"));
    Content::ToolCall {
        id: id.to_owned(),
        name: "read".to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

fn result(id: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: "read".to_owned(),
        content: vec![Content::Text {
            text: "# Yi".to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

/// Moonshot validates thinking across tool turns: every assistant turn of a thinking model
/// carries `reasoning_content`, the streamed text where there was some and empty where not.
#[test]
fn a_tool_turn_replays_reasoning_content_for_a_direct_kimi_id() -> TestResult {
    let mut messages = hi();
    messages.push(assistant(
        vec![
            Content::Thinking {
                thinking: "read the readme first".to_owned(),
                thinking_signature: Some("reasoning_content".to_owned()),
                redacted: None,
            },
            call("read:0"),
        ],
        StopReason::ToolUse,
    ));
    messages.push(result("read:0"));
    messages.push(assistant(vec![call("read:1")], StopReason::ToolUse));
    messages.push(result("read:1"));
    let sent = body(&k3_1()?, Some(Effort::High), messages);
    let replayed: Vec<&Value> = sent["messages"]
        .as_array()
        .ok_or("no messages")?
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| &message["reasoning_content"])
        .collect();
    assert_eq!(replayed, [&json!("read the readme first"), &json!("")]);
    Ok(())
}

fn mapped(sse: &str, model: &Model) -> Result<AgentMessage, Box<dyn Error>> {
    let mut mapper = ChunkMapper::new(model);
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data != "[DONE]" {
            let _ = mapper.push_chunk(&serde_json::from_str(data)?);
        }
    }
    let ended = mapper.finish().pop().ok_or("no terminal event")?;
    match ended {
        yi_types::event::AssistantMessageEvent::Done { message, .. } => Ok(message),
        other => Err(format!("stream did not end done: {other:?}").into()),
    }
}

/// A turn the account streamed on 2026-10-24: reasoning, then a bash call, with the usage at the
/// top level. The ratios are the raw chunk's own: 13307 prompt tokens, 12800 of them cached.
#[test]
fn the_recorded_tool_turn_maps_to_thinking_a_call_and_its_usage() -> TestResult {
    let message = mapped(
        include_str!("fixtures/kimi-chat-2026-10-24-bash.sse"),
        &k3_1()?,
    )?;
    let AgentMessage::Assistant { usage, content, .. } = &message else {
        return Err("not an assistant message".into());
    };
    assert_eq!(
        (usage.input, usage.cache_read, usage.output),
        (507, 12800, 132)
    );
    assert!(
        matches!(content.first(), Some(Content::Thinking { thinking_signature: Some(field), .. }) if field == "reasoning_content")
    );
    let Some(Content::ToolCall {
        name, arguments, ..
    }) = content.last()
    else {
        return Err(format!("no tool call: {content:?}").into());
    };
    assert_eq!(
        (name.as_str(), arguments.get("command")),
        ("bash", Some(&json!("echo one")))
    );
    Ok(())
}

pub(crate) fn json_reply(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn user_agent(request: &str) -> Option<String> {
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("user-agent")
            .then(|| value.trim().to_owned())
    })
}

/// A provider sees Yi, not the HTTP library: the stream, a catalog read and a login token post
/// all say `yi/<version>`, and a profile's `stream_headers` user-agent still wins on the stream.
#[test]
fn every_agent_identifies_as_yi_and_a_profile_header_replaces_it() -> TestResult {
    let (port, _, handle) = serve(vec![
        json_reply("200 OK", "{}"),
        json_reply("200 OK", "{}"),
        json_reply("200 OK", "{}"),
        json_reply("200 OK", r#"{"access_token":"a2","expires_in":900}"#),
    ])?;
    let base = format!("http://127.0.0.1:{port}");
    let probe =
        yi_ai::breakpoints::Encoded::provider_prefix(json!({"model": "p"}), "messages", Vec::new());
    let quiet = |_| {};
    yi_ai::request::send_with_retry(&format!("{base}/chat"), &[], &probe, None, &quiet)?;
    yi_ai::request::send_with_retry(
        &format!("{base}/chat"),
        &[("User-Agent".to_owned(), "profile/1".to_owned())],
        &probe,
        None,
        &quiet,
    )?;
    yi_ai::request::get_json(&format!("{base}/models"), &[], None, 1024)?;
    let spec = yi_oauth::registry::OauthCode {
        id: "kimi".to_owned(),
        client_id: "client".to_owned(),
        client_secret: None,
        token: format!("{base}/token"),
        scopes: String::new(),
        json_token: false,
        stream_headers: Vec::new(),
        refresh_headers: Vec::new(),
        api_base: None,
        login: yi_oauth::registry::Login::Device {
            device_authorization: format!("{base}/device"),
        },
    };
    let stored = yi_oauth::store::Credential {
        kind: yi_oauth::store::Kind::Oauth,
        access: "a1".to_owned(),
        refresh: Some("r1".to_owned()),
        expires: None,
        account: None,
        org: None,
        extra: Map::new(),
    };
    yi_oauth::flow::refresh(&spec, &stored, None)?;
    let agents: Vec<Option<String>> = handle
        .join()
        .map_err(|_| "server thread")?
        .iter()
        .map(|request| user_agent(request))
        .collect();
    let yi = Some(yi_types::USER_AGENT.to_owned());
    assert_eq!(
        agents,
        [yi.clone(), Some("profile/1".to_owned()), yi.clone(), yi]
    );
    assert!(yi_types::USER_AGENT.starts_with("yi/"));
    Ok(())
}

/// The Moonshot branch is a route fact: OpenRouter's own Kimi and a direct OpenAI model build
/// the bytes they built before it existed (captured from the unfixed build_params).
#[test]
fn a_non_moonshot_route_is_byte_identical() -> TestResult {
    let pinned: Value =
        serde_json::from_str(include_str!("fixtures/non-moonshot-params-2026-10-24.json"))?;
    let catalog = yi_ai::catalog::Catalog::bundled();
    for (provider, id) in [
        ("openrouter", "moonshotai/kimi-k3"),
        ("openai", "gpt-5.6-luna"),
    ] {
        let model = catalog.get(provider, id).ok_or(id)?;
        let mut messages = hi();
        messages.push(assistant(
            vec![
                Content::Thinking {
                    thinking: "look".to_owned(),
                    thinking_signature: Some("reasoning_content".to_owned()),
                    redacted: None,
                },
                call("read:0"),
            ],
            StopReason::ToolUse,
        ));
        messages.push(result("read:0"));
        let sent = body(model, Some(Effort::High), messages).to_string();
        assert_eq!(sent, pinned[id].as_str().ok_or(id)?, "{id}");
    }
    Ok(())
}

/// An entry Kimi lists with `protocol: anthropic` posts to the messages path Moonshot serves,
/// `?beta=true` included, off the same `api_base` the chat models use.
#[test]
fn an_anthropic_protocol_kimi_model_posts_to_beta_messages() -> TestResult {
    let (port, _, handle) = serve(vec![json_reply("200 OK", "{}")])?;
    let base = format!("http://127.0.0.1:{port}/coding/v1");
    let list =
        json!({"data": [{"id": "k3-a", "protocol": "anthropic", "limit": {"context": 262144}}]});
    let (written, _) = yi_ai::kimi::catalog(&list, &base)?;
    let model: Model = serde_json::from_value(written["anthropic-messages"]["k3-a"].clone())?;
    let options = yi_ai::anthropic::AnthropicOptions::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut events = yi_ai::anthropic::stream(&model, &context(hi()), &options, "key");
        while events.recv().await.is_some() {}
    });
    let seen = handle.join().map_err(|_| "server thread")?;
    let line = seen.first().and_then(|request| request.lines().next());
    assert_eq!(line, Some("POST /coding/v1/messages?beta=true HTTP/1.1"));
    Ok(())
}

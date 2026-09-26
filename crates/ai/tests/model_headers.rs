use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::json;
use yi_types::model::{LlmContext, Model};

type Res = Result<(), Box<dyn std::error::Error>>;

fn serve_once() -> Result<(u16, std::thread::JoinHandle<String>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return String::new();
        };
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]).into_owned();
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n");
        request
    });
    Ok((port, handle))
}

fn model(
    api: &str,
    provider: &str,
    port: u16,
    headers: serde_json::Value,
) -> Result<Model, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "id": "probe",
        "name": "probe",
        "api": api,
        "provider": provider,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000,
        "maxTokens": 16,
        "headers": headers
    }))?)
}

fn context() -> LlmContext {
    LlmContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: None,
        tool_choice: None,
    }
}

fn drain(
    open: impl FnOnce() -> tokio::sync::mpsc::Receiver<yi_types::event::AssistantMessageEvent>,
) -> Res {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut receiver = open();
        while receiver.recv().await.is_some() {}
    });
    Ok(())
}

/// A gateway that wants its own auth header had no way to ask for one: `headers` was
/// in the model schema with no consumer, so a catalog overlay carrying it changed nothing.
#[test]
fn a_catalog_overlay_header_reaches_the_messages_wire() -> Res {
    let (port, handle) = serve_once()?;
    let model = model(
        "anthropic-messages",
        "anthropic",
        port,
        json!({"x-gateway-tenant": "acme", "anthropic-beta": "probe-beta"}),
    )?;
    let options = yi_ai::anthropic::AnthropicOptions::default();
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, "key-1"))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("x-gateway-tenant: acme"), "{request}");
    assert!(lower.contains("anthropic-beta: probe-beta"), "{request}");
    assert!(lower.contains("x-api-key: key-1"), "{request}");
    Ok(())
}

/// Without case-insensitive replacement an overlay `Authorization` would be sent beside
/// the adapter's own, and ureq sends both: the gateway then reads whichever it likes.
#[test]
fn an_overlay_name_replaces_the_adapter_header_once() -> Res {
    let (port, handle) = serve_once()?;
    let model = model(
        "anthropic-messages",
        "anthropic",
        port,
        json!({"X-Api-Key": "overlay-key"}),
    )?;
    let options = yi_ai::anthropic::AnthropicOptions::default();
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, "adapter-key"))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert_eq!(lower.matches("x-api-key:").count(), 1, "{request}");
    assert!(lower.contains("overlay-key"), "{request}");
    assert!(!lower.contains("adapter-key"), "{request}");
    Ok(())
}

/// The OpenAI-style adapters build their bearer in a different function; wiring only the
/// Anthropic path would leave every OpenRouter and `openai-codex` gateway header silently dropped.
#[test]
fn the_openai_paths_carry_the_overlay_too() -> Res {
    for (api, path) in [
        ("openai-completions", "chat/completions"),
        ("openai-responses", "responses"),
    ] {
        let (port, handle) = serve_once()?;
        let model = model(api, "openai", port, json!({"x-gateway-tenant": "acme"}))?;
        let options = yi_ai::openai::OpenAiOptions::default();
        drain(|| {
            if api == "openai-completions" {
                yi_ai::openai::stream(&model, &context(), &options, "key-1")
            } else {
                yi_ai::openai_responses::stream(&model, &context(), &options, "key-1")
            }
        })?;
        let request = handle.join().map_err(|_| "server thread")?;
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains(path), "{api} hit the wrong route: {request}");
        assert!(lower.contains("x-gateway-tenant: acme"), "{api}: {request}");
        assert!(
            lower.contains("authorization: bearer key-1"),
            "{api}: {request}"
        );
    }
    Ok(())
}

/// A model with no `headers` must send exactly what it sent before this seam existed.
#[test]
fn a_model_without_headers_is_unchanged() -> Res {
    let (port, handle) = serve_once()?;
    let model = model(
        "anthropic-messages",
        "anthropic",
        port,
        serde_json::Value::Null,
    )?;
    let options = yi_ai::anthropic::AnthropicOptions::default();
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, "key-1"))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("x-api-key: key-1"), "{request}");
    assert!(lower.contains("anthropic-version:"), "{request}");
    Ok(())
}

/// An overlay that blanks a header must actually remove it: ureq sets an empty value
/// as `name: `, and a provider reading `x-api-key: ` sees a malformed key, not none.
#[test]
fn an_empty_overlay_value_removes_the_header() -> Res {
    let (port, handle) = serve_once()?;
    let model = model(
        "anthropic-messages",
        "anthropic",
        port,
        json!({"x-api-key": ""}),
    )?;
    let options = yi_ai::anthropic::AnthropicOptions::default();
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, "adapter-key"))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(!lower.contains("x-api-key"), "{request}");
    Ok(())
}

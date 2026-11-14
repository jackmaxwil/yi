use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};

use serde_json::json;
use yi_ai::auth::{self, AuthKind};
use yi_oauth::store::{Credential, Kind, Store};
use yi_types::model::{LlmContext, Model};

type Res = crate::harness::Res;
use crate::harness::drain;

/// These tests set HOME and the provider env vars, which are process-wide.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Removes the temp HOME on drop: a failed assert must not leave one behind.
struct Home(std::path::PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn home(tag: &str) -> Result<Home, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("yi-oauth-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".yi"))?;
    unsafe { std::env::set_var("HOME", &dir) };
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    Ok(Home(dir))
}

fn write_profile(home: &Home, provider: &str, profile: &serde_json::Value) -> Res {
    let dir = home.0.join(".yi").join("oauth");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{provider}.json")), profile.to_string())?;
    Ok(())
}

fn save_oauth_credential(
    access: &str,
    refresh: Option<&str>,
    expires: Option<std::time::SystemTime>,
) -> Res {
    Store::user().save(
        "anthropic",
        &Credential {
            kind: Kind::Oauth,
            access: access.to_owned(),
            refresh: refresh.map(str::to_owned),
            expires,
            account: None,
            org: None,
            extra: Default::default(),
        },
    )?;
    Ok(())
}

const EMPTY_REPLY: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n";

fn serve_once(
    reply: String,
) -> Result<(u16, std::thread::JoinHandle<String>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return String::new();
        };
        let mut reader = BufReader::new(stream);
        let request = crate::loopback::read_request(&mut reader);
        let _ = reader.get_mut().write_all(reply.as_bytes());
        request
    });
    Ok((port, handle))
}

fn model(port: u16) -> Result<Model, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "id": "probe",
        "name": "probe",
        "api": "anthropic-messages",
        "provider": "anthropic",
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1000,
        "maxTokens": 16
    }))?)
}

fn context() -> LlmContext {
    crate::harness::empty_context()
}

/// An env key must outrank a stored credential, or a user who exports a key to
/// override a stale login silently keeps streaming on the stale one.
#[test]
fn env_beats_the_store_and_logout_deletes() -> Res {
    let _serial = serial();
    let _home = home("env")?;
    save_oauth_credential("stored-token", None, None)?;
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "env-key") };
    let resolved = auth::resolve("anthropic").ok_or("expected the env credential")?;
    assert_eq!(resolved.kind, AuthKind::ApiKey);
    assert_eq!(resolved.secret.expose(), "env-key");

    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    let stored = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    assert_eq!(stored.kind, AuthKind::Oauth);
    assert_eq!(stored.secret.expose(), "stored-token");

    yi_oauth::flow::logout("anthropic")?;
    assert!(auth::resolve("anthropic").is_none());
    assert!(
        auth::missing_message("anthropic").contains("yi login anthropic"),
        "{}",
        auth::missing_message("anthropic")
    );
    Ok(())
}

/// The whole point of D191: the identity on the wire comes from the user's own
/// profile. Yi compiles none of it, so a profile that carries nothing sends nothing.
#[test]
fn a_profile_puts_its_stream_headers_on_the_wire() -> Res {
    let _serial = serial();
    let home = home("stream")?;
    write_profile(
        &home,
        "anthropic",
        &json!({
            "kind": "oauth-code",
            "client_id": "test-client",
            "authorize": "https://example.invalid/authorize",
            "token": "https://example.invalid/token",
            "callback_port": 8765,
            "stream_headers": {
                "user-agent": "probe-agent/1.0",
                "x-probe-app": "cli"
            }
        }),
    )?;
    save_oauth_credential("oauth-tok", None, None)?;

    let resolved = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    assert_eq!(resolved.kind, AuthKind::Oauth);

    let (port, handle) = serve_once(EMPTY_REPLY.to_owned())?;
    let model = model(port)?;
    let options = yi_ai::anthropic::AnthropicOptions {
        oauth: true,
        extra_headers: resolved.headers.clone(),
        ..yi_ai::anthropic::AnthropicOptions::default()
    };
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, resolved.secret.expose()))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();

    assert!(
        lower.contains("authorization: bearer oauth-tok"),
        "{request}"
    );
    assert!(!lower.contains("x-api-key"), "{request}");
    assert!(lower.contains("user-agent: probe-agent/1.0"), "{request}");
    assert!(lower.contains("x-probe-app: cli"), "{request}");
    Ok(())
}

/// A profile with no `stream_headers` must send a plain OAuth request: the binary
/// has no identity of its own to fall back to, and inventing one is the whole hazard.
#[test]
fn a_profile_without_headers_sends_only_bearer() -> Res {
    let _serial = serial();
    let home = home("bare")?;
    write_profile(
        &home,
        "anthropic",
        &json!({
            "kind": "oauth-code",
            "client_id": "test-client",
            "authorize": "https://example.invalid/authorize",
            "token": "https://example.invalid/token",
            "callback_port": 8765
        }),
    )?;
    save_oauth_credential("bare-tok", None, None)?;
    let resolved = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    assert!(resolved.headers.is_empty(), "{:?}", resolved.headers);

    let (port, handle) = serve_once(EMPTY_REPLY.to_owned())?;
    let model = model(port)?;
    let options = yi_ai::anthropic::AnthropicOptions {
        oauth: true,
        extra_headers: resolved.headers.clone(),
        ..yi_ai::anthropic::AnthropicOptions::default()
    };
    drain(|| yi_ai::anthropic::stream(&model, &context(), &options, resolved.secret.expose()))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer bare-tok"),
        "{request}"
    );
    assert_eq!(lower.matches("authorization:").count(), 1, "{request}");
    assert!(!lower.contains("x-api-key"), "{request}");
    Ok(())
}

/// The built-in Claude login must reach the wire as Claude Code: Bearer, the oauth beta
/// header, and the identity line first in `system`, ahead of the caller's own prompt.
#[test]
fn the_claude_login_sends_the_beta_header_and_the_identity_line() -> Res {
    let _serial = serial();
    let _home = home("claude")?;
    save_oauth_credential("claude-tok", None, None)?;
    let resolved = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    let (port, handle) = serve_once(EMPTY_REPLY.to_owned())?;
    let model = model(port)?;
    let mut context = context();
    context.system_prompt = "Be brief.".to_owned();
    let options = yi_ai::anthropic::AnthropicOptions {
        oauth: true,
        claude_code: true,
        extra_headers: resolved.headers.clone(),
        ..yi_ai::anthropic::AnthropicOptions::default()
    };
    drain(|| yi_ai::anthropic::stream(&model, &context, &options, resolved.secret.expose()))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer claude-tok"),
        "{request}"
    );
    assert!(
        lower.contains("anthropic-beta: oauth-2025-04-20"),
        "{request}"
    );
    let identity = request
        .find("You are Claude Code, Anthropic's official CLI for Claude.")
        .ok_or("identity line missing")?;
    let prompt = request.find("Be brief.").ok_or("system prompt missing")?;
    assert!(identity < prompt, "{request}");
    Ok(())
}

#[test]
fn an_oauth_credential_without_the_claude_login_sends_no_identity_line() -> Res {
    let _serial = serial();
    let _home = home("kimi-wire")?;
    let (port, handle) = serve_once(EMPTY_REPLY.to_owned())?;
    let model = model(port)?;
    let mut context = context();
    context.system_prompt = "Be brief.".to_owned();
    let options = yi_ai::anthropic::AnthropicOptions {
        oauth: true,
        ..yi_ai::anthropic::AnthropicOptions::default()
    };
    drain(|| yi_ai::anthropic::stream(&model, &context, &options, "kimi-tok"))?;
    let request = handle.join().map_err(|_| "server thread")?;
    let lower = request.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer kimi-tok"),
        "{request}"
    );
    assert!(!request.contains("You are Claude Code"), "{request}");
    Ok(())
}

/// An expired Claude login is refreshed before its token is handed out, so the request
/// carries the refreshed Bearer token and not the stale one on disk.
#[test]
fn an_expired_claude_login_is_refreshed_before_it_is_sent() -> Res {
    let _serial = serial();
    let home = home("refresh")?;
    let body = json!({
        "access_token": "fresh-tok",
        "refresh_token": "fresh-refresh",
        "expires_in": 3600
    })
    .to_string();
    let reply = crate::loopback::reply("200 OK", "application/json", &body);
    let (port, server) = serve_once(reply)?;
    write_profile(
        &home,
        "anthropic",
        &json!({
            "kind": "oauth-code",
            "client_id": "test-client",
            "authorize": "https://example.invalid/authorize",
            "token": format!("http://127.0.0.1:{port}/token"),
            "callback_port": 8765
        }),
    )?;
    save_oauth_credential(
        "stale-tok",
        Some("old-refresh"),
        Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
    )?;

    let resolved = auth::resolve("anthropic").ok_or("expected the refreshed credential")?;
    assert_eq!(resolved.secret.expose(), "fresh-tok");
    assert_eq!(resolved.kind, AuthKind::Oauth);
    let request = server.join().map_err(|_| "server thread")?;
    assert!(request.contains("old-refresh"), "{request}");
    Ok(())
}

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};

use serde_json::json;
use yi_ai::auth::{self, AuthKind};
use yi_oauth::store::{Credential, Kind, Store};
use yi_types::model::{LlmContext, Model};

type Res = Result<(), Box<dyn std::error::Error>>;

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
    LlmContext {
        system_prompt: String::new(),
        messages: vec![],
        transient: Vec::new(),
        schema: None,
        reuse: yi_types::model::Reuse::Loop,
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

/// An env key must outrank a stored credential, or a user who exports a key to
/// override a stale login silently keeps streaming on the stale one.
#[test]
fn env_beats_the_store_and_logout_deletes() -> Res {
    let _serial = serial();
    let _home = home("env")?;
    let store = Store::user();
    store.save(
        "anthropic",
        &Credential {
            kind: Kind::Oauth,
            access: "stored-token".to_owned(),
            refresh: None,
            expires: None,
            account: None,
            org: None,
            extra: Default::default(),
        },
    )?;
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
    Store::user().save(
        "anthropic",
        &Credential {
            kind: Kind::Oauth,
            access: "oauth-tok".to_owned(),
            refresh: None,
            expires: None,
            account: None,
            org: None,
            extra: Default::default(),
        },
    )?;

    let resolved = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    assert_eq!(resolved.kind, AuthKind::Oauth);

    let (port, handle) = serve_once()?;
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
    Store::user().save(
        "anthropic",
        &Credential {
            kind: Kind::Oauth,
            access: "bare-tok".to_owned(),
            refresh: None,
            expires: None,
            account: None,
            org: None,
            extra: Default::default(),
        },
    )?;
    let resolved = auth::resolve("anthropic").ok_or("expected the stored credential")?;
    assert!(resolved.headers.is_empty(), "{:?}", resolved.headers);

    let (port, handle) = serve_once()?;
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

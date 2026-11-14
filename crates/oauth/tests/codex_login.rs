//! The built-in `openai-codex` profile against a fake token server: the authorize URL it opens,
//! the code exchange, the ChatGPT account id from the id_token, and a refresh that keeps it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use serde_json::{Value, json};
use yi_oauth::codex;
use yi_oauth::flow;
use yi_oauth::registry::{self, CodeLogin, Kind as ProfileKind, Login, OauthCode};
use yi_oauth::store::{self, Credential, Kind, Store};

mod common;
use common::Res;

const ID_TOKEN: &str =
    "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC03In19.sig";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yi-codex-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn opener_log() -> PathBuf {
    std::env::temp_dir().join(format!("yi-codex-opened-{}", std::process::id()))
}

/// One process-wide setup: HOME holds a profile dir, and PATH starts with a stand-in browser
/// opener that writes the URL it was given to a file instead of launching anything.
fn environment() -> Res {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| {
        let bin = scratch("bin");
        let log = opener_log();
        let _ = std::fs::create_dir_all(&bin);
        let name = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let script = bin.join(name);
        let _ = std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", log.display()),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
        }
        let mut path = std::ffi::OsString::from(&bin);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let home = scratch("home");
        let _ = std::fs::create_dir_all(&home);
        // SAFETY: runs once, before any test body reads the environment.
        unsafe {
            std::env::set_var("PATH", path);
            std::env::set_var("HOME", &home);
        }
    });
    Ok(())
}

fn codex_spec() -> Result<OauthCode, Box<dyn std::error::Error>> {
    match codex::profile() {
        ProfileKind::OauthCode(spec) => Ok(*spec),
        ProfileKind::ApiKey | ProfileKind::Cursor => {
            Err("openai-codex is not an OAuth code profile".into())
        }
    }
}

fn query(url: &str, key: &str) -> Option<String> {
    let (_, rest) = url.split_once('?')?;
    rest.split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(str::to_owned)
}

fn expired(org: Option<&str>) -> Credential {
    Credential {
        kind: Kind::Oauth,
        access: "old-access".to_owned(),
        refresh: Some("old-refresh".to_owned()),
        expires: Some(store::now() - Duration::from_secs(60)),
        account: None,
        org: org.map(str::to_owned),
        extra: Default::default(),
    }
}

#[test]
fn the_login_opens_the_codex_authorize_url_and_exchanges_the_code() -> Res {
    environment()?;
    let body = json!({
        "access_token": "chatgpt-access",
        "refresh_token": "chatgpt-refresh",
        "expires_in": 3600,
        "id_token": ID_TOKEN
    });
    let (token_port, seen) = common::serve(vec![(200, body)])?;
    let callback_port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();

    let mut spec = codex_spec()?;
    spec.token = format!("http://127.0.0.1:{token_port}/oauth/token");
    let Login::Code(CodeLogin { callback_path, .. }) = &spec.login else {
        return Err("openai-codex logs in by code".into());
    };
    assert_eq!(callback_path, "/auth/callback");
    if let Login::Code(login) = &mut spec.login {
        login.callback_port = callback_port;
    }

    let _ = std::fs::remove_file(opener_log());
    let login = std::thread::spawn(move || {
        flow::login_oauth(&spec, &flow::LoginOptions { no_browser: false }, None)
    });
    let mut url = String::new();
    for _ in 0..100 {
        url = std::fs::read_to_string(opener_log()).unwrap_or_default();
        if !url.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        url.starts_with("https://auth.openai.com/oauth/authorize?"),
        "{url}"
    );
    for (key, want) in [
        ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
        ("code_challenge_method", "S256"),
    ] {
        assert_eq!(query(&url, key).as_deref(), Some(want), "{key} in {url}");
    }
    assert_eq!(
        query(&url, "scope").as_deref(),
        Some("openid%20profile%20email%20offline_access")
    );
    let redirect = query(&url, "redirect_uri").ok_or("redirect_uri")?;
    assert_eq!(
        redirect,
        format!("http%3A%2F%2Flocalhost%3A{callback_port}%2Fauth%2Fcallback")
    );
    let state = query(&url, "state").ok_or("state")?;

    let mut conn = std::net::TcpStream::connect(("127.0.0.1", callback_port))?;
    conn.write_all(
        format!(
            "GET /auth/callback?code=the-code&state={state} HTTP/1.1\r\nhost: localhost\r\n\r\n"
        )
        .as_bytes(),
    )?;
    let mut buf = [0u8; 1024];
    let _ = conn.read(&mut buf);

    let credential = login.join().map_err(|_| "login thread panicked")??;
    assert_eq!(credential.access, "chatgpt-access");
    assert_eq!(credential.refresh.as_deref(), Some("chatgpt-refresh"));
    assert_eq!(credential.org.as_deref(), Some("acct-7"));

    let rows = seen.lock().map_err(|_| "poisoned")?.clone();
    let [request] = rows.as_slice() else {
        return Err(format!("expected one token request, saw {rows:?}").into());
    };
    assert!(request.starts_with("POST /oauth/token"), "{request}");
    for field in [
        "grant_type=authorization_code",
        "code=the-code",
        "client_id=app_EMoamEEZ73f0CkXaXp7hrann",
        "code_verifier=",
        &format!("redirect_uri=http%3A%2F%2Flocalhost%3A{callback_port}%2Fauth%2Fcallback"),
    ] {
        assert!(request.contains(field), "{field} missing in {request}");
    }
    Ok(())
}

#[test]
fn a_refresh_keeps_the_account_id_the_response_does_not_repeat() -> Res {
    environment()?;
    let body = json!({"access_token": "new-access", "expires_in": 3600});
    let (port, seen) = common::serve(vec![(200, body)])?;
    let mut spec = codex_spec()?;
    spec.token = format!("http://127.0.0.1:{port}/oauth/token");
    let store = Store::open(scratch("store"));
    store.save(&spec.id, &expired(Some("acct-7")))?;

    let live = flow::live_oauth(&spec, &store, None)?;
    assert_eq!(live.access, "new-access");
    assert_eq!(live.refresh.as_deref(), Some("old-refresh"));
    assert_eq!(live.org.as_deref(), Some("acct-7"));
    let on_disk = store.load(&spec.id).ok_or("saved")?;
    assert_eq!(on_disk.org.as_deref(), Some("acct-7"));
    let rows = seen.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        rows.iter()
            .any(|row| row.contains("grant_type=refresh_token")
                && row.contains("refresh_token=old-refresh")),
        "{rows:?}"
    );
    Ok(())
}

#[test]
fn the_codex_login_is_built_in_and_a_user_profile_file_wins() -> Res {
    environment()?;
    let ProfileKind::OauthCode(built_in) = registry::lookup("openai-codex")?.ok_or("built in")?
    else {
        return Err("openai-codex is not an OAuth code profile".into());
    };
    assert_eq!(built_in.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    let Login::Code(login) = &built_in.login else {
        return Err("code login".into());
    };
    assert_eq!(login.callback_port, 1455);
    assert!(!login.port_fallback, "the registered redirect is fixed");
    assert!(registry::ids().iter().any(|id| id == "openai-codex"));

    let path = registry::profile_path("openai-codex");
    std::fs::create_dir_all(path.parent().ok_or("profile dir")?)?;
    let custom: Value = json!({
        "kind": "oauth-code",
        "client_id": "mine",
        "authorize": "https://example.invalid/a",
        "token": "https://example.invalid/t",
        "callback_port": 4242
    });
    std::fs::write(&path, custom.to_string())?;
    let looked_up = registry::lookup("openai-codex");
    std::fs::remove_file(&path)?;
    let ProfileKind::OauthCode(user) = looked_up?.ok_or("profile")? else {
        return Err("openai-codex is not an OAuth code profile".into());
    };
    assert_eq!(user.client_id, "mine");
    Ok(())
}

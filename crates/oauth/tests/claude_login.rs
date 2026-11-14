//! The Claude subscription login: a pasted `code#state` exchanged as a JSON token body,
//! and the built-in profile that a file at the same id replaces.

use serde_json::{Value, json};
use yi_oauth::flow::{self, LoginOptions};
use yi_oauth::registry::{self, Kind, Login};

mod common;
use common::Res;

fn local_spec(port: u16) -> Result<registry::OauthCode, Box<dyn std::error::Error>> {
    let Kind::OauthCode(mut spec) = yi_oauth::claude::profile() else {
        return Err("claude profile is not an oauth-code profile".into());
    };
    spec.token = format!("http://127.0.0.1:{port}/v1/oauth/token");
    Ok(*spec)
}

fn paste(spec: &registry::OauthCode, input: &str) -> yi_oauth::Result<yi_oauth::store::Credential> {
    let Login::Paste(login) = &spec.login else {
        return Err("not a paste login".into());
    };
    flow::login_paste(
        spec,
        login,
        &LoginOptions { no_browser: true },
        None,
        &mut std::io::Cursor::new(input.to_owned()),
    )
}

#[test]
fn a_pasted_code_is_exchanged_as_a_json_body_with_state_and_verifier() -> Res {
    let body = json!({
        "access_token": "sk-ant-oat01-access",
        "refresh_token": "sk-ant-ort01-refresh",
        "expires_in": 28800,
        "account": {"uuid": "acct-1"},
        "organization": {"uuid": "org-1"}
    });
    let (port, seen) = common::serve(vec![(200, body)])?;
    let spec = local_spec(port)?;
    let credential = paste(&spec, "the-code\n")?;
    assert_eq!(credential.access, "sk-ant-oat01-access");
    assert_eq!(credential.refresh.as_deref(), Some("sk-ant-ort01-refresh"));
    assert_eq!(credential.org.as_deref(), Some("org-1"));
    let log = seen.lock().map_err(|_| "poisoned")?;
    let sent: Value = serde_json::from_str(
        log.first()
            .ok_or("no request")?
            .split_once('\n')
            .ok_or("no body")?
            .1,
    )?;
    assert_eq!(sent["grant_type"], "authorization_code");
    assert_eq!(sent["code"], "the-code");
    assert_eq!(sent["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(
        sent["redirect_uri"],
        "https://console.anthropic.com/oauth/code/callback"
    );
    assert!(
        sent["state"]
            .as_str()
            .is_some_and(|state| !state.is_empty())
    );
    assert!(
        sent["code_verifier"]
            .as_str()
            .is_some_and(|v| v.len() >= 43)
    );
    Ok(())
}

#[test]
fn a_pasted_state_that_is_not_ours_is_refused_before_any_request() -> Res {
    let (port, seen) = common::serve(vec![(200, json!({"access_token": "x"}))])?;
    let spec = local_spec(port)?;
    let error = paste(&spec, "the-code#someone-elses-state\n")
        .err()
        .ok_or("a foreign state must be refused")?;
    assert!(error.to_string().contains("state mismatch"), "{error}");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(seen.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}

#[test]
fn an_empty_paste_is_refused() -> Res {
    let spec = local_spec(1)?;
    let error = paste(&spec, "\n").err().ok_or("empty paste must fail")?;
    assert!(error.to_string().contains("no code"), "{error}");
    Ok(())
}

#[test]
fn claude_is_built_in_until_a_profile_file_exists_and_anthropic_stays_a_key() -> Res {
    let home = std::env::temp_dir().join(format!("yi-claude-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home)?;
    unsafe { std::env::set_var("HOME", &home) };

    let Kind::OauthCode(spec) = registry::claude()? else {
        return Err("built-in claude is not an oauth-code profile".into());
    };
    assert_eq!(spec.id, "anthropic");
    assert!(spec.json_token);
    assert_eq!(
        spec.scopes,
        "org:create_api_key user:profile user:inference"
    );
    assert_eq!(spec.token, "https://console.anthropic.com/v1/oauth/token");
    assert!(
        spec.stream_headers
            .contains(&("anthropic-beta".to_owned(), "oauth-2025-04-20".to_owned()))
    );
    let Login::Paste(login) = &spec.login else {
        return Err("claude must be a paste login".into());
    };
    assert_eq!(login.authorize, "https://claude.ai/oauth/authorize");
    assert!(matches!(registry::lookup("anthropic")?, Some(Kind::ApiKey)));
    assert!(registry::ids().iter().any(|id| id == "claude"));

    let dir = home.join(".yi").join("oauth");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("anthropic.json"),
        json!({
            "kind": "oauth-code", "client_id": "mine", "token": "https://example.invalid/t",
            "authorize": "https://example.invalid/a", "callback_port": 1455
        })
        .to_string(),
    )?;
    let Kind::OauthCode(spec) = registry::claude()? else {
        return Err("profile file must win".into());
    };
    assert_eq!(spec.client_id, "mine");
    assert!(matches!(
        registry::refresh_kind("anthropic", false)?,
        Some(Kind::OauthCode(_))
    ));
    Ok(())
}

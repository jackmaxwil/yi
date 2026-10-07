use std::time::Duration;

use serde_json::json;
use yi_oauth::cursor::{Pace, login_polled, login_url, tokens_from_exchange};
use yi_oauth::pkce::base64url_nopad;
use yi_oauth::registry::{self, Kind};

fn jwt_with_exp(exp: u64) -> String {
    let header = base64url_nopad(br#"{"alg":"none"}"#);
    let payload = base64url_nopad(format!(r#"{{"exp":{exp}}}"#).as_bytes());
    format!("{header}.{payload}.sig")
}

#[test]
fn poll_waits_through_404_then_stores_the_token() {
    let token = jwt_with_exp(1_893_456_000);
    let mut calls = 0u32;
    let credential = login_polled(
        true,
        Pace {
            attempts: 4,
            delay: Duration::ZERO,
        },
        |url| {
            calls += 1;
            assert!(url.contains("verifier="), "{url}");
            if calls == 1 {
                return Ok((404, Vec::new()));
            }
            Ok((
                200,
                json!({"accessToken": token, "refreshToken": "refresh-1"})
                    .to_string()
                    .into_bytes(),
            ))
        },
    )
    .expect("poll");
    assert_eq!(calls, 2);
    assert_eq!(credential.access, token);
    assert_eq!(credential.refresh.as_deref(), Some("refresh-1"));
    assert!(credential.expires.is_some());
}

#[test]
fn refresh_keeps_the_old_refresh_when_the_body_omits_one() {
    let token = jwt_with_exp(1_893_456_000);
    let body = json!({"accessToken": token}).to_string();
    let credential = tokens_from_exchange(body.as_bytes(), "refresh-old").expect("refresh");
    assert_eq!(credential.access, token);
    assert_eq!(credential.refresh.as_deref(), Some("refresh-old"));
}

#[test]
fn cursor_is_a_built_in_until_a_profile_file_exists() {
    let home = std::env::temp_dir().join(format!("yi-cursor-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("home");
    unsafe { std::env::set_var("HOME", &home) };
    assert!(matches!(
        registry::lookup("cursor").expect("lookup"),
        Some(Kind::Cursor)
    ));
    assert!(registry::ids().iter().any(|id| id == "cursor"));
    let dir = home.join(".yi").join("oauth");
    std::fs::create_dir_all(&dir).expect("oauth dir");
    std::fs::write(dir.join("cursor.json"), r#"{"kind":"api-key"}"#).expect("profile");
    assert!(matches!(
        registry::lookup("cursor").expect("profile wins"),
        Some(Kind::ApiKey)
    ));
    let url = login_url("challenge", "uuid");
    assert!(url.starts_with("https://cursor.com/loginDeepControl?"));
}

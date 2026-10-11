use std::error::Error;
use std::time::{Duration, SystemTime};

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
    assert_eq!(
        credential.expires,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_893_456_000 - 300))
    );
}

fn answer(token: &str) -> (u16, Vec<u8>) {
    (200, json!({"accessToken": token}).to_string().into_bytes())
}

#[test]
fn a_transient_poll_failure_does_not_abort_an_approved_login() -> Result<(), Box<dyn Error>> {
    let token = jwt_with_exp(1_893_456_000);
    let mut replies = vec![
        Err("cursor poll: Connection Failed".to_owned()),
        Ok((503, Vec::new())),
        Ok((429, Vec::new())),
        Ok(answer(&token)),
    ]
    .into_iter();
    let credential = login_polled(
        true,
        Pace {
            attempts: 4,
            delay: Duration::ZERO,
        },
        |_| replies.next().ok_or("polled past the script".to_owned())?,
    )?;
    assert_eq!(credential.access, token);
    Ok(())
}

#[test]
fn a_poll_that_never_approves_times_out_and_a_hard_status_stops_it() {
    let pace = || Pace {
        attempts: 2,
        delay: Duration::ZERO,
    };
    let mut calls = 0;
    let timeout = login_polled(true, pace(), |_| {
        calls += 1;
        Ok((404, Vec::new()))
    });
    assert_eq!(calls, 2);
    assert_eq!(
        timeout.err().map(|error| error.to_string()),
        Some("cursor authentication polling timeout".to_owned())
    );
    let refused = login_polled(true, pace(), |_| Ok((401, Vec::new())));
    assert_eq!(
        refused.err().map(|error| error.to_string()),
        Some("cursor poll failed: status code 401".to_owned())
    );
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

#[test]
fn a_network_that_stays_down_ends_the_login_at_six_failures() -> Result<(), Box<dyn Error>> {
    let token = jwt_with_exp(1_893_456_000);
    let down = || Err::<(u16, Vec<u8>), String>("cursor poll: Dns Failed".to_owned());
    let pace = || Pace {
        attempts: 150,
        delay: Duration::ZERO,
    };
    let mut five = std::iter::repeat_with(down)
        .take(5)
        .chain([Ok(answer(&token))]);
    let credential = login_polled(true, pace(), |_| {
        five.next().ok_or("polled past the script".to_owned())?
    })?;
    assert_eq!(credential.access, token);
    let mut polls = 0_u32;
    let refused = login_polled(true, pace(), |_| {
        polls += 1;
        down()
    });
    assert_eq!(polls, 6);
    let error = refused.err().ok_or("a dead network still logged in")?;
    assert_eq!(
        error.to_string(),
        "cursor poll: Dns Failed, 6 times in a row"
    );
    Ok(())
}

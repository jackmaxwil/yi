//! The RFC 8628 device login against a device and token endpoint on 127.0.0.1. Response bodies
//! are RFC 8628 §3.2 and §3.5 and RFC 6749 §5.1, shaped as kimi-code's oauth package reads them.

use std::time::Duration;

use serde_json::{Value, json};
use yi_oauth::device;
use yi_oauth::flow;
use yi_oauth::registry::{self, Kind, Login, OauthCode};
use yi_oauth::store::{self, Credential, Store};

mod common;
use common::{Res, serve};

/// The login's outcome, the waits it asked for, and the requests the fake saw with a
/// `sleep` row for each wait, in the order they happened.
type Outcome = (Result<Credential, String>, Vec<Duration>, Vec<String>);

fn spec(port: u16, scopes: Value) -> Res<(OauthCode, String)> {
    let profile = json!({
        "scopes": scopes,
        "kind": "device-code",
        "client_id": "fixture-device-client",
        "device_authorization": format!("http://127.0.0.1:{port}/api/oauth/device_authorization"),
        "token": format!("http://127.0.0.1:{port}/api/oauth/token"),
        "api_base": "https://api.example.com/coding/v1"
    });
    let Kind::OauthCode(spec) = registry::parse("kimi", &profile)? else {
        return Err("a device-code profile is an OAuth profile".into());
    };
    let Login::Device {
        device_authorization,
    } = &spec.login
    else {
        return Err("a device-code profile logs in by device code".into());
    };
    let url = device_authorization.clone();
    Ok((*spec, url))
}

fn grant(expires_in: Value, interval: Value) -> (u16, Value) {
    (
        200,
        json!({
            "device_code": "GmRhmhcxhwAzkoEqiMEg_DnyEysNkuNhszIySk9eS",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://example.com/device",
            "verification_uri_complete": "https://example.com/device?user_code=WDJB-MJHT",
            "expires_in": expires_in,
            "interval": interval
        }),
    )
}

fn error(status: u16, code: &str) -> (u16, Value) {
    (status, json!({"error": code}))
}

fn tokens(expires_in: Value) -> (u16, Value) {
    (
        200,
        json!({
            "access_token": "2YotnFZFEjr1zCsicMWpAA",
            "token_type": "Bearer",
            "expires_in": expires_in,
            "refresh_token": "tGzv3JOkF0XG5Qx2TlKWIA",
            "scope": ""
        }),
    )
}

fn run(script: Vec<(u16, Value)>) -> Res<Outcome> {
    run_scoped(script, Value::Null)
}

/// The login with every wait recorded instead of slept.
fn run_scoped(script: Vec<(u16, Value)>, scopes: Value) -> Res<Outcome> {
    let (port, seen) = serve(script)?;
    let (spec, url) = spec(port, scopes)?;
    let mut waits = Vec::new();
    let outcome = device::login(&spec, &url, true, None, |wait| {
        waits.push(wait);
        if let Ok(mut log) = seen.lock() {
            log.push(format!("sleep {wait:?}"));
        }
    })
    .map_err(|error| error.to_string());
    let seen = seen.lock().map_err(|_| "poisoned")?.clone();
    Ok((outcome, waits, seen))
}

fn polls(seen: &[String]) -> usize {
    seen.iter()
        .filter(|request| request.starts_with("POST /api/oauth/token "))
        .count()
}

#[test]
fn a_device_login_polls_through_pending_and_slow_down_then_stores_the_token() -> Res {
    assert_eq!(
        spec(9, Value::Null)?.0.api_base.as_deref(),
        Some("https://api.example.com/coding/v1")
    );
    let (outcome, waits, seen) = run(vec![
        grant(json!(1800), json!(5)),
        error(400, "authorization_pending"),
        error(400, "slow_down"),
        tokens(json!(3600)),
    ])?;
    let credential = outcome?;
    let second = Duration::from_secs(1);
    assert_eq!(waits, [5 * second, 5 * second, 10 * second]);
    let order: Vec<&str> = seen
        .iter()
        .map(|row| row.split(' ').take(2).last().unwrap_or_default())
        .collect();
    let (wait, poll) = ("5s", "/api/oauth/token");
    assert_eq!(
        order,
        [
            "/api/oauth/device_authorization",
            wait,
            poll,
            wait,
            poll,
            "10s",
            poll
        ]
    );
    let (first, rest) = seen.split_first().ok_or("no request")?;
    assert!(
        first.starts_with("POST /api/oauth/device_authorization "),
        "{first}"
    );
    assert!(
        first.ends_with("\nclient_id=fixture-device-client"),
        "{first}"
    );
    assert_eq!(polls(&seen), 3);
    for poll in rest.iter().filter(|row| !row.starts_with("sleep")) {
        assert!(poll.contains("client_id=fixture-device-client"), "{poll}");
        assert!(
            poll.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "{poll}"
        );
        assert!(
            poll.contains("device_code=GmRhmhcxhwAzkoEqiMEg_DnyEysNkuNhszIySk9eS"),
            "{poll}"
        );
    }

    let root = std::env::temp_dir().join(format!("yi-device-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(root);
    store.save("kimi", &credential)?;
    let saved = store.load("kimi").ok_or("saved")?;
    assert_eq!(saved.access, "2YotnFZFEjr1zCsicMWpAA");
    assert_eq!(saved.refresh.as_deref(), Some("tGzv3JOkF0XG5Qx2TlKWIA"));
    let life = saved
        .expires
        .ok_or("expires")?
        .duration_since(store::now())?;
    assert!(
        life > Duration::from_secs(3500) && life <= Duration::from_secs(3600),
        "{life:?}"
    );
    Ok(())
}

#[test]
fn expired_and_denied_end_the_login_with_the_verb_that_restarts_it() -> Res {
    let (outcome, _, seen) = run(vec![
        grant(json!(1800), json!(5)),
        error(400, "expired_token"),
    ])?;
    let text = outcome.err().ok_or("an expired code logged in")?;
    assert!(text.contains("run: yi login kimi"), "{text}");
    assert_eq!(polls(&seen), 1);

    let (outcome, _, seen) = run(vec![
        grant(json!(1800), json!(5)),
        error(403, "access_denied"),
    ])?;
    let text = outcome.err().ok_or("a denied code logged in")?;
    assert!(
        text.contains("the login was denied in the browser"),
        "{text}"
    );
    assert_eq!(polls(&seen), 1);
    Ok(())
}

#[test]
fn the_poll_reads_the_error_code_of_any_4xx() -> Res {
    let (outcome, waits, seen) = run(vec![
        grant(json!(1800), json!(5)),
        error(401, "authorization_pending"),
        error(429, "slow_down"),
        error(410, "expired_token"),
    ])?;
    let text = outcome.err().ok_or("an expired code logged in")?;
    assert!(text.contains("expired before it was confirmed"), "{text}");
    let second = Duration::from_secs(1);
    assert_eq!(waits, [5 * second, 5 * second, 10 * second]);
    assert_eq!(polls(&seen), 3);
    Ok(())
}

#[test]
fn a_5xx_or_an_unknown_code_ends_the_login_with_the_servers_own_body() -> Res {
    let page = "<html><body>502 Bad Gateway</body></html>";
    for (status, body, want) in [
        (
            502,
            json!(page),
            "device login failed: status 502: \"<html><body>502 Bad Gateway",
        ),
        (
            502,
            json!({"error": "authorization_pending"}),
            "device login failed: status 502",
        ),
        (
            400,
            json!({"error": "invalid_grant"}),
            "device login failed: status 400",
        ),
    ] {
        let (outcome, _, seen) = run(vec![grant(json!(1800), json!(5)), (status, body)])?;
        let text = outcome.err().ok_or("a failed poll logged in")?;
        assert!(text.contains(want), "{text}");
        assert_eq!(polls(&seen), 1);
    }
    let (outcome, _, seen) = run(vec![(502, json!(page))])?;
    let text = outcome.err().ok_or("a failed device request logged in")?;
    assert!(
        text.contains("device authorization failed: status 502: \"<html>"),
        "{text}"
    );
    assert_eq!(polls(&seen), 0);
    Ok(())
}

#[test]
fn a_grant_without_interval_or_complete_uri_logs_in_and_zero_waits_one_second() -> Res {
    let (status, mut body) = grant(json!(1800), json!(5));
    let fields = body.as_object_mut().ok_or("the grant is an object")?;
    fields.remove("interval");
    fields.remove("verification_uri_complete");
    let second = Duration::from_secs(1);
    for (reply, want) in [
        ((status, body), 5 * second),
        (grant(json!(1800), json!(0)), second),
    ] {
        let (outcome, waits, _) = run(vec![reply, tokens(json!(3600))])?;
        outcome?;
        assert_eq!(waits, [want]);
    }
    Ok(())
}

#[test]
fn a_profile_with_scopes_asks_for_them_and_a_control_character_user_code_is_refused() -> Res {
    let (outcome, _, seen) = run_scoped(
        vec![grant(json!(1800), json!(5)), tokens(json!(3600))],
        json!("coding"),
    )?;
    outcome?;
    let first = seen.first().ok_or("no request")?;
    assert!(
        first.ends_with("\nclient_id=fixture-device-client&scope=coding"),
        "{first}"
    );

    let (status, mut body) = grant(json!(1800), json!(5));
    body["user_code"] = json!("WDJB\u{1b}]0;owned\u{7}");
    let (outcome, _, seen) = run(vec![(status, body)])?;
    let text = outcome.err().ok_or("a control-character code was shown")?;
    assert!(
        text.contains("device authorization failed: status 200"),
        "{text}"
    );
    assert_eq!(polls(&seen), 0);

    let (status, mut body) = grant(json!(1800), json!(5));
    body["verification_uri_complete"] = json!("https://example.com/device\u{1b}]0;owned\u{7}");
    let (outcome, _, seen) = run(vec![(status, body)])?;
    let text = outcome.err().ok_or("a control-character URI was printed")?;
    assert!(
        text.contains("device authorization failed: status 200"),
        "{text}"
    );
    assert_eq!(polls(&seen), 0);

    let (status, mut body) = grant(json!(1800), json!(5));
    body["verification_uri_complete"] = json!("file:///etc/passwd");
    let (outcome, _, _) = run(vec![(status, body)])?;
    let text = outcome.err().ok_or("a file URI was opened")?;
    assert!(
        text.contains("insecure endpoint refused: file:///etc/passwd"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_profile_names_the_key_it_lacks_misspells_or_does_not_read() -> Res {
    let device = json!({
        "kind": "device-code",
        "client_id": "fixture-device-client",
        "device_authorization": "https://auth.example.com/api/oauth/device_authorization",
        "token": "https://auth.example.com/api/oauth/token"
    });
    let refusal = |edit: &dyn Fn(&mut Value)| -> Res<String> {
        let mut profile = device.clone();
        edit(&mut profile);
        match registry::parse("kimi", &profile) {
            Ok(_) => Err(format!("accepted {profile}").into()),
            Err(error) => Ok(error.to_string()),
        }
    };
    let rename = |profile: &mut Value| {
        if let Some(fields) = profile.as_object_mut()
            && let Some(url) = fields.remove("device_authorization")
        {
            fields.insert("device_authorisation".to_owned(), url);
        }
    };
    let text = refusal(&rename)?;
    assert!(
        text.contains("unknown profile key(s) device_authorisation;"),
        "{text}"
    );
    assert!(text.contains("\"device_authorization\""), "{text}");
    let text = refusal(&|profile: &mut Value| {
        if let Some(fields) = profile.as_object_mut() {
            fields.remove("device_authorization");
        }
    })?;
    assert!(
        text.contains("kimi: profile needs `device_authorization`"),
        "{text}"
    );
    let text = refusal(&|profile: &mut Value| {
        profile["client_secret"] = json!("s");
        profile["json_token"] = json!(true);
    })?;
    assert!(
        text.contains("a profile of kind device-code reads no client_secret, json_token"),
        "{text}"
    );
    let text = refusal(&|profile: &mut Value| {
        profile["kind"] = json!("oauth-code");
        profile["authorize"] = json!("https://auth.example.com/authorize");
        profile["callback_port"] = json!(8765);
    })?;
    assert!(
        text.contains("a profile of kind oauth-code reads no device_authorization"),
        "{text}"
    );
    let text = refusal(&|profile: &mut Value| {
        profile["kind"] = json!("api-key");
        profile["api_base"] = json!("https://api.example.com/coding/v1");
    })?;
    assert!(
        text.contains("a profile of kind api-key reads no device_authorization, api_base"),
        "{text}"
    );
    let text = refusal(&|profile: &mut Value| {
        profile["api_base"] = json!("http://api.example.com/coding/v1");
    })?;
    assert!(
        text.contains("kimi: `api_base` insecure endpoint refused: http://api.example.com/"),
        "{text}"
    );
    Ok(())
}

#[test]
fn the_deadline_is_the_servers_expires_in_at_the_limit_and_the_limit_plus_one() -> Res {
    let pending = || error(400, "authorization_pending");
    let cases = [
        (15, 5, 3),
        (14, 5, 2),
        (3600, 300, 3),
        (3600, 900, 1),
        (3600, 901, 0),
    ];
    for (expires_in, interval, want) in cases {
        let mut script = vec![grant(json!(expires_in), json!(interval))];
        script.extend(std::iter::repeat_with(pending).take(5));
        let (outcome, _, seen) = run(script)?;
        let text = outcome.err().ok_or("a pending code logged in")?;
        assert!(
            text.contains("expired before it was confirmed; run: yi login kimi"),
            "{text}"
        );
        assert_eq!(
            polls(&seen),
            want,
            "expires_in {expires_in}, interval {interval}"
        );
    }
    Ok(())
}

#[test]
fn a_device_profile_refreshes_through_live_oauth_and_keeps_the_rotated_refresh() -> Res {
    let (port, seen) = serve(vec![(
        200,
        json!({"access_token": "new-access", "refresh_token": "rotated-refresh", "expires_in": 3600}),
    )])?;
    let (spec, _) = spec(port, Value::Null)?;
    let root = std::env::temp_dir().join(format!("yi-device-refresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(root);
    store.save(
        "kimi",
        &Credential {
            kind: store::Kind::Oauth,
            access: "old-access".to_owned(),
            refresh: Some("old-refresh".to_owned()),
            expires: Some(store::now() - Duration::from_secs(60)),
            account: None,
            org: None,
            extra: Default::default(),
        },
    )?;
    let live = flow::live_oauth(&spec, &store, None)?;
    assert_eq!(live.access, "new-access");
    let saved = store.load("kimi").ok_or("saved")?;
    assert_eq!(saved.refresh.as_deref(), Some("rotated-refresh"));
    let request = seen.lock().map_err(|_| "poisoned")?.join("\n");
    assert!(request.starts_with("POST /api/oauth/token "), "{request}");
    assert!(
        request.contains("grant_type=refresh_token&refresh_token=old-refresh"),
        "{request}"
    );
    Ok(())
}

#[test]
fn a_float_expires_in_still_expires() -> Res {
    let (outcome, _, _) = run(vec![grant(json!(1800), json!(5)), tokens(json!(900.0))])?;
    let expires = outcome?.expires.ok_or("a 900.0 lifetime never expires")?;
    let life = expires.duration_since(store::now())?;
    assert!(
        life > Duration::from_secs(800) && life <= Duration::from_secs(900),
        "{life:?}"
    );
    Ok(())
}

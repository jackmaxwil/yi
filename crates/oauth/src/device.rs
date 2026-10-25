use std::time::Duration;

use serde_json::Value;

use crate::Result;
use crate::flow::{http_agent, open_url, parse_tokens, seconds};
use crate::registry::OauthCode;
use crate::store::Credential;
use crate::url::https_or_local;

/// Kimi Code stops waiting at 15 minutes whatever `expires_in` says; so does Yi.
const DEADLINE_MAX: Duration = Duration::from_secs(15 * 60);
const SLOW_DOWN: Duration = Duration::from_secs(5);
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

fn post(agent: &ureq::Agent, url: &str, fields: &[(&str, &str)]) -> Result<(u16, Value)> {
    https_or_local(url)?;
    let (status, text) = match agent.post(url).send_form(fields) {
        Ok(ok) => (ok.status(), ok.into_string()?),
        Err(ureq::Error::Status(code, response)) => {
            (code, response.into_string().unwrap_or_default())
        }
        Err(error) => return Err(format!("device login: {error}").into()),
    };
    Ok((
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    ))
}

/// RFC 8628. `sleep` is the only clock: the deadline counts the waits it was handed.
pub fn login(
    spec: &OauthCode,
    device_authorization: &str,
    no_browser: bool,
    proxy: Option<&ureq::Proxy>,
    mut sleep: impl FnMut(Duration),
) -> Result<Credential> {
    let agent = http_agent(proxy);
    let mut fields = vec![("client_id", spec.client_id.as_str())];
    if !spec.scopes.is_empty() {
        fields.push(("scope", spec.scopes.as_str()));
    }
    let (status, grant) = post(&agent, device_authorization, &fields)?;
    let text = |key: &str| grant.get(key).and_then(Value::as_str);
    let printable = |shown: &&str| !shown.chars().any(char::is_control);
    let (Some(device_code), Some(user_code), Some(uri)) = (
        text("device_code"),
        text("user_code").filter(printable),
        text("verification_uri_complete")
            .or(text("verification_uri"))
            .filter(printable),
    ) else {
        return Err(format!(
            "{}: device authorization failed: status {status}: {grant}",
            spec.id
        )
        .into());
    };
    https_or_local(uri)?;
    open_url(uri, no_browser);
    println!("Confirm the code {user_code} in the browser.");
    let deadline =
        seconds(&grant, "expires_in").map_or(DEADLINE_MAX, |life| life.min(DEADLINE_MAX));
    let mut wait = seconds(&grant, "interval")
        .unwrap_or(Duration::from_secs(5))
        .max(Duration::from_secs(1));
    let mut waited = Duration::ZERO;
    let poll = [
        ("grant_type", GRANT),
        ("device_code", device_code),
        ("client_id", spec.client_id.as_str()),
    ];
    loop {
        waited = waited.saturating_add(wait);
        if waited > deadline {
            break;
        }
        sleep(wait);
        let (status, body) = post(&agent, &spec.token, &poll)?;
        match (status, body.get("error").and_then(Value::as_str)) {
            (200..=299, _) => return parse_tokens(&body),
            (400..=499, Some("authorization_pending")) => {}
            (400..=499, Some("slow_down")) => wait = wait.saturating_add(SLOW_DOWN),
            (400..=499, Some("access_denied")) => {
                return Err(format!("{}: the login was denied in the browser", spec.id).into());
            }
            (400..=499, Some("expired_token")) => break,
            _ => {
                return Err(
                    format!("{}: device login failed: status {status}: {body}", spec.id).into(),
                );
            }
        }
    }
    Err(format!(
        "{}: the device code expired before it was confirmed; run: yi login {}",
        spec.id, spec.id
    )
    .into())
}

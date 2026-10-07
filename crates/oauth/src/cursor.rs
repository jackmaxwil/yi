use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::Result;
use crate::pkce;
use crate::store::{self, Credential, Kind};
use crate::url::base64url_decode;

pub const LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
pub const POLL_URL: &str = "https://api2.cursor.sh/auth/poll";
pub const REFRESH_URL: &str = "https://api2.cursor.sh/auth/exchange_user_api_key";
const POLL_ATTEMPTS: u32 = 150;
const SLACK: Duration = Duration::from_secs(5 * 60);

pub struct Pace {
    pub attempts: u32,
    pub delay: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            attempts: POLL_ATTEMPTS,
            delay: Duration::from_secs(1),
        }
    }
}

fn uuid_v4() -> Result<String> {
    let mut bytes = pkce::random_bytes(16).map_err(crate::Error::from)?;
    if let Some(clock) = bytes.get_mut(6) {
        *clock = (*clock & 0x0f) | 0x40;
    }
    if let Some(variant) = bytes.get_mut(8) {
        *variant = (*variant & 0x3f) | 0x80;
    }
    let hex = pkce::hex(&bytes);
    let take = |start: usize, len: usize| hex.chars().skip(start).take(len).collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        take(0, 8),
        take(8, 4),
        take(12, 4),
        take(16, 4),
        take(20, 12)
    ))
}

pub fn login_url(challenge: &str, uuid: &str) -> String {
    format!("{LOGIN_URL}?challenge={challenge}&uuid={uuid}&mode=login&redirectTarget=cli")
}

fn jwt_expiry(token: &str) -> Option<SystemTime> {
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    let exp = claims.get("exp").and_then(Value::as_u64)?;
    let millis = exp
        .saturating_mul(1000)
        .saturating_sub(SLACK.as_millis() as u64);
    (millis != 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_millis(millis))
}

fn credential(body: &Value, fallback_refresh: Option<String>) -> Result<Credential> {
    let access = body
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or("cursor token response has no accessToken")?
        .to_owned();
    let refresh = body
        .get("refreshToken")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(fallback_refresh);
    let expires =
        jwt_expiry(&access).or_else(|| store::now().checked_add(Duration::from_secs(3600)));
    Ok(Credential {
        kind: Kind::Oauth,
        access,
        refresh,
        expires,
        account: None,
        org: None,
        extra: Default::default(),
    })
}

pub fn login_polled<F>(no_browser: bool, pace: Pace, mut poll: F) -> Result<Credential>
where
    F: FnMut(&str) -> std::result::Result<(u16, Vec<u8>), String>,
{
    let pkce = pkce::generate().map_err(crate::Error::from)?;
    let uuid = uuid_v4()?;
    let url = login_url(&pkce.challenge, &uuid);
    crate::flow::open_url(&url, no_browser);
    let poll_url = format!("{POLL_URL}?uuid={uuid}&verifier={}", pkce.verifier);
    let mut delay = pace.delay;
    for _ in 0..pace.attempts {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let (status, body) = poll(&poll_url).map_err(crate::Error::from)?;
        if status == 404 {
            delay = (delay.mul_f32(1.2)).min(Duration::from_secs(10));
            continue;
        }
        if status != 200 {
            return Err(format!("cursor poll failed: status code {status}").into());
        }
        let parsed: Value = serde_json::from_slice(&body)
            .map_err(|error| format!("cursor poll response: {error}"))?;
        return credential(&parsed, None);
    }
    Err("cursor authentication polling timeout".into())
}

pub fn tokens_from_exchange(body: &[u8], previous_refresh: &str) -> Result<Credential> {
    let parsed: Value = serde_json::from_slice(body)
        .map_err(|error| format!("cursor refresh response: {error}"))?;
    credential(&parsed, Some(previous_refresh.to_owned()))
}

pub fn login_ureq(no_browser: bool, proxy: Option<&ureq::Proxy>) -> Result<Credential> {
    let agent = {
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30));
        if let Some(proxy) = proxy {
            builder = builder.proxy(proxy.clone());
        }
        builder.build()
    };
    login_polled(no_browser, Pace::default(), |url| {
        match agent.get(url).call() {
            Ok(response) => Ok((
                200,
                response
                    .into_string()
                    .map_err(|error| error.to_string())?
                    .into_bytes(),
            )),
            Err(ureq::Error::Status(code, response)) => Ok((
                code,
                response.into_string().unwrap_or_default().into_bytes(),
            )),
            Err(error) => Err(error.to_string()),
        }
    })
}

pub fn refresh_ureq(refresh_token: &str, proxy: Option<&ureq::Proxy>) -> Result<Credential> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30));
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    let response = builder
        .build()
        .post(REFRESH_URL)
        .set("authorization", &format!("Bearer {refresh_token}"))
        .set("content-type", "application/json")
        .send_string("{}");
    let body = match response {
        Ok(ok) => ok
            .into_string()
            .map_err(|error| format!("cursor refresh response: {error}"))?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("cursor refresh failed: status code {code}: {text}").into());
        }
        Err(error) => return Err(format!("cursor refresh failed: {error}").into()),
    };
    tokens_from_exchange(body.as_bytes(), refresh_token)
}

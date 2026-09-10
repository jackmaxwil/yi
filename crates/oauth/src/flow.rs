use serde_json::{Value, json};

use crate::loopback;
use crate::pkce;
use crate::registry::{DeviceCode, OauthCode};
use crate::store::{self, Credential, Kind, Store};

pub struct LoginOptions {
    pub no_browser: bool,
    pub inspect_url: Option<fn(&str)>,
}

fn https_or_local(url: &str) -> Result<(), String> {
    let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
    if url.starts_with("https://") || local {
        Ok(())
    } else {
        Err(format!("insecure endpoint refused: {url}"))
    }
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn http_agent(proxy: Option<&ureq::Proxy>) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30));
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    builder.build()
}

fn parse_tokens(body: &Value) -> Result<Credential, String> {
    let access = body
        .get("access_token")
        .or_else(|| body.get("key"))
        .and_then(Value::as_str)
        .ok_or("token response has no access_token")?
        .to_owned();
    let expires_ms = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|seconds| store::now_ms().saturating_add(seconds.saturating_mul(1000)))
        .unwrap_or(0);
    Ok(Credential {
        kind: Kind::Oauth,
        access,
        refresh: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        expires_ms,
        account: body
            .pointer("/account/uuid")
            .or_else(|| body.pointer("/account/email_address"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        org: body
            .pointer("/organization/uuid")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn post_token(
    spec: &OauthCode,
    proxy: Option<&ureq::Proxy>,
    fields: &[(&str, &str)],
    refresh: bool,
) -> Result<Credential, String> {
    https_or_local(&spec.token)?;
    let agent = http_agent(proxy);
    let mut request = agent.post(&spec.token);
    if refresh {
        for (name, value) in &spec.refresh_headers {
            request = request.set(name, value);
        }
    }
    let response = if spec.json_token {
        let mut map = serde_json::Map::new();
        for (key, value) in fields {
            map.insert((*key).to_owned(), json!(*value));
        }
        request
            .set("content-type", "application/json")
            .send_string(&Value::Object(map).to_string())
    } else {
        request.send_form(fields)
    };
    let text = match response {
        Ok(ok) => ok
            .into_string()
            .map_err(|error| format!("token response: {error}"))?,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            return Err(format!(
                "token request failed: {}: status code {code}: {body}",
                spec.token
            ));
        }
        Err(error) => return Err(format!("token request failed: {error}")),
    };
    parse_tokens(&serde_json::from_str(&text).map_err(|error| format!("token response: {error}"))?)
}

fn open_url(url: &str, no_browser: bool) {
    if no_browser {
        println!("Open this URL to authorize: {url}");
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "launching the user's browser for the interactive login is this fn's job"
    )]
    if std::process::Command::new(opener).arg(url).spawn().is_err() {
        println!("Open this URL to authorize: {url}");
    }
}

pub fn login_oauth(
    spec: &OauthCode,
    options: &LoginOptions,
    proxy: Option<&ureq::Proxy>,
) -> Result<Credential, String> {
    let listener = loopback::bind(spec.callback_port, spec.port_fallback)?;
    let redirect = loopback::redirect_uri(&listener, &spec.redirect_host, &spec.callback_path)?;
    let pkce = pkce::generate()?;
    let mut auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}&scope={}",
        &spec.authorize,
        urlencode(&spec.client_id),
        urlencode(&redirect),
        pkce.challenge,
        pkce.state,
        urlencode(&spec.scopes),
    );
    for (key, value) in &spec.extra_authorize {
        auth_url.push_str(&format!("&{key}={}", urlencode(value)));
    }
    if let Some(inspect) = options.inspect_url {
        inspect(&auth_url);
    }
    open_url(&auth_url, options.no_browser);
    let callback = loopback::wait_for_callback(&listener)?;
    if callback.state != pkce.state {
        return Err("state mismatch in OAuth callback (possible CSRF); aborting".to_owned());
    }
    // Anthropic's token JSON requires `state`. `code=true` may return `code#state`.
    let (code, hash_state) = callback
        .code
        .split_once('#')
        .map(|(code, state)| (code.to_owned(), Some(state.to_owned())))
        .unwrap_or_else(|| (callback.code.clone(), None));
    let state = hash_state.unwrap_or(callback.state.clone());
    let mut fields = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("state", state.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", spec.client_id.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
    ];
    if let Some(secret) = spec.client_secret.as_deref() {
        fields.push(("client_secret", secret));
    }
    post_token(spec, proxy, &fields, false)
}

pub fn login_device(spec: &DeviceCode, proxy: Option<&ureq::Proxy>) -> Result<Credential, String> {
    https_or_local(&spec.device)?;
    https_or_local(&spec.token)?;
    let client = http_agent(proxy);
    let started = client
        .post(&spec.device)
        .send_form(&[
            ("client_id", spec.client_id.as_str()),
            ("scope", spec.scopes.as_str()),
        ])
        .map_err(|error| format!("device code request failed: {error}"))?
        .into_string()
        .map_err(|error| error.to_string())?;
    let body: Value =
        serde_json::from_str(&started).map_err(|error| format!("device code response: {error}"))?;
    let device_code = body
        .get("device_code")
        .and_then(Value::as_str)
        .ok_or("device response has no device_code")?;
    let user_code = body.get("user_code").and_then(Value::as_str).unwrap_or("");
    let verify = body
        .get("verification_uri")
        .and_then(Value::as_str)
        .unwrap_or("");
    println!("Visit {verify} and enter {user_code}");
    let interval = body.get("interval").and_then(Value::as_u64).unwrap_or(5);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("device login timed out".to_owned());
        }
        std::thread::sleep(std::time::Duration::from_secs(interval.max(1)));
        let token_agent = http_agent(proxy);
        match token_agent.post(&spec.token).send_form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", spec.client_id.as_str()),
        ]) {
            Ok(response) => {
                let text = response.into_string().map_err(|error| error.to_string())?;
                return parse_tokens(
                    &serde_json::from_str(&text)
                        .map_err(|error| format!("token response: {error}"))?,
                );
            }
            Err(ureq::Error::Status(400 | 428, _)) => continue,
            Err(error) => return Err(format!("device token poll failed: {error}")),
        }
    }
}

pub fn login_api_key(_provider: &str, key: String) -> Result<Credential, String> {
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("empty API key".to_owned());
    }
    Ok(Credential {
        kind: Kind::Key,
        access: key,
        refresh: None,
        expires_ms: 0,
        account: None,
        org: None,
    })
}

pub fn refresh(
    spec: &OauthCode,
    stored: &Credential,
    proxy: Option<&ureq::Proxy>,
) -> Result<Credential, String> {
    let refresh_token = stored
        .refresh
        .as_deref()
        .ok_or_else(|| format!("token expired; run: yi login {}", spec.id))?;
    let mut fields = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", spec.client_id.as_str()),
    ];
    if let Some(secret) = spec.client_secret.as_deref() {
        fields.push(("client_secret", secret));
    }
    let mut next = post_token(spec, proxy, &fields, true)?;
    if next.refresh.is_none() {
        next.refresh = stored.refresh.clone();
    }
    next.account = stored.account.clone();
    next.org = stored.org.clone();
    Ok(next)
}

pub fn live_oauth(
    spec: &OauthCode,
    store: &Store,
    proxy: Option<&ureq::Proxy>,
) -> Result<Credential, String> {
    let current = store
        .load(&spec.id)
        .ok_or_else(|| format!("no stored tokens; run: yi login {}", spec.id))?;
    if !Store::expired(&current, store::now_ms()) {
        return Ok(current);
    }
    store.with_refresh_lock(&spec.id, || {
        let current = store.load(&spec.id).unwrap_or(current.clone());
        if !Store::expired(&current, store::now_ms()) {
            return Ok(current);
        }
        let refreshed = refresh(spec, &current, proxy)?;
        store.save(&spec.id, &refreshed)?;
        Ok(refreshed)
    })
}

pub fn save(provider: &str, credential: &Credential) -> Result<(), String> {
    Store::user().save(provider, credential)
}

pub fn logout(provider: &str) -> Result<(), String> {
    Store::user().delete(provider)
}

use serde_json::{Value, json};

use crate::loopback;
use crate::pkce;
use crate::registry::OauthCode;
use crate::store::{self, Credential, Kind, Store};
use crate::url::{base64url_decode, https_or_local, urlencode};
use crate::Result;

pub struct LoginOptions {
    pub no_browser: bool,
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

fn parse_tokens(body: &Value) -> Result<Credential> {
    let access = body
        .get("access_token")
        .or_else(|| body.get("key"))
        .and_then(Value::as_str)
        .ok_or("token response has no access_token")?
        .to_owned();
    let expires = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|seconds| store::now() + std::time::Duration::from_secs(seconds));
    Ok(Credential {
        kind: Kind::Oauth,
        access,
        refresh: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        expires,
        account: body
            .pointer("/account/uuid")
            .or_else(|| body.pointer("/account/email_address"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        org: body
            .pointer("/organization/uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| id_token_account(body)),
        extra: Default::default(),
    })
}

/// ChatGPT's token response carries no `/organization/uuid`; the account id is the
/// id_token JWT's `https://api.openai.com/auth` claim (where codex-rs and Pi read it).
fn id_token_account(body: &Value) -> Option<String> {
    let id_token = body.get("id_token").and_then(Value::as_str)?;
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    claims
        .pointer(
            "/https:~1api.openai.com~1auth/chatgpt_account_id"
                .replace("~1", "/")
                .as_str(),
        )
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn post_token(
    spec: &OauthCode,
    proxy: Option<&ureq::Proxy>,
    fields: &[(&str, &str)],
    refresh: bool,
) -> Result<Credential> {
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
            )
            .into());
        }
        Err(error) => return Err(format!("token request failed: {error}").into()),
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
) -> Result<Credential> {
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
    open_url(&auth_url, options.no_browser);
    let callback = loopback::wait_for_callback(&listener, &spec.callback_path)?;
    if callback.state != pkce.state {
        return Err("state mismatch in OAuth callback (possible CSRF); aborting".into());
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

pub fn login_api_key(key: String) -> Result<Credential> {
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("empty API key".into());
    }
    Ok(Credential {
        kind: Kind::Key,
        access: key,
        refresh: None,
        expires: None,
        account: None,
        org: None,
        extra: Default::default(),
    })
}

pub fn refresh(
    spec: &OauthCode,
    stored: &Credential,
    proxy: Option<&ureq::Proxy>,
) -> Result<Credential> {
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
    next.extra = stored.extra.clone();
    Ok(next)
}

pub fn live_oauth(
    spec: &OauthCode,
    store: &Store,
    proxy: Option<&ureq::Proxy>,
) -> Result<Credential> {
    let current = store
        .load(&spec.id)
        .ok_or_else(|| format!("no stored tokens; run: yi login {}", spec.id))?;
    if !Store::expired(&current) {
        return Ok(current);
    }
    store.with_refresh_lock(&spec.id, || {
        let current = store.load(&spec.id).unwrap_or(current.clone());
        if !Store::expired(&current) {
            return Ok(current);
        }
        let refreshed = refresh(spec, &current, proxy)?;
        store.save(&spec.id, &refreshed)?;
        Ok(refreshed)
    })
}

pub fn save(provider: &str, credential: &Credential) -> Result<()> {
    Store::user().save(provider, credential)
}

pub fn logout(provider: &str) -> Result<()> {
    Store::user().delete(provider)
}

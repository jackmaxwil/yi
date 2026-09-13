use yi_oauth::flow;
use yi_oauth::registry;
use yi_oauth::store::{Kind, Store};

pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    ApiKey,
    Oauth,
}

pub struct Resolved {
    pub secret: Secret,
    pub kind: AuthKind,
    pub org: Option<String>,
    /// When the credential expires, so a long-lived session can re-resolve instead of
    /// streaming a dead token (None = never).
    pub expires: Option<std::time::SystemTime>,
    /// The login profile's `stream_headers` (D191). Yi ships none: this is whatever
    /// the user's own `~/.yi/oauth/<provider>.json` carries.
    pub headers: Vec<(String, String)>,
}

fn env_var(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "google" => "GEMINI_API_KEY",
        _ => return None,
    })
}

fn env_key(provider: &str) -> Option<Secret> {
    let variable = env_var(provider)?;
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
        .map(Secret::new)
}

/// A stored OAuth credential is refreshed before it is handed out, through the
/// session's proxy; a failed refresh falls back to the disk copy (the 401 names `yi login`).
fn stored(
    provider: &str,
    proxy: Option<&crate::request::ProxyConfig>,
) -> Option<(yi_oauth::store::Credential, Vec<(String, String)>)> {
    let store = Store::user();
    let current = store.load(provider)?;
    let spec = match registry::lookup(provider) {
        Ok(Some(registry::Kind::OauthCode(spec))) => spec,
        Ok(_) => return Some((current, Vec::new())),
        // A broken profile is not a missing credential: refuse it so
        // `yi login <provider>` prints the parse error instead of streaming blind.
        Err(_) => return None,
    };
    let headers = spec.stream_headers.clone();
    if current.kind != Kind::Oauth {
        return Some((current, headers));
    }
    let refresh_proxy =
        proxy.and_then(|config| config.proxy_for(crate::request::host_of(&spec.token)));
    let live = flow::live_oauth(&spec, &store, refresh_proxy).unwrap_or(current);
    Some((live, headers))
}

pub fn resolve(provider: &str) -> Option<Resolved> {
    resolve_with_proxy(provider, None)
}

pub fn resolve_with_proxy(
    provider: &str,
    proxy: Option<&crate::request::ProxyConfig>,
) -> Option<Resolved> {
    if let Some(secret) = env_key(provider) {
        return Some(Resolved {
            secret,
            kind: AuthKind::ApiKey,
            org: None,
            expires: None,
            headers: Vec::new(),
        });
    }
    let (credential, headers) = stored(provider, proxy)?;
    Some(Resolved {
        secret: Secret::new(credential.access),
        kind: match credential.kind {
            Kind::Oauth => AuthKind::Oauth,
            Kind::Key => AuthKind::ApiKey,
        },
        org: credential.org,
        expires: credential.expires,
        headers,
    })
}

pub fn api_key(provider: &str) -> Option<Secret> {
    resolve(provider).map(|resolved| resolved.secret)
}

pub fn missing_message(provider: &str) -> String {
    let env = env_var(provider)
        .map(|name| format!(" or export {name}"))
        .unwrap_or_default();
    format!("no credential for provider {provider} (run: yi login {provider}{env})")
}

pub fn login(args: &[String]) -> i32 {
    yi_oauth::cli::run("login", args)
}

pub fn logout(args: &[String]) -> i32 {
    yi_oauth::cli::run("logout", args)
}

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
    /// The login profile's `stream_headers` (D172). Yi ships none: this is whatever
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

/// A stored OAuth credential is refreshed before it is handed out; a refresh that
/// fails falls back to what is on disk, so the provider names the 401, not the store.
fn stored(provider: &str) -> Option<(yi_oauth::store::Credential, Vec<(String, String)>)> {
    let store = Store::user();
    let current = store.load(provider)?;
    let Some(registry::Kind::OauthCode(spec)) = registry::lookup(provider) else {
        return Some((current, Vec::new()));
    };
    let headers = spec.stream_headers.clone();
    if current.kind != Kind::Oauth {
        return Some((current, headers));
    }
    let live = flow::live_oauth(&spec, &store, None).unwrap_or(current);
    Some((live, headers))
}

pub fn resolve(provider: &str) -> Option<Resolved> {
    if let Some(secret) = env_key(provider) {
        return Some(Resolved {
            secret,
            kind: AuthKind::ApiKey,
            org: None,
            headers: Vec::new(),
        });
    }
    let (credential, headers) = stored(provider)?;
    Some(Resolved {
        secret: Secret::new(credential.access),
        kind: match credential.kind {
            Kind::Oauth => AuthKind::Oauth,
            Kind::Key => AuthKind::ApiKey,
        },
        org: credential.org,
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

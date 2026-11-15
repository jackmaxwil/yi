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

/// Where a `config.json` provider's key is: `apiKeyEnv`'s variable, else the literal `apiKey`.
#[derive(Debug, Default)]
pub struct CustomKey {
    pub env: Option<String>,
    pub literal: Option<Secret>,
}

static CUSTOM_KEYS: std::sync::OnceLock<Vec<(String, CustomKey)>> = std::sync::OnceLock::new();

/// Set once at config load, before the first credential is resolved.
pub fn set_custom_keys(keys: Vec<(String, CustomKey)>) {
    let _already_set = CUSTOM_KEYS.set(keys);
}

fn custom_key(provider: &str) -> Option<&'static CustomKey> {
    (CUSTOM_KEYS.get()?.iter())
        .find(|(id, _)| id == provider)
        .map(|(_, key)| key)
}

fn env_var(provider: &str) -> Option<&'static str> {
    if let Some(custom) = custom_key(provider) {
        return custom.env.as_deref();
    }
    (yi_types::PROVIDER_KEY_VARS.iter())
        .find(|(name, _)| *name == provider)
        .map(|(_, variable)| *variable)
}

fn env_key(provider: &str) -> Option<Secret> {
    let from_env = env_var(provider)
        .and_then(|variable| std::env::var(variable).ok())
        .filter(|value| !value.is_empty());
    from_env
        .or_else(|| Some(custom_key(provider)?.literal.as_ref()?.expose().to_owned()))
        .filter(|value| !value.is_empty())
        .map(Secret::new)
}

fn refresh_cursor(
    store: Store,
    provider: &str,
    current: yi_oauth::store::Credential,
    proxy: Option<&crate::request::ProxyConfig>,
) -> (yi_oauth::store::Credential, Vec<(String, String)>) {
    if current.kind != Kind::Oauth {
        return (current, Vec::new());
    }
    if current.refresh.is_none() {
        return (current, Vec::new());
    }
    let wire = proxy.and_then(|config| {
        config.proxy_for(crate::request::host_of(yi_oauth::cursor::REFRESH_URL))
    });
    // The token re-read inside the lock, not the one captured before it: another process may
    // have rotated it while this one waited.
    let fresh = flow::refresh_locked(&store, provider, current.clone(), |locked| {
        let token = locked
            .refresh
            .as_deref()
            .ok_or("cursor: no refresh token")?;
        yi_oauth::cursor::refresh_ureq(token, wire)
    })
    .unwrap_or(current);
    (fresh, Vec::new())
}

/// The stored credential and its login profile; a broken profile refuses it so
/// `yi login <provider>` prints the parse error instead of streaming blind.
fn load_valid(
    store: &Store,
    provider: &str,
) -> Option<(yi_oauth::store::Credential, Option<registry::Kind>)> {
    let current = store.load(provider)?;
    let kind = registry::refresh_kind(provider, current.kind == Kind::Oauth).ok()?;
    Some((current, kind))
}

/// A stored OAuth credential is refreshed before it is handed out, through the
/// session's proxy; a failed refresh falls back to the disk copy (the 401 names `yi login`).
fn stored(
    root: std::path::PathBuf,
    provider: &str,
    proxy: Option<&crate::request::ProxyConfig>,
) -> Option<(yi_oauth::store::Credential, Vec<(String, String)>)> {
    let store = Store::open(root);
    let (current, kind) = load_valid(&store, provider)?;
    let spec = match kind {
        Some(registry::Kind::Cursor) => {
            return Some(refresh_cursor(store, provider, current, proxy));
        }
        Some(registry::Kind::OauthCode(spec)) => spec,
        Some(registry::Kind::ApiKey) | None => return Some((current, Vec::new())),
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

/// The provider's key from the environment or the store at `root`.
fn key_at(root: std::path::PathBuf, provider: &str) -> Option<Secret> {
    resolve_at(root, provider, None).map(|resolved| resolved.secret)
}

/// The provider's key from the environment or the `yi login` store under `home`.
pub fn stored_key(home: &std::path::Path, provider: &str) -> Option<Secret> {
    key_at(Store::root_under(home), provider)
}

pub fn resolve(provider: &str) -> Option<Resolved> {
    resolve_with_proxy(provider, None)
}

pub fn has_credential(provider: &str) -> bool {
    env_key(provider).is_some()
        || load_valid(&Store::open(Store::default_root()), provider).is_some()
}

pub fn resolve_with_proxy(
    provider: &str,
    proxy: Option<&crate::request::ProxyConfig>,
) -> Option<Resolved> {
    resolve_at(Store::default_root(), provider, proxy)
}

fn resolve_at(
    root: std::path::PathBuf,
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
    let (credential, headers) = stored(root, provider, proxy)?;
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
    let env = env_var(provider);
    if custom_key(provider).is_some() {
        return match env {
            Some(name) => format!("no credential for provider {provider} (export {name})"),
            None => format!(
                "no credential for provider {provider} (set providers.{provider}.apiKeyEnv in config.json)"
            ),
        };
    }
    let env = env
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

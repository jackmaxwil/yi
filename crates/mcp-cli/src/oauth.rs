use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use serde_json::{Value, json};
use yi_types::mcp::{McpOauthProfile, McpTokenSet};

use crate::pkce;
use crate::sessions::now_ms;
use crate::tokens::Tokens;

const CALLBACK_TIMEOUT_SECS: u64 = 120;
// Refresh this long before nominal expiry so a token never dies mid-request.
const EXPIRY_SLACK_MS: u64 = 30_000;

fn https_or_local(url: &str) -> Result<(), String> {
    let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
    if url.starts_with("https://") || local {
        Ok(())
    } else {
        Err(format!("insecure endpoint refused: {url}"))
    }
}

fn host_of(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.split('@').next_back().unwrap_or(rest)
}

fn origin_of(url: &str) -> String {
    let scheme_end = url.find("://").map_or(0, |index| index + 3);
    let path_start = url[scheme_end..]
        .find('/')
        .map_or(url.len(), |index| scheme_end + index);
    url[..path_start].to_owned()
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<Value, String> {
    https_or_local(url)?;
    let text = agent
        .get(url)
        .call()
        .map_err(|error| format!("GET {url}: {error}"))?
        .into_string()
        .map_err(|error| format!("GET {url}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("GET {url}: bad JSON: {error}"))
}

fn parse_resource_metadata_url(www_authenticate: &str) -> Option<String> {
    let marker = "resource_metadata=\"";
    let start = www_authenticate.find(marker)? + marker.len();
    let end = www_authenticate[start..].find('"')? + start;
    Some(www_authenticate[start..end].to_owned())
}

pub struct Discovered {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub iss_required: bool,
}

/// RFC 9728 → RFC 8414 discovery (D37). The authorization server metadata is
/// validated for issuer binding (codex lesson): issuer must be present and
/// endpoint origins must be https (or loopback for tests).
pub fn discover(agent: &ureq::Agent, server_url: &str) -> Result<Discovered, String> {
    let resource_metadata_url = match agent.get(server_url).call() {
        Err(ureq::Error::Status(401, response)) => response
            .header("www-authenticate")
            .and_then(parse_resource_metadata_url),
        _ => None,
    }
    .unwrap_or_else(|| {
        format!(
            "{}/.well-known/oauth-protected-resource",
            origin_of(server_url)
        )
    });
    let resource = get_json(agent, &resource_metadata_url)?;
    let auth_server = resource
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| origin_of(server_url));
    let metadata_url = format!(
        "{}/.well-known/oauth-authorization-server",
        auth_server.trim_end_matches('/')
    );
    let metadata = get_json(agent, &metadata_url)?;
    let issuer = metadata
        .get("issuer")
        .and_then(Value::as_str)
        .ok_or("authorization server metadata has no issuer")?
        .to_owned();
    https_or_local(&issuer)?;
    let endpoint = |key: &str| -> Result<String, String> {
        let url = metadata
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("authorization server metadata has no {key}"))?;
        https_or_local(url)?;
        Ok(url.to_owned())
    };
    Ok(Discovered {
        authorization_endpoint: endpoint("authorization_endpoint")?,
        token_endpoint: endpoint("token_endpoint")?,
        registration_endpoint: metadata
            .get("registration_endpoint")
            .and_then(Value::as_str)
            .map(str::to_owned),
        iss_required: metadata
            .get("authorization_response_iss_parameter_supported")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        issuer,
    })
}

/// RFC 7591 dynamic registration of a public client.
fn register_client(
    agent: &ureq::Agent,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<String, String> {
    https_or_local(registration_endpoint)?;
    let response = agent
        .post(registration_endpoint)
        .set("content-type", "application/json")
        .send_string(
            &json!({
                "client_name": "yi",
                "redirect_uris": [redirect_uri],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })
            .to_string(),
        )
        .map_err(|error| format!("client registration failed: {error}"))?;
    let text = response
        .into_string()
        .map_err(|error| format!("registration response: {error}"))?;
    let body: Value =
        serde_json::from_str(&text).map_err(|error| format!("registration response: {error}"))?;
    body.get("client_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or("registration response has no client_id".to_owned())
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

fn urldecode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 3 <= bytes.len() => {
                let hex = &text[index + 1..index + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| urldecode(value))
    })
}

struct Callback {
    code: String,
    state: String,
    iss: Option<String>,
}

/// One-shot loopback listener: single connection, 120 s timeout, then a
/// friendly page. Binds 127.0.0.1 only.
fn wait_for_callback(listener: &TcpListener) -> Result<Callback, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(CALLBACK_TIMEOUT_SECS);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "no OAuth callback within {CALLBACK_TIMEOUT_SECS}s; aborting"
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("callback listener failed: {error}")),
        }
    };
    stream
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buffer = [0u8; 8192];
    let read = stream
        .read(&mut buffer)
        .map_err(|error| error.to_string())?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("malformed callback request")?;
    let query = path.split_once('?').map(|(_, query)| query).unwrap_or("");
    let code = query_param(query, "code").ok_or("callback missing code")?;
    let state = query_param(query, "state").ok_or("callback missing state")?;
    let iss = query_param(query, "iss");
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\n<html><body>Login complete. Return to the terminal.</body></html>",
    );
    Ok(Callback { code, state, iss })
}

fn parse_token_response(body: Value) -> Result<McpTokenSet, String> {
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("token response has no access_token")?
        .to_owned();
    let expires_at_ms = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|seconds| now_ms().saturating_add(seconds.saturating_mul(1000)))
        .unwrap_or(0);
    Ok(McpTokenSet {
        access_token,
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        expires_at_ms,
        scope: body.get("scope").and_then(Value::as_str).map(str::to_owned),
        extra: serde_json::Map::new(),
    })
}

pub struct LoginOptions {
    pub profile: String,
    pub scopes: Vec<String>,
    pub client_id: Option<String>,
    pub no_browser: bool,
}

pub fn profile_key(profile: &str, server_url: &str) -> String {
    format!("{profile}@{}", host_of(server_url))
}

/// The full authorization-code + PKCE login (D37). Never triggered by a 401 —
/// this runs only from the explicit `yi mcp login` command.
pub fn login(
    agent: &ureq::Agent,
    server_url: &str,
    options: &LoginOptions,
) -> Result<(McpOauthProfile, McpTokenSet), String> {
    let discovered = discover(agent, server_url)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("cannot bind loopback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let client_id = match &options.client_id {
        Some(id) => id.clone(),
        None => {
            let endpoint = discovered
                .registration_endpoint
                .as_deref()
                .ok_or("server offers no dynamic registration; pass --client-id")?;
            register_client(agent, endpoint, &redirect_uri)?
        }
    };
    let pkce = pkce::generate()?;
    let mut auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}&resource={}",
        discovered.authorization_endpoint,
        urlencode(&client_id),
        urlencode(&redirect_uri),
        pkce.challenge,
        pkce.state,
        urlencode(server_url),
    );
    if !options.scopes.is_empty() {
        auth_url.push_str(&format!("&scope={}", urlencode(&options.scopes.join(" "))));
    }
    if options.no_browser {
        println!("Open this URL to authorize: {auth_url}");
    } else {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        #[expect(
            clippy::disallowed_methods,
            reason = "launching the user's browser for the interactive login is this fn's job"
        )]
        let launched = std::process::Command::new(opener).arg(&auth_url).spawn();
        if launched.is_err() {
            println!("Open this URL to authorize: {auth_url}");
        }
    }
    let callback = wait_for_callback(&listener)?;
    if callback.state != pkce.state {
        return Err("state mismatch in OAuth callback (possible CSRF); aborting".to_owned());
    }
    if discovered.iss_required {
        let iss = callback.iss.as_deref().unwrap_or("");
        if iss != discovered.issuer {
            return Err(format!(
                "issuer mismatch in OAuth callback: expected {}, got {iss}",
                discovered.issuer
            ));
        }
    }
    let response = agent
        .post(&discovered.token_endpoint)
        .send_form(&[
            ("grant_type", "authorization_code"),
            ("code", &callback.code),
            ("redirect_uri", &redirect_uri),
            ("client_id", &client_id),
            ("code_verifier", &pkce.verifier),
            ("resource", server_url),
        ])
        .map_err(|error| format!("token exchange failed: {error}"))?;
    let body = response
        .into_string()
        .map_err(|error| format!("token response: {error}"))?;
    let tokens = parse_token_response(
        serde_json::from_str(&body).map_err(|error| format!("token response: {error}"))?,
    )?;
    let timestamp = now_ms();
    let profile = McpOauthProfile {
        name: options.profile.clone(),
        server_url: server_url.to_owned(),
        issuer: discovered.issuer,
        client_id,
        authorization_endpoint: discovered.authorization_endpoint,
        token_endpoint: discovered.token_endpoint,
        iss_required: discovered.iss_required,
        scopes: options.scopes.clone(),
        created_at: timestamp,
        updated_at: timestamp,
        extra: serde_json::Map::new(),
    };
    Ok((profile, tokens))
}

fn expired(tokens: &McpTokenSet) -> bool {
    tokens.expires_at_ms != 0 && now_ms().saturating_add(EXPIRY_SLACK_MS) >= tokens.expires_at_ms
}

/// Returns a live access token for the profile, refreshing under the
/// cross-process lock when expired. Re-reads the store inside the lock — the
/// other process may have already refreshed.
pub fn access_token(
    agent: &ureq::Agent,
    profile: &McpOauthProfile,
    store: &Tokens,
) -> Result<String, String> {
    let key = profile_key(&profile.name, &profile.server_url);
    let tokens = store.load(&key).ok_or_else(|| {
        format!(
            "no stored tokens for {key}; run: yi mcp login {}",
            profile.server_url
        )
    })?;
    if !expired(&tokens) {
        return Ok(tokens.access_token);
    }
    store.with_refresh_lock(&key, || {
        let current = store.load(&key).unwrap_or(tokens.clone());
        if !expired(&current) {
            return Ok(current.access_token);
        }
        let refresh_token = current.refresh_token.clone().ok_or_else(|| {
            format!(
                "token expired and no refresh token; run: yi mcp login {}",
                profile.server_url
            )
        })?;
        let response = agent
            .post(&profile.token_endpoint)
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
                ("client_id", &profile.client_id),
                ("resource", &profile.server_url),
            ])
            .map_err(|error| format!("token refresh failed: {error}"))?;
        let body = response
            .into_string()
            .map_err(|error| format!("refresh response: {error}"))?;
        let mut refreshed = parse_token_response(
            serde_json::from_str(&body).map_err(|error| format!("refresh response: {error}"))?,
        )?;
        if refreshed.refresh_token.is_none() {
            refreshed.refresh_token = Some(refresh_token);
        }
        store.save(&key, &refreshed)?;
        Ok(refreshed.access_token)
    })
}

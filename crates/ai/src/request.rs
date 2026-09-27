use serde_json::Value;
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::{AssistantMessageEvent, Wait};
use yi_types::message::{AgentMessage, StopReason, Usage};
use yi_types::model::Model;

use crate::retry::{RetryPolicy, is_retryable_status, retry_delay};
use crate::sse::SseEvent;

pub fn empty_assistant(model: &Model) -> AgentMessage {
    AgentMessage::Assistant {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        // A stream that dies before its usage chunk must not read as a free
        // turn; every mapper clears this when a usage object actually arrives.
        usage: Usage::unknown(),
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// Invariant: a configured proxy is applied or startup fails — degrading to a
/// direct connection in an air-gapped runner is an unattributable hang (E2).
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    proxy: ureq::Proxy,
    no_proxy: Vec<String>,
}

impl ProxyConfig {
    pub fn from_values(
        https: Option<&str>,
        http: Option<&str>,
        no_proxy: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let Some(url) = [https, http]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        if url.starts_with("socks") {
            return Err(format!(
                "SOCKS proxies are not supported: {}",
                redacted(url)
            ));
        }
        let proxy = ureq::Proxy::new(url)
            .map_err(|error| format!("invalid proxy {}: {error}", redacted(url)))?;
        Ok(Some(Self {
            proxy,
            no_proxy: no_proxy
                .unwrap_or_default()
                .split(',')
                .map(|entry| entry.trim().trim_start_matches('.').to_lowercase())
                .filter(|entry| !entry.is_empty())
                .collect(),
        }))
    }

    // ponytail: host suffix and `*` only — no ports, no CIDR.
    pub fn proxy_for(&self, host: &str) -> Option<&ureq::Proxy> {
        let host = host.to_lowercase();
        let bypass = self
            .no_proxy
            .iter()
            .any(|entry| entry == "*" || host == *entry || host.ends_with(&format!(".{entry}")));
        (!bypass).then_some(&self.proxy)
    }
}

/// Incident: a proxy url carries inline basic-auth and every refusal above is
/// printed to stderr, so the userinfo is dropped before the value is named back.
fn redacted(url: &str) -> String {
    let start = url.find("://").map_or(0, |at| at.saturating_add(3));
    let rest = url.get(start..).unwrap_or_default();
    let authority = rest.split_once('/').map_or(rest, |(head, _)| head);
    let Some(at) = authority.rfind('@') else {
        return url.to_owned();
    };
    let scheme = url.get(..start).unwrap_or_default();
    let host = rest.get(at.saturating_add(1)..).unwrap_or_default();
    format!("{scheme}***@{host}")
}

pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split_once('/').map_or(rest, |(host, _)| host);
    rest.split_once(':').map_or(rest, |(host, _)| host)
}

/// One GET with a byte cap, no retry: a catalog refresh can wait for the next session.
pub fn get_json(
    url: &str,
    headers: &[(&str, String)],
    proxy: Option<&ProxyConfig>,
    cap: usize,
) -> Result<Value, String> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(20));
    if let Some(proxy) = proxy.and_then(|config| config.proxy_for(host_of(url))) {
        builder = builder.proxy(proxy.clone());
    }
    let mut request = builder.build().get(url).set("accept", "application/json");
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request.call().map_err(|error| format!("{url}: {error}"))?;
    let mut body = Vec::new();
    let limit = u64::try_from(cap.saturating_add(1)).unwrap_or(u64::MAX);
    let mut reader = std::io::Read::take(response.into_reader(), limit);
    std::io::Read::read_to_end(&mut reader, &mut body)
        .map_err(|error| format!("{url}: {error}"))?;
    if body.len() > cap {
        return Err(format!("{url}: body over {cap} bytes"));
    }
    serde_json::from_slice(&body).map_err(|error| format!("{url}: {error}"))
}

pub fn waiting(sender: &Sender<AssistantMessageEvent>) -> impl Fn(Wait) + '_ {
    move |wait| {
        let _ = sender.blocking_send(AssistantMessageEvent::Waiting { wait });
    }
}

/// Past this a pooled connection is not trusted: a NAT may have dropped it without a FIN.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(30);

static POOLED: std::sync::Mutex<Option<(ureq::Agent, std::time::Instant)>> =
    std::sync::Mutex::new(None);

// ponytail: a proxied request pools nothing; key the pool by proxy if that path matters.
fn stream_agent(proxy: Option<&ureq::Proxy>) -> ureq::Agent {
    let builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(60));
    if let Some(proxy) = proxy {
        return builder.proxy(proxy.clone()).build();
    }
    let Ok(mut pooled) = POOLED.lock() else {
        return builder.build();
    };
    let agent = match pooled.take() {
        Some((agent, idle_since)) if idle_since.elapsed() < KEEP_ALIVE => agent,
        _ => builder.build(),
    };
    *pooled = Some((agent.clone(), std::time::Instant::now()));
    agent
}

fn pooled_idle_from_now() {
    if let Ok(mut pooled) = POOLED.lock()
        && let Some((_, idle_since)) = pooled.as_mut()
    {
        *idle_since = std::time::Instant::now();
    }
}

pub fn send_with_retry(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    proxy: Option<&ProxyConfig>,
    on_retry: &dyn Fn(Wait),
) -> Result<ureq::Response, String> {
    let policy = RetryPolicy::default();
    let started = std::time::Instant::now();
    let agent = stream_agent(proxy.and_then(|config| config.proxy_for(host_of(url))));
    let body = {
        let _span = yi_types::trace::span("ai.serialize");
        body.to_string()
    };
    let mut attempt: u32 = 0;
    loop {
        let mut request = agent
            .post(url)
            .set("accept", "application/json")
            .set("content-type", "application/json");
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let sending = yi_types::trace::span("ai.http_send")
            .arg("attempt", attempt)
            .arg("bytes", body.len());
        let sent = request.send_string(&body);
        drop(sending);
        match sent {
            Ok(response) => return Ok(response),
            Err(ureq::Error::Status(status, response)) => {
                let retryable = is_retryable_status(status)
                    && attempt < policy.max_attempts
                    && started.elapsed() < policy.max_total_wall;
                if !retryable {
                    let text = response.into_string().unwrap_or_default();
                    return Err(format!("HTTP {status}: {text}"));
                }
                let retry_after_ms = response
                    .header("retry-after-ms")
                    .and_then(|value| value.parse::<f64>().ok());
                let retry_after = response
                    .header("retry-after")
                    .and_then(|value| value.parse::<f64>().ok());
                let delay = retry_delay(attempt, retry_after_ms, retry_after, &policy);
                attempt = attempt.saturating_add(1);
                announce(on_retry, attempt, &policy, delay, format!("HTTP {status}"));
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
            }
            Err(error) => {
                if attempt < policy.max_attempts && started.elapsed() < policy.max_total_wall {
                    let delay = retry_delay(attempt, None, None, &policy);
                    attempt = attempt.saturating_add(1);
                    // Incident: ureq writes the URL first, so an 80-char cut kept it and lost why.
                    let text = error.to_string();
                    let url = match &error {
                        ureq::Error::Transport(transport) => {
                            transport.url().map(|url| format!("{url}: "))
                        }
                        ureq::Error::Status(..) => None,
                    };
                    let reason = url.and_then(|url| text.strip_prefix(&url)).unwrap_or(&text);
                    let cause: String = reason.chars().take(80).collect();
                    announce(on_retry, attempt, &policy, delay, cause);
                    if let Some(delay) = delay {
                        std::thread::sleep(delay);
                    }
                    continue;
                }
                return Err(error.to_string());
            }
        }
    }
}

fn announce(
    on_retry: &dyn Fn(Wait),
    attempt: u32,
    policy: &RetryPolicy,
    delay: Option<std::time::Duration>,
    cause: String,
) {
    on_retry(Wait::Retry {
        attempt,
        of: policy.max_attempts,
        delay_ms: delay.map_or(0, |delay| {
            u64::try_from(delay.as_millis()).unwrap_or(u64::MAX)
        }),
        cause,
    });
}

pub fn pump_sse(
    response: ureq::Response,
    stop: Option<&std::sync::atomic::AtomicBool>,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let mut reader = response.into_reader();
    let mut decoder = crate::sse::SseDecoder::default();
    let mut buffer = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    // the loop's cut is read between events; dropping the reader closes the connection (D163).
    let cut = || stop.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
    loop {
        if cut() {
            return Ok(());
        }
        let read =
            std::io::Read::read(&mut reader, &mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(buffer.get(..read).unwrap_or_default());
        let Ok(chunk) = std::str::from_utf8(&pending) else {
            continue;
        };
        let chunk = chunk.to_owned();
        pending.clear();
        for event in decoder.feed(&chunk) {
            if cut() || !on_event(event)? {
                return Ok(());
            }
        }
    }
    for event in decoder.finish() {
        if !on_event(event)? {
            return Ok(());
        }
    }
    Ok(())
}

/// a body that died before its first event is sent once more; the first error returns (D146).
pub fn pump_sse_with_resend(
    stop: Option<&std::sync::atomic::AtomicBool>,
    send: impl Fn() -> Result<ureq::Response, String>,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, String>,
) -> Result<Option<String>, String> {
    let mut first_error: Option<String> = None;
    loop {
        let sent_us = yi_types::trace::now_us();
        let response = send()?;
        let mut delivered = false;
        let pumped = pump_sse(response, stop, |event| {
            if !delivered {
                yi_types::trace::complete("ai.first_sse_event", sent_us, serde_json::Map::new());
            }
            delivered = true;
            let _span = yi_types::trace::span("ai.sse_event");
            on_event(event)
        });
        match pumped {
            Ok(()) => {
                pooled_idle_from_now();
                return Ok(first_error);
            }
            // Incident: `Bad address (os error 14)` killed a trial's third request
            // before any byte arrived (ledger 0017, issue #256).
            Err(text) if resend_dead_stream(delivered, first_error.is_some()) => {
                first_error = Some(text);
            }
            Err(text) => {
                return Err(match first_error {
                    Some(first) => format!("{text} (resent once after: {first})"),
                    None => text,
                });
            }
        }
    }
}

pub fn resend_dead_stream(delivered: bool, resent: bool) -> bool {
    !delivered && !resent
}

pub fn note_resend(output: &mut AgentMessage, first_error: &str) {
    if let AgentMessage::Assistant {
        diagnostics,
        timestamp,
        ..
    } = output
    {
        let mut details = serde_json::Map::new();
        details.insert("resends".to_owned(), Value::from(1u32));
        diagnostics.get_or_insert_with(Vec::new).push(
            yi_types::message::AssistantMessageDiagnostic {
                diagnostic_type: "stream_resent".to_owned(),
                timestamp: *timestamp,
                error: Some(yi_types::message::DiagnosticErrorInfo {
                    name: None,
                    message: first_error.to_owned(),
                    stack: None,
                    code: None,
                }),
                details: Some(details),
            },
        );
    }
}

/// The upstream an OpenRouter turn ran on, kept once: every chunk names it, and so does the record.
pub fn note_upstream(output: &mut AgentMessage, upstream: &str) {
    if let AgentMessage::Assistant {
        diagnostics,
        timestamp,
        ..
    } = output
    {
        let notes = diagnostics.get_or_insert_with(Vec::new);
        if notes.iter().any(|note| note.diagnostic_type == "upstream") {
            return;
        }
        let mut details = serde_json::Map::new();
        details.insert("provider".to_owned(), Value::from(upstream));
        notes.push(yi_types::message::AssistantMessageDiagnostic {
            diagnostic_type: "upstream".to_owned(),
            timestamp: *timestamp,
            error: None,
            details: Some(details),
        });
    }
}

pub fn fail_message(output: &mut AgentMessage, text: &str) -> crate::EventOut {
    if let AgentMessage::Assistant {
        stop_reason,
        error_message,
        ..
    } = output
    {
        *stop_reason = StopReason::Error;
        *error_message = Some(text.to_owned());
    }
    crate::EventOut::Error {
        reason: StopReason::Error,
        error: output.clone(),
    }
}

pub fn terminal_event(output: AgentMessage) -> crate::EventOut {
    let stop_reason = match &output {
        AgentMessage::Assistant { stop_reason, .. } => *stop_reason,
        _ => StopReason::Error,
    };
    if stop_reason == StopReason::Error || stop_reason == StopReason::Aborted {
        return crate::EventOut::Error {
            reason: stop_reason,
            error: output,
        };
    }
    crate::EventOut::Done {
        reason: stop_reason,
        message: output,
    }
}

/// Invariant: a `~/.yi/catalog/<provider>.json` name replaces the adapter's own header
/// case-insensitively; empty removes it, since a gateway retarget must drop `x-api-key`.
pub fn headers_for(model: &Model, base: Vec<(&str, String)>) -> Vec<(String, String)> {
    let merged: Vec<(String, String)> = base
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    let overlay = model
        .headers
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())));
    merge_headers(merged, overlay)
}

/// The one merge the wire has: a catalog overlay (D170) and a login profile's
/// `stream_headers` (D191) both arrive here, so both obey the same replace/remove rule.
pub fn merge_headers(
    mut merged: Vec<(String, String)>,
    overlay: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    for (name, value) in overlay {
        let at = merged
            .iter()
            .position(|(existing, _)| existing.eq_ignore_ascii_case(&name));
        match (at, value.is_empty()) {
            (Some(at), true) => drop(merged.remove(at)),
            (Some(at), false) => {
                if let Some(slot) = merged.get_mut(at) {
                    slot.1 = value;
                }
            }
            (None, true) => {}
            (None, false) => merged.push((name, value)),
        }
    }
    merged
}

pub fn openai_bearer_post(
    url: &str,
    model: &Model,
    api_key: &str,
    body: &Value,
    proxy: Option<&ProxyConfig>,
    extra: &[(String, String)],
    on_retry: &dyn Fn(Wait),
) -> Result<ureq::Response, String> {
    let headers = headers_for(model, vec![("authorization", format!("Bearer {api_key}"))]);
    send_with_retry(
        url,
        &merge_headers(headers, extra.to_vec()),
        body,
        proxy,
        on_retry,
    )
}

/// What one request needs beside its body: the key, the proxy, and the loop's cut flag.
#[derive(Clone, Copy)]
pub struct Wire<'a> {
    pub api_key: &'a str,
    pub proxy: Option<&'a ProxyConfig>,
    pub stop: Option<&'a std::sync::atomic::AtomicBool>,
    /// The login profile's `stream_headers`, and anything else only the live
    /// credential knows: never static model data, so the catalog cannot hold it.
    pub extra: &'a [(String, String)],
    /// A stored OAuth credential is on the wire (D191): a 401 then names the login
    /// verb instead of dumping the provider's body.
    pub oauth: bool,
}

/// A 401 with a stored OAuth credential means the login was rejected or expired past
/// repair; the provider's body dump helps nobody. Name the verb that fixes it.
pub fn auth_hint(message: &str, oauth: bool, provider: &str) -> String {
    if oauth && message.starts_with("HTTP 401") {
        format!("{message} — credential rejected; run: yi login {provider}")
    } else {
        message.to_owned()
    }
}

/// The owned half of a [`Wire`]: what the spawned thread must hold for the request.
pub struct WireOwned {
    pub api_key: String,
    pub proxy: Option<ProxyConfig>,
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub extra: Vec<(String, String)>,
    pub oauth: bool,
}

/// The OpenAI-style stream shape, shared by the completions and responses paths: one
/// blocking request on a spawned thread, its failure mapped to a terminal event.
pub fn spawn_stream(
    fail: impl FnOnce(&Model, &str) -> crate::EventOut + Send + 'static,
    model: &Model,
    body: Value,
    owned: WireOwned,
    run: impl FnOnce(&Model, &Value, Wire<'_>, &Sender<crate::EventOut>) -> Result<(), String>
    + Send
    + 'static,
) -> Receiver<crate::EventOut> {
    let model = model.clone();
    spawn_provider_stream(move |sender| {
        let wire = Wire {
            api_key: &owned.api_key,
            proxy: owned.proxy.as_ref(),
            stop: owned.stop.as_deref(),
            extra: &owned.extra,
            oauth: owned.oauth,
        };
        if let Err(message) = run(&model, &body, wire, sender) {
            let _ = sender.blocking_send(fail(&model, &message));
        }
    })
}

pub fn spawn_provider_stream(
    run: impl FnOnce(&Sender<crate::EventOut>) + Send + 'static,
) -> Receiver<crate::EventOut> {
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    tokio::task::spawn_blocking(move || run(&sender));
    receiver
}

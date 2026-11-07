use serde_json::Value;
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::{AssistantMessageEvent, Wait};
use yi_types::message::{AgentMessage, Content, StopReason, Usage};
use yi_types::model::Model;
use yi_types::telemetry::{QUOTA_STAMP, REFUSED_STAMP};

use crate::retry::{RetryPolicy, is_retryable_status, quota_exhausted, server_wait, wait_for};
use crate::sse::SseEvent;
mod reply;

pub use reply::{Stripped, note_reply, note_resend, note_stripped, note_upstream, record_stripped};

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

pub(crate) fn assistant_content_mut(message: &mut AgentMessage) -> Option<&mut Vec<Content>> {
    match message {
        AgentMessage::Assistant { content, .. } => Some(content),
        _ => None,
    }
}

pub(crate) fn assistant_block_mut(
    message: &mut AgentMessage,
    index: usize,
) -> Option<&mut Content> {
    assistant_content_mut(message)?.get_mut(index)
}

/// A JSON object whose values move in; `json!` copies every interpolated value, an image too.
pub(crate) fn object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

pub(crate) fn image_url(data: &str, mime_type: &str) -> Value {
    object([
        ("type", serde_json::json!("image_url")),
        (
            "image_url",
            object([(
                "url",
                Value::String(format!("data:{mime_type};base64,{data}")),
            )]),
        ),
    ])
}

pub(crate) fn push_assistant_block(message: &mut AgentMessage, block: Content) -> usize {
    let Some(content) = assistant_content_mut(message) else {
        return 0;
    };
    content.push(block);
    content.len().saturating_sub(1)
}

pub(crate) fn content_end_events(
    output: &mut AgentMessage,
    mut include_tool: impl FnMut(usize) -> bool,
) -> Vec<AssistantMessageEvent> {
    assistant_content_mut(output)
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(content_index, block)| match block {
            Content::Text { text, .. } => Some(AssistantMessageEvent::TextEnd {
                content_index,
                content: text.clone(),
            }),
            Content::Thinking { thinking, .. } => Some(AssistantMessageEvent::ThinkingEnd {
                content_index,
                content: thinking.clone(),
            }),
            Content::ToolCall { .. } if include_tool(content_index) => {
                Some(AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call: block.clone(),
                })
            }
            Content::ToolCall { .. } | Content::Image { .. } => None,
        })
        .collect()
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
            no_proxy: yi_types::settings::comma_list(no_proxy.unwrap_or_default())
                .iter()
                .map(|entry| entry.trim_start_matches('.').to_lowercase())
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

pub const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn agent(url: &str, proxy: Option<&ProxyConfig>, read_timeout: std::time::Duration) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .user_agent(yi_types::USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(read_timeout)
        .timeout_write(read_timeout);
    if let Some(proxy) = proxy.and_then(|config| config.proxy_for(host_of(url))) {
        builder = builder.proxy(proxy.clone());
    }
    builder.build()
}

/// Invariant: both retry layers count into one place, so the notice names the requests that ran.
pub struct Retry<'a> {
    deadline: std::time::Duration,
    attempts: std::sync::atomic::AtomicU32,
    stop: Option<&'a std::sync::atomic::AtomicBool>,
    on_wait: &'a dyn Fn(Wait),
}

impl<'a> Retry<'a> {
    pub fn new(
        deadline: std::time::Duration,
        stop: Option<&'a std::sync::atomic::AtomicBool>,
        on_wait: &'a dyn Fn(Wait),
    ) -> Self {
        Self {
            deadline,
            attempts: std::sync::atomic::AtomicU32::new(0),
            stop,
            on_wait,
        }
    }

    pub fn ran(&self) -> u32 {
        self.attempts.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn bump(&self) {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Incident: ureq words the connect and read deadlines alike, and only the read one is the
/// gateway that never sends its first token.
fn read_timed_out(text: &str) -> bool {
    text.contains("timed out reading response")
}

/// Invariant: a stall names the field that fixes it and the wait that ran, because the retry
/// layers used to spend the deadline twice on a gateway that had already proved silent.
fn read_timeout_notice(read_timeout: std::time::Duration, attempts: u32) -> String {
    let ran = match attempts {
        1 => "1 attempt ran".to_owned(),
        other => format!("{other} attempts ran"),
    };
    format!(
        "{} {} s (the model's readTimeoutSecs, default {}); \
         raise readTimeoutSecs on the model in the catalog entry; {ran}, not retried",
        yi_types::telemetry::READ_DEADLINE_SPENT,
        read_timeout.as_secs(),
        yi_types::model::DEFAULT_READ_TIMEOUT_SECS
    )
}

fn read_capped(response: ureq::Response, url: &str, cap: usize) -> Result<Value, String> {
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

/// One GET with a byte cap, no retry: a catalog refresh can wait for the next session.
pub fn get_json(
    url: &str,
    headers: &[(&str, String)],
    proxy: Option<&ProxyConfig>,
    cap: usize,
) -> Result<Value, String> {
    let mut request = agent(url, proxy, DEFAULT_READ_TIMEOUT)
        .get(url)
        .set("accept", "application/json");
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request.call().map_err(|error| format!("{url}: {error}"))?;
    read_capped(response, url, cap)
}

pub fn post_json(
    url: &str,
    headers: &[(&str, String)],
    body: &str,
    proxy: Option<&ProxyConfig>,
    cap: usize,
    read_timeout: std::time::Duration,
) -> Result<Value, PostJsonError> {
    let mut request = agent(url, proxy, read_timeout)
        .post(url)
        .set("accept", "application/json")
        .set("content-type", "application/json");
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request.send_string(body).map_err(|error| match error {
        ureq::Error::Status(status, _) => PostJsonError::Http { status },
        ureq::Error::Transport(error) => PostJsonError::Transport(format!("{url}: {error}")),
    })?;
    if !(200..300).contains(&response.status()) {
        return Err(PostJsonError::Http {
            status: response.status(),
        });
    }
    read_capped(response, url, cap).map_err(PostJsonError::Protocol)
}

fn http_status_message(status: u16) -> &'static str {
    match status {
        401 | 403 => "the key was refused",
        402 => "out of credits",
        404 => "the model or engine was not found",
        408 | 429 => "rate limited or timed out; retry later",
        400..=499 => "the request was refused",
        500..=599 => "the provider failed; retry later",
        _ => "unexpected status",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PostJsonError {
    #[error("{0}")]
    Transport(String),
    #[error("HTTP {status}: {}", http_status_message(*status))]
    Http { status: u16 },
    #[error("{0}")]
    Protocol(String),
}

pub fn waiting(sender: &Sender<AssistantMessageEvent>) -> impl Fn(Wait) + '_ {
    move |wait| {
        let _ = sender.blocking_send(AssistantMessageEvent::Waiting { wait });
    }
}

/// Past this a pooled connection is not trusted: a NAT may have dropped it without a FIN.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(30);

static POOLED: std::sync::Mutex<Vec<Pooled>> = std::sync::Mutex::new(Vec::new());

/// A deadline is built into the agent, so a pool keyed by nothing but idle time handed a fast
/// model's connection the slow model's bound. One slot thrashed two models in one session.
struct Pooled {
    agent: ureq::Agent,
    idle_since: std::time::Instant,
    read_timeout: std::time::Duration,
}

// ponytail: a proxied request pools nothing; key the pool by proxy if that path matters.
fn stream_agent(proxy: Option<&ureq::Proxy>, read_timeout: std::time::Duration) -> ureq::Agent {
    let builder = ureq::AgentBuilder::new()
        .user_agent(yi_types::USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(read_timeout)
        // ureq keeps one idle connection per host by default, so a later turn of the session
        // dialed again: the family's eight live streams is the width worth keeping.
        .max_idle_connections(16)
        .max_idle_connections_per_host(16);
    if let Some(proxy) = proxy {
        return builder.proxy(proxy.clone()).build();
    }
    let Ok(mut pooled) = POOLED.lock() else {
        return builder.build();
    };
    pooled.retain(|held| held.idle_since.elapsed() < KEEP_ALIVE);
    if let Some(agent) = pooled
        .iter_mut()
        .find(|held| held.read_timeout == read_timeout)
        .map(|held| {
            held.idle_since = std::time::Instant::now();
            held.agent.clone()
        })
    {
        return agent;
    }
    let agent = builder.build();
    if pooled.len() >= POOLED_SLOTS
        && let Some((at, _)) = pooled
            .iter()
            .enumerate()
            .min_by_key(|(_, held)| held.idle_since)
    {
        drop(pooled.swap_remove(at));
    }
    pooled.push(Pooled {
        agent: agent.clone(),
        idle_since: std::time::Instant::now(),
        read_timeout,
    });
    agent
}

/// Two deadlines at once is the alternating main/advisor pattern; a third covers a subagent.
const POOLED_SLOTS: usize = 3;

fn pooled_idle_from_now(read_timeout: std::time::Duration) {
    if let Ok(mut pooled) = POOLED.lock()
        && let Some(held) = pooled
            .iter_mut()
            .find(|held| held.read_timeout == read_timeout)
    {
        held.idle_since = std::time::Instant::now();
    }
}

/// The cache fields a 400 may name, the specific ones before the generic `cache`.
const CACHE_FIELDS: [&str; 6] = [
    "prompt_cache_retention",
    "prompt_cache_key",
    "cache_control",
    "cache_ttl",
    "session_id",
    "cache",
];

fn cache_field(text: &str) -> Option<&'static str> {
    CACHE_FIELDS.into_iter().find(|field| text.contains(field))
}

pub fn send_with_retry(
    url: &str,
    headers: &[(String, String)],
    body: &crate::breakpoints::Encoded,
    proxy: Option<&ProxyConfig>,
    retry: &Retry<'_>,
) -> Result<ureq::Response, String> {
    send_stripping_cache(url, headers, body, proxy, retry, &Stripped::default())
}

/// One POST, retried by the policy; a cache-field 400 is asked again without the fields.
pub fn send_stripping_cache(
    url: &str,
    headers: &[(String, String)],
    body: &crate::breakpoints::Encoded,
    proxy: Option<&ProxyConfig>,
    retry: &Retry<'_>,
    stripped: &Stripped,
) -> Result<ureq::Response, String> {
    let policy = RetryPolicy::default();
    let started = std::time::Instant::now();
    let agent = stream_agent(
        proxy.and_then(|config| config.proxy_for(host_of(url))),
        retry.deadline,
    );
    let mut bare: Option<crate::breakpoints::Encoded> = None;
    let mut serialized = {
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
            .arg("bytes", serialized.len());
        retry.bump();
        let sent = request.send_string(&serialized);
        drop(sending);
        match sent {
            Ok(response) => return Ok(response),
            Err(ureq::Error::Status(status, response)) => {
                let retry_after_ms = response
                    .header("retry-after-ms")
                    .and_then(|value| value.parse::<f64>().ok());
                let retry_after = response
                    .header("retry-after")
                    .and_then(|value| value.parse::<f64>().ok());
                let asked = match response.header("x-should-retry") {
                    Some(value) if value.eq_ignore_ascii_case("true") => Some(true),
                    Some(value) if value.eq_ignore_ascii_case("false") => Some(false),
                    _ => None,
                };
                let text = response.into_string().unwrap_or_default();
                if status == 400
                    && bare.is_none()
                    && let Some(field) = cache_field(&text)
                {
                    stripped.set(field);
                    bare = Some(body.without(&CACHE_FIELDS));
                    if let Some(bare) = &bare {
                        serialized = {
                            let _span = yi_types::trace::span("ai.serialize");
                            bare.to_string()
                        };
                    }
                    announce(
                        retry.on_wait,
                        attempt.saturating_add(1),
                        &policy,
                        None,
                        format!("HTTP 400 named {field}"),
                    );
                    continue;
                }
                if quota_exhausted(status, &text, url) {
                    return Err(format!("HTTP {status} {QUOTA_STAMP} {text}"));
                }
                let retryable = asked.unwrap_or_else(|| is_retryable_status(status))
                    && attempt < policy.max_attempts
                    && started.elapsed() < policy.max_total_wall;
                if asked == Some(false) {
                    return Err(format!(
                        "HTTP {status} {REFUSED_STAMP} x-should-retry: false: {text}"
                    ));
                }
                if !retryable {
                    return Err(format!("HTTP {status}: {text}"));
                }
                let delay = match wait_for(
                    attempt,
                    server_wait(retry_after_ms, retry_after),
                    &policy,
                ) {
                    Ok(delay) => delay,
                    Err(wait) => {
                        return Err(format!(
                            "HTTP {status} {REFUSED_STAMP} the provider asked to wait {wait:?}, past the {:?} retry cap: {text}",
                            policy.max_delay
                        ));
                    }
                };
                attempt = attempt.saturating_add(1);
                announce(
                    retry.on_wait,
                    attempt,
                    &policy,
                    Some(delay),
                    format!("HTTP {status}"),
                );
                if !sleep_in_slices(delay, retry.stop) {
                    return Err(format!(
                        "HTTP {status}: the turn was cut during the {delay:?} retry wait"
                    ));
                }
            }
            Err(error) => {
                // Incident: ureq writes the URL first, so an 80-char cut kept it and lost why.
                let reason = match &error {
                    ureq::Error::Transport(transport) => transport_cause(transport),
                    ureq::Error::Status(..) => error.to_string(),
                };
                if read_timed_out(&reason) {
                    return Err(read_timeout_notice(retry.deadline, retry.ran()));
                }
                if attempt < policy.max_attempts && started.elapsed() < policy.max_total_wall {
                    let delay = crate::retry::retry_delay(attempt);
                    attempt = attempt.saturating_add(1);
                    let cause: String = reason.chars().take(80).collect();
                    announce(retry.on_wait, attempt, &policy, Some(delay), cause);
                    if !sleep_in_slices(delay, retry.stop) {
                        return Err(format!("the turn was cut during the {delay:?} retry wait"));
                    }
                    continue;
                }
                return Err(error.to_string());
            }
        }
    }
}

/// A backoff slept in one call cannot be interrupted: a cut then waits the whole wait out and
/// sends the request the loop no longer wants. Slices end it within 100 ms of the cut.
fn sleep_in_slices(
    total: std::time::Duration,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    const SLICE: std::time::Duration = std::time::Duration::from_millis(100);
    let cut = || stop.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
    let mut left = total;
    while !left.is_zero() {
        if cut() {
            return false;
        }
        let step = left.min(SLICE);
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
    !cut()
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
    let mut consumed: u64 = 0;
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
        // Incident: a chunk boundary inside a multi-byte character, or an invalid byte, was skipped
        // and the loop spun on a buffer that could never decode, stalling to the 60 s read timeout.
        let (valid, invalid) = match std::str::from_utf8(&pending) {
            Ok(_) => (pending.len(), None),
            Err(error) => (
                error.valid_up_to(),
                error.error_len().map(|_| error.valid_up_to()),
            ),
        };
        if let Some(at) = invalid {
            return Err(format!(
                "the stream carried an invalid UTF-8 byte at offset {}",
                consumed.saturating_add(u64::try_from(at).unwrap_or(u64::MAX))
            ));
        }
        let chunk = pending
            .get(..valid)
            .and_then(|head| std::str::from_utf8(head).ok())
            .unwrap_or_default()
            .to_owned();
        pending.drain(..valid);
        consumed = consumed.saturating_add(u64::try_from(valid).unwrap_or(u64::MAX));
        for event in decoder.feed(&chunk) {
            if cut() {
                return Ok(());
            }
            if !on_event(event)? {
                drain_to_eof(reader);
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

/// Incident: `response.completed` ends the Responses stream before the body's terminator, so the
/// dropped reader never reached the pool. Reading on for 2 s hands the connection back to ureq.
fn drain_to_eof(reader: Box<dyn std::io::Read + Send + Sync + 'static>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const DRAIN: std::time::Duration = std::time::Duration::from_secs(2);
    const MAX_DRAINS: usize = 16;
    static DRAINING: AtomicUsize = AtomicUsize::new(0);
    // Incident: a read parks its thread on ureq's 60 s socket timeout past the 2 s wait, so a wide
    // swarm leaked a thread and socket per turn; past MAX_DRAINS the connection is dropped unpooled.
    if DRAINING.fetch_add(1, Ordering::Relaxed) >= MAX_DRAINS {
        DRAINING.fetch_sub(1, Ordering::Relaxed);
        return;
    }
    let (done, drained) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("yi-stream-drain".to_owned())
        .spawn(move || {
            let mut reader = reader;
            let deadline = std::time::Instant::now() + DRAIN;
            let mut sink = [0u8; 8192];
            while std::time::Instant::now() < deadline {
                match std::io::Read::read(&mut reader, &mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            drop(reader);
            DRAINING.fetch_sub(1, Ordering::Relaxed);
            let _ = done.send(());
        });
    match spawned {
        Ok(_) => {
            let _ = drained.recv_timeout(DRAIN);
        }
        Err(_) => {
            DRAINING.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// a body that died before its first event is sent once more; the first error returns (D146).
pub fn pump_sse_with_resend(
    stop: Option<&std::sync::atomic::AtomicBool>,
    retry: &Retry<'_>,
    send: impl Fn() -> Result<ureq::Response, String>,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, String>,
) -> Result<Option<String>, String> {
    let mut first_error: Option<String> = None;
    loop {
        let sent_us = yi_types::trace::now_us();
        // A resend's refusal must not read as the first request's: that one may have billed.
        let response = send().map_err(|text| match &first_error {
            Some(first) => format!("resend refused: {text} (first attempt: {first})"),
            None => text,
        })?;
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
                pooled_idle_from_now(retry.deadline);
                return Ok(first_error);
            }
            // A silent gateway already spent the model's whole deadline: one more wait is longer
            // than the bound the field sets.
            Err(text) if read_timed_out(&text) => {
                let notice = read_timeout_notice(retry.deadline, retry.ran());
                return Err(match first_error {
                    Some(first) => format!("{notice} (resent once after: {first})"),
                    None => notice,
                });
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

pub fn fail_message(output: &mut AgentMessage, text: &str) -> crate::EventOut {
    if let AgentMessage::Assistant {
        stop_reason,
        error_message,
        usage,
        ..
    } = output
    {
        // A 4xx from `send_with_retry` refused the request before it ran: a known zero. A 5xx
        // may come from a gateway after the upstream billed, and a cut stream may have billed.
        if text.starts_with("HTTP 4") {
            *usage = Usage::zero();
        }
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
    body: &crate::breakpoints::Encoded,
    wire: Wire<'_>,
    retry: &Retry<'_>,
    stripped: &Stripped,
) -> Result<ureq::Response, String> {
    let headers = headers_for(
        model,
        vec![("authorization", format!("Bearer {}", wire.api_key))],
    );
    send_stripping_cache(
        url,
        &merge_headers(headers, wire.extra.to_vec()),
        body,
        wire.proxy,
        retry,
        stripped,
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
    pub settle: Option<&'a crate::settle::Settle>,
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
    pub settle: Option<crate::settle::Settle>,
}

/// The OpenAI-style stream shape, shared by the completions and responses paths: one
/// blocking request on a spawned thread, its failure mapped to a terminal event.
pub fn spawn_stream(
    fail: impl FnOnce(&Model, &str) -> crate::EventOut + Send + 'static,
    model: &Model,
    body: crate::breakpoints::Encoded,
    owned: WireOwned,
    run: impl FnOnce(
        &Model,
        &crate::breakpoints::Encoded,
        Wire<'_>,
        &Sender<crate::EventOut>,
    ) -> Result<(), String>
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
            settle: owned.settle.as_ref(),
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

/// A transport failure's kind, message and source, without the URL ureq's own rendering leads with.
pub(crate) fn transport_cause(transport: &ureq::Transport) -> String {
    let message = transport
        .message()
        .map(|message| format!(": {message}"))
        .unwrap_or_default();
    let source = std::error::Error::source(transport)
        .map(|source| format!(": {source}"))
        .unwrap_or_default();
    format!("{}{message}{source}", transport.kind())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use serde_json::json;

    #[test]
    fn a_retry_reuses_the_serialized_request_body() -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            for status in [
                "HTTP/1.1 429 Too Many Requests\r\nretry-after-ms: 0\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept()?;
                let mut request = [0; 4096];
                let _ = stream.read(&mut request)?;
                stream.write_all(status.as_bytes())?;
            }
            Ok(())
        });
        let body = crate::breakpoints::Encoded::provider_prefix(
            json!({"messages": []}),
            "messages",
            Vec::new(),
        );
        crate::breakpoints::reset_serializations();
        let quiet = |_| {};
        let retry = super::Retry::new(std::time::Duration::from_secs(60), None, &quiet);
        let response = super::send_with_retry(
            &format!("http://127.0.0.1:{port}/v1/messages"),
            &[],
            &body,
            None,
            &retry,
        )
        .map_err(|error| format!("request failed: {error}"))?;
        let _ = response.into_string();
        server.join().map_err(|_| "server thread panicked")??;
        assert_eq!(crate::breakpoints::serializations(), 1);
        Ok(())
    }
}

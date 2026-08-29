use serde_json::Value;
use tokio::sync::mpsc::{Receiver, Sender};
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
        usage: Usage::zero(),
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

/// Invariant: a configured proxy is applied or startup fails — degrading to a
/// direct connection in an air-gapped runner is an unattributable hang (A4/E2).
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

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split_once('/').map_or(rest, |(host, _)| host);
    rest.split_once(':').map_or(rest, |(host, _)| host)
}

pub fn send_with_retry(
    url: &str,
    headers: &[(&str, String)],
    body: &Value,
    proxy: Option<&ProxyConfig>,
) -> Result<ureq::Response, String> {
    let policy = RetryPolicy::default();
    let started = std::time::Instant::now();
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(60));
    if let Some(proxy) = proxy.and_then(|config| config.proxy_for(host_of(url))) {
        builder = builder.proxy(proxy.clone());
    }
    let agent = builder.build();
    let mut attempt: u32 = 0;
    loop {
        let mut request = agent
            .post(url)
            .set("accept", "application/json")
            .set("content-type", "application/json");
        for (name, value) in headers {
            request = request.set(name, value);
        }
        match request.send_string(&body.to_string()) {
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
                if let Some(delay) = retry_delay(attempt, retry_after_ms, retry_after, &policy) {
                    std::thread::sleep(delay);
                }
                attempt = attempt.saturating_add(1);
            }
            Err(error) => {
                if attempt < policy.max_attempts && started.elapsed() < policy.max_total_wall {
                    if let Some(delay) = retry_delay(attempt, None, None, &policy) {
                        std::thread::sleep(delay);
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                return Err(error.to_string());
            }
        }
    }
}

pub fn pump_sse(
    response: ureq::Response,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let mut reader = response.into_reader();
    let mut decoder = crate::sse::SseDecoder::default();
    let mut buffer = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
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
            if !on_event(event)? {
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

pub fn openai_bearer_post(
    url: &str,
    api_key: &str,
    body: &Value,
    proxy: Option<&ProxyConfig>,
) -> Result<ureq::Response, String> {
    send_with_retry(
        url,
        &[("authorization", format!("Bearer {api_key}"))],
        body,
        proxy,
    )
}

pub fn spawn_provider_stream(
    run: impl FnOnce(&Sender<crate::EventOut>) + Send + 'static,
) -> Receiver<crate::EventOut> {
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    tokio::task::spawn_blocking(move || run(&sender));
    receiver
}

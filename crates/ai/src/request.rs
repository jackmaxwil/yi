use serde_json::Value;
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

pub fn send_with_retry(
    url: &str,
    headers: &[(&str, String)],
    body: &Value,
) -> Result<ureq::Response, String> {
    let policy = RetryPolicy::default();
    let started = std::time::Instant::now();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(60))
        .build();
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
    mut on_event: impl FnMut(SseEvent) -> Result<(), String>,
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
            on_event(event)?;
        }
    }
    for event in decoder.finish() {
        on_event(event)?;
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

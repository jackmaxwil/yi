//! The one error a rerun cannot duplicate: a stream that died before it showed anything.

use yi_types::message::{AgentMessage, Content};

pub const STREAM_RETRY_CUSTOM_TYPE: &str = "stream_retry";
const STREAM_RETRY_TEXT: &str = "The provider dropped the stream before any output reached the transcript; the same turn runs again.";
pub const STREAM_RETRY_AT: u32 = 1;

/// A stream that was generating, died on the wire or ended on an in-band error chunk, and
/// showed nothing; a synthesized error with zero usage never went to a provider.
pub fn nothing_delivered(message: &AgentMessage) -> bool {
    let AgentMessage::Assistant {
        content,
        usage,
        error_message,
        raw_stop_reason,
        ..
    } = message
    else {
        return false;
    };
    let generating = usage.output > 0 || usage.reasoning.unwrap_or(0) > 0;
    let wire = error_message.as_deref().is_some_and(|text| {
        yi_types::telemetry::ErrorClass::from_provider_text(text).is_wire_refusal()
    });
    // Incident: Wafer's in-band 502 had no usage and no transport class; yi exited 0 (D175)
    let in_band = raw_stop_reason.as_deref() == Some(yi_types::message::RAW_STOP_IN_BAND_ERROR);
    (generating || wire || in_band)
        && !(yi_types::message::has_text(content)
            || content
                .iter()
                .any(|block| matches!(block, Content::ToolCall { .. })))
}

pub fn answered(message: &AgentMessage) -> bool {
    let AgentMessage::Assistant { content, .. } = message else {
        return false;
    };
    yi_types::message::has_text(content)
}

/// A dropped stream that showed nothing is the one error a rerun cannot duplicate.
pub fn stream_retry(error: Option<&str>) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: STREAM_RETRY_CUSTOM_TYPE.to_owned(),
        content: yi_types::message::UserContent::Text(STREAM_RETRY_TEXT.to_owned()),
        display: false,
        details: Some(serde_json::json!({"error": error})),
        timestamp: 0,
    }
}

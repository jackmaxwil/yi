use std::io::{BufRead, BufReader, Read};

use serde_json::Value;

const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
// Incident: rmcp's DEFAULT_MAX_SSE_EVENT_SIZE, carried across D71 — an
// unbounded SSE event is a server-controlled allocation.
const MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;
const EVENT_STREAM_MIME: &str = "text/event-stream";
const JSON_MIME: &str = "application/json";

// Invariant: [`crate::run`] matches this substring to turn a 401 into the
// login hint, so [`HttpError::AuthRequired`] must render it. A test pins both.
pub const UNAUTHORIZED_MARKER: &str = "unauthorized (status 401)";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("unexpected status {0}")]
    Status(u16),
    #[error("unauthorized (status 401)")]
    AuthRequired,
    #[error("unexpected content type {0}")]
    ContentType(String),
    #[error("stream ended before the reply arrived")]
    UnexpectedEnd,
}

/// Whether a POST expects a reply. A notification is answered with 202 and no
/// body, so waiting for a matching id would hang.
pub enum PostBody {
    Request(u64),
    Notification,
}

/// MCP streamable HTTP over the tree's one HTTP stack, so reqwest stays banned.
pub struct HttpTransport {
    agent: ureq::Agent,
    url: String,
    auth_token: Option<String>,
    session_id: Option<String>,
}

impl HttpTransport {
    pub fn new(url: &str, auth_token: Option<String>) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().build(),
            url: url.to_owned(),
            auth_token,
            session_id: None,
        }
    }

    fn post(&self, body: &str) -> Result<ureq::Response, HttpError> {
        let mut request = self
            .agent
            .post(&self.url)
            .set("content-type", JSON_MIME)
            .set("accept", &format!("{EVENT_STREAM_MIME}, {JSON_MIME}"));
        if let Some(token) = &self.auth_token {
            request = request.set("authorization", &format!("Bearer {token}"));
        }
        if let Some(session) = &self.session_id {
            request = request.set(HEADER_SESSION_ID, session);
        }
        match request.send_string(body) {
            Ok(response) => Ok(response),
            Err(ureq::Error::Status(401, _)) => Err(HttpError::AuthRequired),
            Err(ureq::Error::Status(code, _)) => Err(HttpError::Status(code)),
            Err(error) => Err(HttpError::Transport(error.to_string())),
        }
    }

    pub fn round_trip(&mut self, message: &Value, body: PostBody) -> Result<Value, String> {
        let payload = serde_json::to_string(message).map_err(|error| error.to_string())?;
        let response = self.post(&payload).map_err(|error| error.to_string())?;
        if self.session_id.is_none() {
            self.session_id = response.header(HEADER_SESSION_ID).map(str::to_owned);
        }
        let content_type = response.content_type().to_owned();
        let status = response.status();
        let PostBody::Request(id) = body else {
            return Ok(Value::Null);
        };
        if status == 202 {
            return Err("server accepted the request without replying".to_owned());
        }
        if content_type.starts_with(EVENT_STREAM_MIME) {
            return read_sse_reply(response.into_reader(), id).map_err(|error| error.to_string());
        }
        if content_type.starts_with(JSON_MIME) {
            let text = response
                .into_string()
                .map_err(|error| HttpError::Transport(error.to_string()).to_string())?;
            return serde_json::from_str::<Value>(&text).map_err(|error| error.to_string());
        }
        Err(HttpError::ContentType(content_type).to_string())
    }

    /// Best-effort session teardown, matching what rmcp's `cancel` did: a server
    /// that answers 405 never had a session to release.
    pub fn close(self) {
        let Some(session) = self.session_id else {
            return;
        };
        let mut request = self
            .agent
            .delete(&self.url)
            .set(HEADER_SESSION_ID, &session);
        if let Some(token) = self.auth_token {
            request = request.set("authorization", &format!("Bearer {token}"));
        }
        let _ = request.call();
    }
}

/// Reads SSE frames until one carries the JSON-RPC reply for `id`, discarding
/// the server-initiated notifications that may precede it.
fn read_sse_reply(reader: impl Read, id: u64) -> Result<Value, HttpError> {
    let mut lines = BufReader::new(reader).lines();
    let mut data: Vec<String> = Vec::new();
    let mut bytes: usize = 0;
    loop {
        match lines.next() {
            Some(Ok(line)) => {
                if line.is_empty() {
                    if let Some(reply) = frame_reply(&mut data, id) {
                        return Ok(reply);
                    }
                    bytes = 0;
                    continue;
                }
                if line.starts_with(':') {
                    continue;
                }
                bytes = bytes.saturating_add(line.len());
                if bytes > MAX_SSE_EVENT_SIZE {
                    return Err(HttpError::Transport(
                        "SSE event exceeds max size".to_owned(),
                    ));
                }
                if let Some(value) = field_value(&line, "data") {
                    data.push(value.to_owned());
                }
            }
            Some(Err(error)) => return Err(HttpError::Transport(error.to_string())),
            None => {
                if let Some(reply) = frame_reply(&mut data, id) {
                    return Ok(reply);
                }
                return Err(HttpError::UnexpectedEnd);
            }
        }
    }
}

fn field_value<'line>(line: &'line str, name: &str) -> Option<&'line str> {
    let (field, value) = line.split_once(':').unwrap_or((line, ""));
    (field == name).then(|| value.strip_prefix(' ').unwrap_or(value))
}

fn frame_reply(data: &mut Vec<String>, id: u64) -> Option<Value> {
    if data.is_empty() {
        return None;
    }
    let body = std::mem::take(data).join("\n");
    let parsed = serde_json::from_str::<Value>(&body).ok()?;
    (parsed.get("id").and_then(Value::as_u64) == Some(id)).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unauthorized_marker_is_what_auth_required_renders() {
        assert_eq!(HttpError::AuthRequired.to_string(), UNAUTHORIZED_MARKER);
    }

    #[test]
    fn a_notification_frame_before_the_reply_is_skipped() -> Result<(), Box<dyn std::error::Error>>
    {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n",
            "\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n",
            "\n",
        );
        let reply = read_sse_reply(body.as_bytes(), 2)?;
        assert_eq!(reply.get("id").and_then(Value::as_u64), Some(2));
        Ok(())
    }

    #[test]
    fn a_reply_split_across_data_lines_is_rejoined() -> Result<(), Box<dyn std::error::Error>> {
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\n",
            "data: \"result\":{\"ok\":true}}\n",
            "\n",
        );
        let reply = read_sse_reply(body.as_bytes(), 7)?;
        assert_eq!(
            reply.pointer("/result/ok").and_then(Value::as_bool),
            Some(true)
        );
        Ok(())
    }

    #[test]
    fn a_stream_that_ends_without_the_reply_is_an_error() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n\n";
        assert!(read_sse_reply(body.as_bytes(), 2).is_err());
    }
}

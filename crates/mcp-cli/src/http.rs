use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::Sse;

const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
// rmcp's DEFAULT_MAX_SSE_EVENT_SIZE (private): 16 MiB cap on one raw SSE event.
const MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;
const HEADER_LAST_EVENT_ID: &str = "Last-Event-Id";
const EVENT_STREAM_MIME: &str = "text/event-stream";
const JSON_MIME: &str = "application/json";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("unexpected status {0}")]
    Status(u16),
}

/// ureq-based implementation of rmcp's client transport trait (D37): the one
/// HTTP stack in the tree serves MCP too — reqwest stays banned. Blocking
/// calls run in spawn_blocking; SSE bodies are pumped by a detached reader
/// thread feeding a channel-backed stream.
#[derive(Clone)]
pub struct UreqHttpClient {
    agent: ureq::Agent,
}

impl Default for UreqHttpClient {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new().build(),
        }
    }
}

struct SseFields {
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    retry: Option<u64>,
    bytes: usize,
}

impl SseFields {
    fn new() -> Self {
        Self {
            event: None,
            data: Vec::new(),
            id: None,
            retry: None,
            bytes: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.event.is_none() && self.data.is_empty() && self.id.is_none() && self.retry.is_none()
    }

    fn into_sse(self) -> Sse {
        Sse {
            event: self.event,
            data: if self.data.is_empty() {
                None
            } else {
                Some(self.data.join("\n"))
            },
            id: self.id,
            retry: self.retry,
        }
    }
}

fn parse_sse_line(fields: &mut SseFields, line: &str) {
    let (name, value) = line
        .split_once(':')
        .map_or((line, ""), |(name, value)| (name, value));
    let value = value.strip_prefix(' ').unwrap_or(value);
    match name {
        "event" => fields.event = Some(value.to_owned()),
        "data" => fields.data.push(value.to_owned()),
        "id" => fields.id = Some(value.to_owned()),
        "retry" => fields.retry = value.parse().ok(),
        _ => {}
    }
}

/// Reads an SSE body on a detached thread, emitting one `Sse` per blank-line
/// boundary. Events over `max_event_size` raw bytes fail the stream closed.
fn sse_stream_from(reader: impl Read + Send + 'static, max_event_size: usize) -> BoxedSse {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Result<Sse, SseError>>();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        let mut fields = SseFields::new();
        loop {
            match lines.next() {
                Some(Ok(line)) => {
                    if line.is_empty() {
                        if !fields.is_empty() {
                            let done = sender
                                .send(Ok(
                                    std::mem::replace(&mut fields, SseFields::new()).into_sse()
                                ))
                                .is_err();
                            if done {
                                return;
                            }
                        }
                        continue;
                    }
                    if line.starts_with(':') {
                        continue;
                    }
                    fields.bytes = fields.bytes.saturating_add(line.len());
                    if fields.bytes > max_event_size {
                        let _ =
                            sender.send(Err(SseError::Body("SSE event exceeds max size".into())));
                        return;
                    }
                    parse_sse_line(&mut fields, &line);
                }
                Some(Err(_)) | None => {
                    if !fields.is_empty() {
                        let _ = sender.send(Ok(fields.into_sse()));
                    }
                    return;
                }
            }
        }
    });
    futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    })
    .boxed()
}

type BoxedSse = BoxStream<'static, Result<Sse, SseError>>;
type HttpResult<T> = Result<T, StreamableHttpError<HttpError>>;

struct BlockingResponse {
    status: u16,
    content_type: Option<String>,
    session_id: Option<String>,
    www_authenticate: Option<String>,
    response: Option<ureq::Response>,
}

fn run_blocking(request: ureq::Request, body: Option<String>) -> HttpResult<BlockingResponse> {
    let outcome = match body {
        Some(body) => request.send_string(&body),
        None => request.call(),
    };
    let response = match outcome {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            return Ok(BlockingResponse {
                status,
                content_type: Some(response.content_type().to_owned()),
                session_id: response.header(HEADER_SESSION_ID).map(str::to_owned),
                www_authenticate: response.header("www-authenticate").map(str::to_owned),
                response: None,
            });
        }
        Err(error) => {
            return Err(StreamableHttpError::Client(HttpError::Transport(
                error.to_string(),
            )));
        }
    };
    Ok(BlockingResponse {
        status: response.status(),
        content_type: Some(response.content_type().to_owned()),
        session_id: response.header(HEADER_SESSION_ID).map(str::to_owned),
        www_authenticate: None,
        response: Some(response),
    })
}

fn error_for_status(reply: &BlockingResponse) -> Option<StreamableHttpError<HttpError>> {
    match reply.status {
        401 => Some(StreamableHttpError::AuthRequired(AuthRequiredError::new(
            reply.www_authenticate.clone().unwrap_or_default(),
        ))),
        403 => Some(StreamableHttpError::InsufficientScope(
            InsufficientScopeError::new(reply.www_authenticate.clone().unwrap_or_default(), None),
        )),
        404 => Some(StreamableHttpError::SessionExpired),
        code if code >= 400 => Some(StreamableHttpError::Client(HttpError::Status(code))),
        _ => None,
    }
}

fn apply_headers(
    mut request: ureq::Request,
    auth_header: Option<&str>,
    session_id: Option<&str>,
    custom_headers: &HashMap<http::HeaderName, http::HeaderValue>,
) -> ureq::Request {
    if let Some(token) = auth_header {
        request = request.set("authorization", &format!("Bearer {token}"));
    }
    if let Some(session) = session_id {
        request = request.set(HEADER_SESSION_ID, session);
    }
    for (name, value) in custom_headers {
        if let Ok(text) = value.to_str() {
            request = request.set(name.as_str(), text);
        }
    }
    request
}

impl StreamableHttpClient for UreqHttpClient {
    type Error = HttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> HttpResult<StreamableHttpPostResponse> {
        let agent = self.agent.clone();
        let body = serde_json::to_string(&message)?;
        let reply = tokio::task::spawn_blocking(move || {
            let request = agent
                .post(uri.as_ref())
                .set("content-type", JSON_MIME)
                .set("accept", &format!("{EVENT_STREAM_MIME}, {JSON_MIME}"));
            let request = apply_headers(
                request,
                auth_header.as_deref(),
                session_id.as_deref(),
                &custom_headers,
            );
            run_blocking(request, Some(body))
        })
        .await??;
        if let Some(error) = error_for_status(&reply) {
            return Err(error);
        }
        if reply.status == 202 {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session_id = reply.session_id.clone();
        let content_type = reply.content_type.clone().unwrap_or_default();
        let Some(response) = reply.response else {
            return Err(StreamableHttpError::UnexpectedEndOfStream);
        };
        if content_type.starts_with(EVENT_STREAM_MIME) {
            let stream = sse_stream_from(response.into_reader(), MAX_SSE_EVENT_SIZE);
            return Ok(StreamableHttpPostResponse::Sse(stream, session_id));
        }
        if content_type.starts_with(JSON_MIME) {
            let text = tokio::task::spawn_blocking(move || response.into_string()).await?;
            let text = text.map_err(|error| {
                StreamableHttpError::Client(HttpError::Transport(error.to_string()))
            })?;
            let parsed: ServerJsonRpcMessage = serde_json::from_str(&text)?;
            return Ok(StreamableHttpPostResponse::Json(parsed, session_id));
        }
        Err(StreamableHttpError::UnexpectedContentType(Some(
            content_type,
        )))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> HttpResult<()> {
        let agent = self.agent.clone();
        let reply = tokio::task::spawn_blocking(move || {
            let request = agent.delete(uri.as_ref());
            let request = apply_headers(
                request,
                auth_header.as_deref(),
                Some(session_id.as_ref()),
                &custom_headers,
            );
            run_blocking(request, None)
        })
        .await??;
        if reply.status == 405 {
            return Err(StreamableHttpError::ServerDoesNotSupportDeleteSession);
        }
        if let Some(error) = error_for_status(&reply) {
            return Err(error);
        }
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> HttpResult<BoxedSse> {
        let agent = self.agent.clone();
        let reply = tokio::task::spawn_blocking(move || {
            let mut request = agent.get(uri.as_ref()).set("accept", EVENT_STREAM_MIME);
            if let Some(last) = &last_event_id {
                request = request.set(HEADER_LAST_EVENT_ID, last);
            }
            let request = apply_headers(
                request,
                auth_header.as_deref(),
                session_id.as_deref(),
                &custom_headers,
            );
            run_blocking(request, None)
        })
        .await??;
        if reply.status == 405 {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if let Some(error) = error_for_status(&reply) {
            return Err(error);
        }
        let Some(response) = reply.response else {
            return Err(StreamableHttpError::UnexpectedEndOfStream);
        };
        Ok(sse_stream_from(response.into_reader(), MAX_SSE_EVENT_SIZE))
    }
}

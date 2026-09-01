use serde_json::{Map, Value, json};
use yi_types::mcp::McpServerSpec;

use crate::http::{HttpTransport, PostBody};
use crate::stdio::StdioTransport;

// Invariant: the version rmcp's client advertised before D71. A server that
// hard-rejects an unknown version must see no change across the rewrite.
const PROTOCOL_VERSION: &str = "2025-11-25";
const INITIALIZE_ID: u64 = 1;
const OP_ID: u64 = 2;

pub enum Op {
    Discover,
    ToolsList,
    ToolsCall {
        name: String,
        arguments: Map<String, Value>,
    },
    ResourcesList,
    ResourcesRead {
        uri: String,
    },
    PromptsList,
    Ping,
}

impl Op {
    fn method(&self) -> Option<&'static str> {
        match self {
            Self::Discover | Self::ToolsList => Some("tools/list"),
            Self::ToolsCall { .. } => Some("tools/call"),
            Self::ResourcesList => Some("resources/list"),
            Self::ResourcesRead { .. } => Some("resources/read"),
            Self::PromptsList => Some("prompts/list"),
            Self::Ping => None,
        }
    }

    fn params(self) -> Value {
        match self {
            Self::ToolsCall { name, arguments } => json!({
                "name": name,
                "arguments": Value::Object(arguments),
            }),
            Self::ResourcesRead { uri } => json!({ "uri": uri }),
            Self::Discover
            | Self::ToolsList
            | Self::ResourcesList
            | Self::PromptsList
            | Self::Ping => json!({}),
        }
    }
}

/// The two wire transports design §5.2 allows, behind one request/notify pair.
/// Both are blocking: a one-shot has no second thing to wait on.
enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl Transport {
    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let reply = match self {
            Self::Stdio(stdio) => stdio.round_trip(&message, id)?,
            Self::Http(http) => http.round_trip(&message, PostBody::Request(id))?,
        };
        result_of(reply, method)
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        match self {
            Self::Stdio(stdio) => stdio.send(&message),
            Self::Http(http) => http.round_trip(&message, PostBody::Notification).map(drop),
        }
    }

    fn close(self) {
        match self {
            Self::Stdio(stdio) => stdio.close(),
            Self::Http(http) => http.close(),
        }
    }
}

/// Unwraps a JSON-RPC envelope: `result` through, `error` rendered with the
/// method that produced it, anything else refused rather than guessed at.
fn result_of(mut reply: Value, method: &str) -> Result<Value, String> {
    if let Some(error) = reply.get_mut("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("{method} failed ({code}): {text}"));
    }
    match reply.get_mut("result") {
        Some(result) => Ok(result.take()),
        None => Err(format!("{method}: reply carried neither result nor error")),
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "yi",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// One-shot execution (design §5.2): connect, negotiate, run the op, exit.
/// No resident process, no sockets held after return.
pub fn one_shot(spec: &McpServerSpec, op: Op) -> Result<Value, String> {
    one_shot_with_auth(spec, op, None)
}

/// Same, with a Bearer token attached to every HTTP request (stdio ignores it).
pub fn one_shot_with_auth(
    spec: &McpServerSpec,
    op: Op,
    auth_token: Option<String>,
) -> Result<Value, String> {
    let mut transport = match spec {
        McpServerSpec::Stdio { command, args, env } => {
            Transport::Stdio(StdioTransport::spawn(command, args, env)?)
        }
        McpServerSpec::Http { url } => Transport::Http(HttpTransport::new(url, auth_token)),
    };
    let outcome = negotiate_and_run(&mut transport, op);
    transport.close();
    outcome
}

fn negotiate_and_run(transport: &mut Transport, op: Op) -> Result<Value, String> {
    let info = transport.request(INITIALIZE_ID, "initialize", initialize_params())?;
    transport.notify("notifications/initialized")?;
    match op {
        Op::Ping => Ok(json!({
            "ok": true,
            "protocolVersion": info.get("protocolVersion").cloned().unwrap_or(Value::Null),
        })),
        Op::Discover => {
            let tools = transport
                .request(OP_ID, "tools/list", json!({}))
                .ok()
                .and_then(|mut listing| listing.get_mut("tools").map(Value::take))
                .unwrap_or_else(|| Value::Array(Vec::new()));
            Ok(json!({
                "protocolVersion": info.get("protocolVersion").cloned().unwrap_or(Value::Null),
                "serverInfo": info.get("serverInfo").cloned().unwrap_or(Value::Null),
                "capabilities": info.get("capabilities").cloned().unwrap_or(Value::Null),
                "instructions": info.get("instructions").cloned().unwrap_or(Value::Null),
                "tools": tools,
            }))
        }
        other => {
            let Some(method) = other.method() else {
                return Err("op has no method".to_owned());
            };
            transport.request(OP_ID, method, other.params())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_names_the_method_and_code() {
        let reply = json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32601, "message": "no such tool"}});
        let outcome = result_of(reply, "tools/call");
        assert_eq!(
            outcome,
            Err("tools/call failed (-32601): no such tool".to_owned())
        );
    }

    #[test]
    fn a_reply_with_neither_arm_is_refused() {
        let reply = json!({"jsonrpc": "2.0", "id": 2});
        assert!(result_of(reply, "tools/list").is_err());
    }

    #[test]
    fn initialize_advertises_the_carried_version_and_names_yi() {
        let params = initialize_params();
        assert_eq!(
            params.pointer("/protocolVersion").and_then(Value::as_str),
            Some("2025-11-25")
        );
        assert_eq!(
            params.pointer("/clientInfo/name").and_then(Value::as_str),
            Some("yi")
        );
    }

    #[test]
    fn result_passes_through_unknown_fields() {
        let reply = json!({"result": {"tools": [], "somethingNew": 7}});
        let outcome = result_of(reply, "tools/list");
        assert_eq!(outcome, Ok(json!({"tools": [], "somethingNew": 7})));
    }
}

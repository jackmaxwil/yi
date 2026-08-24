use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, PaginatedRequestParams};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde_json::{Map, Value, json};
use yi_types::mcp::McpServerSpec;

pub struct Connected {
    pub protocol_version: String,
    pub server_name: String,
    pub instructions: Option<String>,
    pub capabilities: Value,
    pub tools: Vec<Value>,
}

pub enum Op {
    Discover,
    ToolsList,
    ToolsCall {
        name: String,
        arguments: Map<String, Value>,
    },
    ResourcesList,
    PromptsList,
    Ping,
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn stdio_command(
    command: &str,
    args: &[String],
    env: &Map<String, Value>,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    for (key, value) in env {
        if let Some(text) = value.as_str() {
            cmd.env(key, text);
        }
    }
    cmd
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        match spec {
            McpServerSpec::Stdio { command, args, env } => {
                let transport = TokioChildProcess::new(stdio_command(command, args, env))
                    .map_err(|error| error.to_string())?;
                let service =
                    ().serve(transport)
                        .await
                        .map_err(|error| format!("connect failed: {error}"))?;
                let info = service
                    .peer_info()
                    .ok_or("server sent no initialize result")?;
                let outcome = run_op(&service, &info, op).await;
                let _ = service.cancel().await;
                outcome
            }
            McpServerSpec::Http { url } => {
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.clone());
                if let Some(token) = auth_token {
                    config = config.auth_header(token);
                }
                let transport = StreamableHttpClientTransport::with_client(
                    crate::http::UreqHttpClient::default(),
                    config,
                );
                let service =
                    ().serve(transport)
                        .await
                        .map_err(|error| format!("connect failed: {error}"))?;
                let info = service
                    .peer_info()
                    .ok_or("server sent no initialize result")?;
                let outcome = run_op(&service, &info, op).await;
                let _ = service.cancel().await;
                outcome
            }
        }
    })
}

async fn run_op(
    service: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    info: &rmcp::model::ServerPeerInfo,
    op: Op,
) -> Result<Value, String> {
    match op {
        Op::Discover => {
            let tools = service
                .list_tools(Option::<PaginatedRequestParams>::None)
                .await
                .map(|result| result.tools.iter().map(to_value).collect::<Vec<_>>())
                .unwrap_or_default();
            Ok(json!({
                "protocolVersion": to_value(&info.protocol_version),
                "serverInfo": to_value(&info.server_info),
                "capabilities": to_value(&info.capabilities),
                "instructions": info.instructions,
                "tools": tools,
            }))
        }
        Op::ToolsList => {
            let result = service
                .list_tools(Option::<PaginatedRequestParams>::None)
                .await
                .map_err(|error| error.to_string())?;
            Ok(to_value(&result))
        }
        Op::ToolsCall { name, arguments } => {
            let result = service
                .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
                .await
                .map_err(|error| error.to_string())?;
            Ok(to_value(&result))
        }
        Op::ResourcesList => {
            let result = service
                .list_resources(Option::<PaginatedRequestParams>::None)
                .await
                .map_err(|error| error.to_string())?;
            Ok(to_value(&result))
        }
        Op::PromptsList => {
            let result = service
                .list_prompts(Option::<PaginatedRequestParams>::None)
                .await
                .map_err(|error| error.to_string())?;
            Ok(to_value(&result))
        }
        Op::Ping => Ok(json!({
            "ok": true,
            "protocolVersion": to_value(&info.protocol_version),
        })),
    }
}

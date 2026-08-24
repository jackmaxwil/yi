use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, PaginatedRequestParams};
use rmcp::transport::TokioChildProcess;
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

fn command_for(spec: &McpServerSpec) -> Result<tokio::process::Command, String> {
    let McpServerSpec::Stdio { command, args, env } = spec else {
        return Err(
            "streamable-HTTP transport is not wired yet (ureq transport is the next 2c batch); use a stdio config entry".to_owned(),
        );
    };
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    for (key, value) in env {
        if let Some(text) = value.as_str() {
            cmd.env(key, text);
        }
    }
    Ok(cmd)
}

/// One-shot execution (design §5.2): connect, negotiate, run the op, exit.
/// No resident process, no sockets held after return.
pub fn one_shot(spec: &McpServerSpec, op: Op) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let transport =
            TokioChildProcess::new(command_for(spec)?).map_err(|error| error.to_string())?;
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

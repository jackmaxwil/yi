//! An MCP server built on rmcp, used as external ground truth for the
//! hand-rolled client (D71). rmcp is a dev-dependency: it never ships.

use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};

#[derive(Clone)]
struct Echo;

impl ServerHandler for Echo {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some("echoes what it is given".to_owned());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        });
        let Some(schema) = schema.as_object().cloned() else {
            return Err(McpError::internal_error("schema was not an object", None));
        };
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "echo",
            "returns its argument",
            schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if request.name != "echo" {
            return Err(McpError::invalid_params("no such tool", None));
        }
        let text = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = Echo.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

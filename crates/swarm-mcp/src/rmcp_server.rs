#![cfg(feature = "rmcp")]

use rmcp::{
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData as McpError, ServerHandler, ServiceExt,
};

use crate::mcp_dispatch::route_mcp_tool_call;
use crate::mcp_schema::mcp_tool_descriptors;
use swarm_contracts::mcp::McpToolDescriptor;

/// rmcp handler for the Agent Swarm MCP tool surface.
#[derive(Clone, Default)]
pub struct SwarmMcpServer;

impl SwarmMcpServer {
    fn tools() -> Vec<Tool> {
        mcp_tool_descriptors()
            .into_iter()
            .map(Self::descriptor_to_tool)
            .collect()
    }

    fn descriptor_to_tool(descriptor: McpToolDescriptor) -> Tool {
        Tool::new_with_raw(
            descriptor.name,
            Some(descriptor.description.into()),
            descriptor
                .input_schema
                .as_object()
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// Serialize the tools/list payload from the descriptor table via the canonical
    /// schema serializer.
    ///
    /// NOTE (P6 follow-up): this returns `mcp_tools_pretty_json()` — the SAME path the
    /// frozen `mcp-tools.json` is generated from — so the test below only re-asserts
    /// descriptor<->frozen identity (already covered by mcp_schema's idempotency gate).
    /// It does NOT yet exercise rmcp's own `Tool` wire-serialization. A real rmcp-wire
    /// parity test (serialize `SwarmMcpServer::tools()` and diff vs the frozen artifact)
    /// is REQUIRED before rmcp can become the default transport: rmcp's `Tool` serde may
    /// rename/reorder fields (esp. `inputSchema`) and drift from the frozen shape.
    #[cfg(test)]
    pub(crate) fn tools_list_payload() -> String {
        crate::mcp_schema::mcp_tools_pretty_json()
    }
}

impl ServerHandler for SwarmMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(
                Implementation::new("agent-swarm", env!("CARGO_PKG_VERSION"))
                    .with_title("Agent Swarm")
                    .with_description("Agent Swarm MCP service"),
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: Self::tools(),
            next_cursor: None,
            meta: None,
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
            let result = route_mcp_tool_call(request.name.as_ref(), &arguments);

            Ok(result.map_or_else(
                |err| CallToolResult::error(vec![Content::text(err)]),
                |output| {
                    if output.is_error {
                        CallToolResult::error(vec![Content::text(output.text)])
                    } else {
                        CallToolResult::success(vec![Content::text(output.text)])
                    }
                },
            ))
        }
    }
}

/// Serve the Agent Swarm rmcp transport over stdio.
pub async fn serve_stdio() -> Result<(), String> {
    let server = SwarmMcpServer::default();
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| format!("Error starting agent-swarm rmcp stdio service: {err}"))?;
    service
        .waiting()
        .await
        .map(|_| ())
        .map_err(|err| format!("agent-swarm rmcp service exited: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rmcp_tools_list_payload_matches_frozen_json_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mcp-tools.json");
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));

        assert_eq!(on_disk, SwarmMcpServer::tools_list_payload());
    }
}

//! MCP server implementation — stdio-based Model Context Protocol server.
//!
//! Allows AI agents (Claude Desktop, etc.) to call virtuoso-cli as a tool
//! via the MCP protocol over stdio.

use crate::client::bridge::VirtuosoClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

pub mod server;
pub mod tools;

/// Version reported to MCP clients in `serverInfo`.
///
/// Derived from the crate version so it can never drift from what
/// `vcli --version` prints (it used to be a hardcoded "0.3.18" while the CLI
/// was at 1.3.5).
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP server configuration.
#[derive(Debug, Clone)]
pub struct McpConfig {
    #[allow(dead_code)]
    pub capabilities: crate::capability::CapabilitySet,
}

impl McpConfig {
    pub fn from_env() -> Self {
        Self {
            capabilities: crate::capability::CapabilitySet::from_env(),
        }
    }
}

/// MCP request envelope (JSON-RPC 2.0).
#[derive(Debug, Deserialize)]
pub struct McpRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
    id: Option<serde_json::Value>,
}

/// MCP response envelope (JSON-RPC 2.0).
#[derive(Debug, Serialize)]
pub struct McpResponse {
    #[serde(rename = "jsonrpc")]
    pub jsonrpc: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<McpError>,
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl McpResponse {
    pub fn success(result: serde_json::Value, id: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(code: i32, message: String, id: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: None,
            error: Some(McpError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

/// MCP server that reads JSON-RPC requests from stdin and writes responses to stdout.
pub struct McpServer {
    #[allow(dead_code)]
    config: McpConfig,
    tools: Vec<tools::McpTool>,
    /// Config + target identity. Held here so call_tool can pass it through to
    /// the RPC dispatcher instead of re-reading env with no target context.
    ctx: crate::context::CommandContext,
}

impl McpServer {
    pub fn new(config: McpConfig, ctx: crate::context::CommandContext) -> Self {
        let mut tools = tools::all_tools(&config.capabilities);
        // Merge in plugin tools if any
        if let Ok(registry) = super::plugins::PluginRegistry::get_global() {
            tools.extend(registry.mcp_tools());
        }
        Self { config, tools, ctx }
    }

    /// Run the MCP stdio loop.
    pub fn run(&self) -> Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout().lock();
        let mut lines = stdin.lock().lines();

        // Send initial handshake (empty JSON object followed by newline)
        // or just start processing requests
        while let Some(Ok(line)) = lines.next() {
            if line.trim().is_empty() {
                continue;
            }

            let request: McpRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    let response = McpResponse::error(-32700, format!("parse error: {e}"), None);
                    writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).ok();
                    stdout.flush().ok();
                    continue;
                }
            };

            let response = self.handle_request(request);
            writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).ok();
            stdout.flush().ok();
        }

        Ok(())
    }

    fn handle_request(&self, request: McpRequest) -> McpResponse {
        match request.method.as_str() {
            "initialize" => McpResponse::success(
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                    },
                    "serverInfo": {
                        "name": "virtuoso-cli",
                        "version": SERVER_VERSION,
                    }
                }),
                request.id,
            ),
            "tools/list" => {
                let tool_list: Vec<serde_json::Value> = self
                    .tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema,
                        })
                    })
                    .collect();
                McpResponse::success(serde_json::json!({ "tools": tool_list }), request.id)
            }
            "tools/call" => {
                let name = request
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = request
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                if let Some(tool) = self.tools.iter().find(|t| t.name == name) {
                    match self.call_tool(tool, arguments) {
                        Ok(result) => McpResponse::success(result, request.id),
                        Err(e) => McpResponse::error(-32603, e.to_string(), request.id),
                    }
                } else {
                    McpResponse::error(-32602, format!("unknown tool: {}", name), request.id)
                }
            }
            _ => McpResponse::error(
                -32601,
                format!("method not found: {}", request.method),
                request.id,
            ),
        }
    }

    fn call_tool(
        &self,
        tool: &tools::McpTool,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let client = VirtuosoClient::from_context(&self.ctx)?;
        let api_key = std::env::var("VCLI_API_KEY").ok().filter(|k| !k.is_empty());
        let request = crate::rpc::dispatcher::RpcRequest {
            method: tool.rpc_method.to_string(),
            params: arguments,
            api_key,
        };
        crate::rpc::dispatcher::RpcDispatcher::new(self.ctx.clone()).dispatch(&client, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MCP `initialize` response must advertise the same version the
    /// CLI reports via `--version`. This pins the "single source of truth"
    /// contract: both come from `CARGO_PKG_VERSION`.
    #[test]
    fn initialize_reports_the_crate_version() {
        let ctx = crate::context::CommandContext::new(
            crate::config::Config::from_env().expect("default config"),
            None,
        )
        .expect("context");
        let server = McpServer::new(McpConfig::from_env(), ctx);

        let request = McpRequest {
            jsonrpc: "2.0".into(),
            method: "initialize".into(),
            params: serde_json::json!({}),
            id: Some(serde_json::json!(1)),
        };
        let response = server.handle_request(request);

        let result = response.result.expect("initialize must succeed");
        assert_eq!(
            result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"),
            "MCP serverInfo.version must match the crate version"
        );
        assert_eq!(result["serverInfo"]["name"], "virtuoso-cli");
    }

    #[test]
    fn server_version_constant_matches_crate_version() {
        assert_eq!(SERVER_VERSION, env!("CARGO_PKG_VERSION"));
    }
}

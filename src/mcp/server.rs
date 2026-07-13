//! MCP server stdio implementation.
//!
//! The actual server logic is in `../mod.rs`; this module provides
//! the main entry point and command-line integration.

use crate::error::Result;

/// Run the MCP server (stdio mode).
pub fn run() -> Result<()> {
    crate::auth::Auth::init();
    let config = crate::mcp::McpConfig::from_env();
    let server = crate::mcp::McpServer::new(config);
    server.run()
}

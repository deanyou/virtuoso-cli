//! MCP server stdio implementation.
//!
//! The actual server logic is in `../mod.rs`; this module provides
//! the main entry point and command-line integration.

use crate::error::Result;

/// Run the MCP server (stdio mode).
pub fn run() -> Result<()> {
    crate::auth::Auth::init();
    let config = crate::mcp::McpConfig::from_env();
    let ctx = crate::context::CommandContext::new(crate::config::Config::from_env()?, None)?;
    let server = crate::mcp::McpServer::new(config, ctx);
    server.run()
}

//! MCP server stdio implementation.
//!
//! The actual server logic is in `../mod.rs`; this module provides
//! the main entry point and command-line integration.

use crate::error::Result;

/// Run the MCP server (stdio mode).
pub fn run() -> Result<()> {
    crate::auth::Auth::init();
    let config = crate::mcp::McpConfig::from_env();
    // Resolve the active target via the canonical selection path so the MCP
    // server inherits the same target identity as any other CLI invocation —
    // NOT a bare env-only Config that ignores targets.yaml.
    let selection = crate::target::resolve::resolve_selection(None, None).map_err(|e| {
        crate::error::VirtuosoError::Config(format!(
            "MCP server failed to resolve target selection: {e}"
        ))
    })?;
    let resolved = crate::target::resolve::resolve_from_selection(selection).map_err(|e| {
        crate::error::VirtuosoError::Config(format!(
            "MCP server failed to resolve target: {e}"
        ))
    })?;
    let ctx = crate::context::CommandContext::from_resolved(&resolved)?;
    let server = crate::mcp::McpServer::new(config, ctx);
    server.run()
}

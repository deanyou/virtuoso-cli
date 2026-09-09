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
        crate::error::VirtuosoError::Config(format!("MCP server failed to resolve target: {e}"))
    })?;
    let ctx = crate::context::CommandContext::from_resolved(&resolved)?;
    let server = crate::mcp::McpServer::new(config, ctx);
    server.run()
}

#[cfg(test)]
mod tests {
    struct EnvGuard {
        vars: Vec<(String, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&str]) -> Self {
            let mut vars = Vec::new();
            for k in keys {
                vars.push((k.to_string(), std::env::var_os(k)));
                std::env::remove_var(k);
            }
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.vars.drain(..) {
                match v {
                    Some(val) => std::env::set_var(&k, val),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }

    /// Regression: MCP entry must inherit the active target from targets.yaml
    /// via the canonical selection chain — NOT fall back to a bare env-only
    /// Config that silently drops target identity.
    #[test]
    #[serial_test::serial]
    fn mcp_selection_chain_inherits_active_target_from_targets_yaml() {
        let _env = EnvGuard::new(&[
            "VB_TARGETS_FILE",
            "VB_TARGET",
            "VB_PROFILE",
            "VB_CONFIG_DIR",
            "VB_REMOTE_HOST",
        ]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let targets_path = tmp.path().join("targets.yaml");
        std::fs::write(
            &targets_path,
            "active_target: prod\n\
             targets:\n  \
               prod:\n    \
                 remote_host: eda-prod\n    \
                 port: 65535\n",
        )
        .expect("write targets.yaml");
        std::env::set_var("VB_TARGETS_FILE", &targets_path);

        // Replicate the exact selection chain mcp::server::run() uses.
        let selection = crate::target::resolve::resolve_selection(None, None)
            .expect("resolve_selection should succeed with valid active_target");
        let resolved = crate::target::resolve::resolve_from_selection(selection)
            .expect("resolve_from_selection should succeed");
        let ctx = crate::context::CommandContext::from_resolved(&resolved)
            .expect("from_resolved should succeed");

        assert_eq!(
            ctx.target_id(),
            Some("prod"),
            "MCP context must carry the active_target identity from targets.yaml"
        );
        assert_eq!(
            ctx.config().remote_host.as_deref(),
            Some("eda-prod"),
            "MCP config must reflect the active target's remote_host"
        );
    }

    /// Regression: a broken active_target must make the MCP selection chain
    /// fail — not silently degrade to an env-only Config with no target.
    #[test]
    #[serial_test::serial]
    fn mcp_selection_chain_fails_on_broken_active_target() {
        let _env = EnvGuard::new(&[
            "VB_TARGETS_FILE",
            "VB_TARGET",
            "VB_PROFILE",
            "VB_CONFIG_DIR",
            "VB_REMOTE_HOST",
        ]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let targets_path = tmp.path().join("targets.yaml");
        std::fs::write(
            &targets_path,
            "active_target: does-not-exist\n\
             targets:\n  \
               prod:\n    \
                 remote_host: eda-prod\n",
        )
        .expect("write targets.yaml");
        std::env::set_var("VB_TARGETS_FILE", &targets_path);

        // resolve_selection itself validates that active_target exists in
        // the targets map — it fails early rather than returning a
        // Selection that resolve_from_selection would later reject.
        let result = crate::target::resolve::resolve_selection(None, None);
        assert!(
            result.is_err(),
            "resolve_selection must fail when active_target is missing, \
             so MCP cannot silently run with no target identity"
        );
    }
}

//! Plugin registry — discovers and manages TOML-based plugins.

use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use crate::mcp::tools::McpTool;
use crate::rpc::schema::{Method, Param};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;

use super::schema::PluginTool;

/// Global plugin registry, discovered once at startup.
static PLUGIN_REGISTRY: OnceLock<PluginRegistry> = OnceLock::new();

/// Plugin registry — holds all discovered plugin tools.
#[derive(Debug)]
pub struct PluginRegistry {
    tools: Vec<PluginTool>,
}

impl PluginRegistry {
    /// Discover and load all plugins from `~/.config/vcli/plugins/*.toml`.
    pub fn discover() -> Result<Self> {
        let config_dir = crate::runtime_paths::config_subdir(&["plugins"]);

        if !config_dir.exists() {
            return Ok(PluginRegistry { tools: Vec::new() });
        }

        let mut tools = Vec::new();

        let entries = std::fs::read_dir(&config_dir)
            .map_err(|e| VirtuosoError::Execution(format!("failed to read plugins dir: {e}")))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                match Self::load_plugin(&path) {
                    Ok(mut plugin_tools) => tools.append(&mut plugin_tools),
                    Err(e) => {
                        tracing::warn!("failed to load plugin {:?}: {}", path, e);
                    }
                }
            }
        }

        tracing::debug!("discovered {} plugin tools", tools.len());
        Ok(PluginRegistry { tools })
    }

    /// Load a single plugin TOML file.
    fn load_plugin(path: &PathBuf) -> Result<Vec<PluginTool>> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| VirtuosoError::Execution(format!("failed to read {:?}: {e}", path)))?;

        // Try parsing as an array of tools first (TOML array of tables: [[tools]]...)
        if let Ok(single) = toml::from_str::<PluginTool>(&content) {
            single.validate()?;
            return Ok(vec![single]);
        }

        // Try parsing as Vec<PluginTool> for multiple [[tools]] entries
        let plugins: Vec<PluginTool> = toml::from_str(&content)
            .map_err(|e| VirtuosoError::Execution(format!("failed to parse {:?}: {e}", path)))?;

        for plugin in &plugins {
            plugin.validate()?;
        }

        Ok(plugins)
    }

    /// Get the global plugin registry, initializing it if not yet loaded.
    pub fn get_global() -> Result<&'static PluginRegistry> {
        // Use get_or_init which is stable
        Ok(PLUGIN_REGISTRY.get_or_init(|| {
            Self::discover().expect("plugin discovery should not fail at this point")
        }))
    }

    /// Dispatch a plugin tool call.
    pub fn dispatch(
        &self,
        domain: &str,
        op: &str,
        params: Value,
        client: &VirtuosoClient,
    ) -> Result<Value> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.domain == domain && t.name == op)
            .ok_or_else(|| {
                VirtuosoError::Execution(format!("unknown plugin method '{}.{}'", domain, op))
            })?;

        let args = params
            .as_object()
            .ok_or_else(|| VirtuosoError::Execution("params must be an object".into()))?;

        let skill = tool.render_skill(args)?;
        let result = client
            .execute_skill_unchecked(&skill, None)?
            .ok_or_exec(&format!("plugin {}.{}", domain, op))?;

        if result.output.trim().is_empty() {
            Ok(Value::Null)
        } else {
            // Try to parse as JSON, fall back to string
            match serde_json::from_str(&result.output) {
                Ok(v) => Ok(v),
                Err(_) => Ok(Value::String(result.output.trim().to_string())),
            }
        }
    }

    /// Convert plugin tools to MCP tools.
    pub fn mcp_tools(&self) -> Vec<McpTool> {
        self.tools
            .iter()
            .map(|t| {
                let domain = t.rpc_method().split('.').next().unwrap_or("").into();
                McpTool::new(
                    t.mcp_name(),
                    t.description.clone(),
                    t.input_schema(),
                    t.rpc_method(),
                    domain,
                )
            })
            .collect()
    }

    /// Convert plugin tools to RPC schema methods.
    #[allow(dead_code)]
    pub fn schema_methods(&self) -> Vec<Method> {
        self.tools
            .iter()
            .map(|t| Method {
                name: t.rpc_method(),
                summary: t.description.clone(),
                params: t
                    .params
                    .iter()
                    .map(|(name, def)| Param {
                        name: name.clone(),
                        ptype: def.ptype.clone(),
                        description: def.description.clone(),
                        required: def.required,
                    })
                    .collect(),
                returns: "any".into(),
            })
            .collect()
    }
}

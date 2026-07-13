//! Plugin system — TOML-based tool discovery and registration.
//!
//! Plugins are defined in `~/.config/vcli/plugins/*.toml` and automatically
//! registered as MCP tools and RPC schema methods at startup.

pub mod registry;
pub mod schema;

pub use registry::PluginRegistry;

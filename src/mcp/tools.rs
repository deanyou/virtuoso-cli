//! MCP tool definitions — maps RPC methods to MCP tools.
//!
//! Tools are filtered based on the client's capability set.

use crate::capability::CapabilitySet;
use serde_json::Value;

/// A single MCP tool derived from an RPC method.
#[derive(Debug, Clone)]
pub struct McpTool {
    /// MCP tool name (e.g. "schematic_open_cell_view")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: Value,
    /// Corresponding RPC method (e.g. "schematic.open_cell_view")
    pub rpc_method: String,
    /// Required capability domain
    #[allow(dead_code)]
    pub domain: String,
}

impl McpTool {
    /// Create an MCP tool with owned strings (for plugin tools).
    pub fn new(
        name: String,
        description: String,
        input_schema: Value,
        rpc_method: String,
        domain: String,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            rpc_method,
            domain,
        }
    }

    /// Check if this tool is permitted for the given capability set.
    pub fn permitted_for(&self, caps: &CapabilitySet) -> bool {
        caps.permits_method(&self.rpc_method)
    }
}

/// Return all available MCP tools filtered by capability.
pub fn all_tools(caps: &CapabilitySet) -> Vec<McpTool> {
    let mut tools = Vec::new();
    tools.extend(
        schematic_tools()
            .into_iter()
            .filter(|t| t.permitted_for(caps)),
    );
    tools.extend(
        maestro_tools()
            .into_iter()
            .filter(|t| t.permitted_for(caps)),
    );
    tools.extend(window_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools.extend(cell_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools.extend(tx_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools.extend(file_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools.extend(util_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools.extend(skill_tools().into_iter().filter(|t| t.permitted_for(caps)));
    tools
}

/// Return all available MCP tools (unfiltered).
#[allow(dead_code)]
pub fn all_tools_unfiltered() -> Vec<McpTool> {
    let mut tools = Vec::new();
    tools.extend(schematic_tools());
    tools.extend(maestro_tools());
    tools.extend(window_tools());
    tools.extend(cell_tools());
    tools.extend(tx_tools());
    tools.extend(file_tools());
    tools.extend(util_tools());
    tools.extend(skill_tools());
    tools
}

pub fn schematic_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "schematic_open_cell_view".into(),
            description: "Open or create a schematic cellview for editing".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "lib": { "type": "string", "description": "Library name" },
                    "cell": { "type": "string", "description": "Cell name" },
                    "view": { "type": "string", "description": "View name (default: schematic)", "default": "schematic" },
                },
                "required": ["lib", "cell"]
            }),
            rpc_method: String::from("schematic.open_cell_view"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_place".into(),
            description: "Place an instance in the open schematic".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "master": { "type": "string", "description": "Master cell in lib/cell format (e.g. smic13mmrf/p12)" },
                    "name": { "type": "string", "description": "Instance name" },
                    "x": { "type": "integer", "description": "X coordinate", "default": 0 },
                    "y": { "type": "integer", "description": "Y coordinate", "default": 0 },
                    "orient": { "type": "string", "description": "Orientation (R0, R90, R180, R270, MY, MX, etc.)", "default": "R0" },
                },
                "required": ["master", "name"]
            }),
            rpc_method: String::from("schematic.place"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_wire".into(),
            description: "Create a wire between named net and coordinates".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "net": { "type": "string", "description": "Net name" },
                    "points": { "type": "array", "items": { "type": "string" }, "description": "Points as x,y strings" },
                },
                "required": ["net", "points"]
            }),
            rpc_method: String::from("schematic.wire"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_save".into(),
            description: "Save the current schematic".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.save"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_check".into(),
            description: "Run schematic check (schCheck)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.check"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_list_instances".into(),
            description: "List all instances in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_instances"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_list_nets".into(),
            description: "List all nets in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_nets"),
            domain: "schematic".into(),
        },
        McpTool {
            name: "schematic_list_pins".into(),
            description: "List all pins in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_pins"),
            domain: "schematic".into(),
        },
    ]
}

pub fn maestro_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "maestro_open_session".into(),
            description: "Open a Maestro session".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "lib": { "type": "string", "description": "Library name" },
                    "cell": { "type": "string", "description": "Cell name" },
                    "view": { "type": "string", "description": "View name", "default": "maestro" },
                },
                "required": ["lib", "cell"]
            }),
            rpc_method: String::from("maestro.open_session"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_close_session".into(),
            description: "Close a Maestro session".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session ID (e.g. fnxSession4)" },
                },
                "required": ["session"]
            }),
            rpc_method: String::from("maestro.close_session"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_list_sessions".into(),
            description: "List all active Maestro sessions".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("maestro.list_sessions"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_set_var".into(),
            description: "Set a design variable".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Variable name" },
                    "value": { "type": "string", "description": "Variable value" },
                },
                "required": ["name", "value"]
            }),
            rpc_method: String::from("maestro.set_var"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_get_var".into(),
            description: "Get a design variable".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Variable name" },
                },
                "required": ["name"]
            }),
            rpc_method: String::from("maestro.get_var"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_list_vars".into(),
            description: "List all design variables".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("maestro.list_vars"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_run".into(),
            description: "Run simulation asynchronously".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session ID" },
                },
                "required": ["session"]
            }),
            rpc_method: String::from("maestro.run"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_save".into(),
            description: "Save Maestro setup to disk".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session ID" },
                },
                "required": ["session"]
            }),
            rpc_method: String::from("maestro.save"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_export".into(),
            description: "Export results to CSV".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session ID" },
                    "path": { "type": "string", "description": "Output CSV file path" },
                    "test_name": { "type": "string", "description": "Test name (optional)" },
                },
                "required": ["session", "path"]
            }),
            rpc_method: String::from("maestro.export"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_get_output_value".into(),
            description: "Get simulation output value".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Output name" },
                    "test": { "type": "string", "description": "Test name" },
                    "corner": { "type": "string", "description": "Corner name (optional)" },
                },
                "required": ["name", "test"]
            }),
            rpc_method: String::from("maestro.get_output_value"),
            domain: "maestro".into(),
        },
        McpTool {
            name: "maestro_get_history_list".into(),
            description: "List simulation history".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("maestro.get_history_list"),
            domain: "maestro".into(),
        },
    ]
}

pub fn window_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "window_list".into(),
            description: "List all open Virtuoso windows".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("window.list"),
            domain: "window".into(),
        },
        McpTool {
            name: "window_screenshot".into(),
            description: "Capture screenshot of current window".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Output PNG file path" },
                },
                "required": ["path"]
            }),
            rpc_method: String::from("window.screenshot"),
            domain: "window".into(),
        },
    ]
}

pub fn cell_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "cell_open".into(),
            description: "Open a cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "lib": { "type": "string", "description": "Library name" },
                    "cell": { "type": "string", "description": "Cell name" },
                    "view": { "type": "string", "description": "View name", "default": "layout" },
                    "mode": { "type": "string", "description": "Open mode: r(ead), o(verwrite), a(ppend)", "default": "a" },
                },
                "required": ["lib", "cell"]
            }),
            rpc_method: String::from("cell.open"),
            domain: "cell".into(),
        },
        McpTool {
            name: "cell_save".into(),
            description: "Save the current cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("cell.save"),
            domain: "cell".into(),
        },
        McpTool {
            name: "cell_close".into(),
            description: "Close the current cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("cell.close"),
            domain: "cell".into(),
        },
        McpTool {
            name: "cell_info".into(),
            description: "Get current cellview info (lib/cell/view)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("cell.info"),
            domain: "cell".into(),
        },
    ]
}

pub fn tx_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "tx_begin".into(),
            description: "Begin a schematic transaction (snapshot)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Transaction ID" },
                    "lib": { "type": "string", "description": "Library name" },
                    "cell": { "type": "string", "description": "Cell name" },
                    "view": { "type": "string", "description": "View name", "default": "schematic" },
                },
                "required": ["id", "lib", "cell"]
            }),
            rpc_method: String::from("tx.begin"),
            domain: "tx".into(),
        },
        McpTool {
            name: "tx_commit".into(),
            description: "Commit the current transaction".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("tx.commit"),
            domain: "tx".into(),
        },
        McpTool {
            name: "tx_rollback".into(),
            description: "Rollback the current transaction".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("tx.rollback"),
            domain: "tx".into(),
        },
        McpTool {
            name: "tx_diff".into(),
            description: "Get the diff between current and snapshot".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("tx.diff"),
            domain: "tx".into(),
        },
        McpTool {
            name: "tx_status".into(),
            description: "Get current transaction status".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("tx.status"),
            domain: "tx".into(),
        },
    ]
}

pub fn file_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "file_upload".into(),
            description: "Upload a local file to Virtuoso server".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "local": { "type": "string", "description": "Local file path" },
                    "remote": { "type": "string", "description": "Remote file path on server" },
                },
                "required": ["local", "remote"]
            }),
            rpc_method: String::from("file.upload"),
            domain: "file".into(),
        },
        McpTool {
            name: "file_download".into(),
            description: "Download a file from Virtuoso server".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "remote": { "type": "string", "description": "Remote file path on server" },
                    "local": { "type": "string", "description": "Local destination path" },
                },
                "required": ["remote", "local"]
            }),
            rpc_method: String::from("file.download"),
            domain: "file".into(),
        },
    ]
}

pub fn util_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "util_version".into(),
            description: "Get Virtuoso version info".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("util.version"),
            domain: "util".into(),
        },
        McpTool {
            name: "util_ping".into(),
            description: "Check connection to Virtuoso".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("util.ping"),
            domain: "util".into(),
        },
        McpTool {
            name: "util_ciw_print".into(),
            description: "Print message to CIW console".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Message to print" },
                },
                "required": ["message"]
            }),
            rpc_method: String::from("util.ciw_print"),
            domain: "util".into(),
        },
    ]
}

pub fn skill_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "skill_exec".into(),
            description: "Execute raw SKILL code (Admin only)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "SKILL code to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (optional)" },
                },
                "required": ["code"]
            }),
            rpc_method: String::from("skill.exec"),
            domain: "skill".into(),
        },
        McpTool {
            name: "skill_load".into(),
            description: "Load a SKILL (.il) file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to .il file" },
                },
                "required": ["path"]
            }),
            rpc_method: String::from("skill.load"),
            domain: "skill".into(),
        },
    ]
}

//! MCP tool definitions — maps RPC methods to MCP tools.

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
}

impl McpTool {
    /// Create an MCP tool with owned strings (for plugin tools).
    pub fn new(name: String, description: String, input_schema: Value, rpc_method: String) -> Self {
        Self {
            name,
            description,
            input_schema,
            rpc_method,
        }
    }
}

/// Return all available MCP tools.
pub fn all_tools() -> Vec<McpTool> {
    let mut tools = Vec::new();
    tools.extend(schematic_tools());
    tools.extend(maestro_tools());
    tools.extend(window_tools());
    tools.extend(cell_tools());
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
        },
        McpTool {
            name: "schematic_save".into(),
            description: "Save the current schematic".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.save"),
        },
        McpTool {
            name: "schematic_check".into(),
            description: "Run schematic check (schCheck)".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.check"),
        },
        McpTool {
            name: "schematic_list_instances".into(),
            description: "List all instances in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_instances"),
        },
        McpTool {
            name: "schematic_list_nets".into(),
            description: "List all nets in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_nets"),
        },
        McpTool {
            name: "schematic_list_pins".into(),
            description: "List all pins in the open cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("schematic.list_pins"),
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
        },
        McpTool {
            name: "maestro_list_sessions".into(),
            description: "List all active Maestro sessions".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("maestro.list_sessions"),
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
        },
        McpTool {
            name: "maestro_list_vars".into(),
            description: "List all design variables".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("maestro.list_vars"),
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
        },
        McpTool {
            name: "cell_save".into(),
            description: "Save the current cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("cell.save"),
        },
        McpTool {
            name: "cell_close".into(),
            description: "Close the current cellview".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            rpc_method: String::from("cell.close"),
        },
    ]
}

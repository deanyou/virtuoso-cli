//! RPC schema — method signatures and parameter metadata.
//!
//! Each method has:
//!   - `name`: "domain.method" namespaced by domain
//!   - `params`: JSON Schema-like parameter list
//!   - `returns`: return type description

use serde::{Deserialize, Serialize};

/// A single RPC method definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    pub summary: String,
    pub params: Vec<Param>,
    pub returns: String,
}

/// A parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ptype: String,
    pub description: String,
    pub required: bool,
}

/// Full RPC schema containing all available methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcSchema {
    pub version: String,
    pub methods: Vec<Method>,
}

impl RpcSchema {
    pub fn new(methods: Vec<Method>) -> Self {
        Self {
            version: "1.0".into(),
            methods,
        }
    }
}

/// Built-in schema with all available RPC methods.
pub fn standard_schema() -> RpcSchema {
    RpcSchema::new(vec![
        // ── Schematic ────────────────────────────────────────────────
        Method {
            name: "schematic.open_cell_view".into(),
            summary: "Open or create a schematic cellview for editing".into(),
            params: vec![
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name (default: schematic)".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.place".into(),
            summary: "Place an instance in the open schematic".into(),
            params: vec![
                Param {
                    name: "master".into(),
                    ptype: "string".into(),
                    description: "Master cell in lib/cell format (e.g. smic13mmrf/p12)".into(),
                    required: true,
                },
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Instance name".into(),
                    required: true,
                },
                Param {
                    name: "x".into(),
                    ptype: "integer".into(),
                    description: "X coordinate".into(),
                    required: false,
                },
                Param {
                    name: "y".into(),
                    ptype: "integer".into(),
                    description: "Y coordinate".into(),
                    required: false,
                },
                Param {
                    name: "orient".into(),
                    ptype: "string".into(),
                    description: "Orientation (R0, R90, R180, R270, MY, MX, etc.)".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.wire".into(),
            summary: "Create a wire between named net and coordinates".into(),
            params: vec![
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name".into(),
                    required: true,
                },
                Param {
                    name: "points".into(),
                    ptype: "array".into(),
                    description: "Points as x1,y1 x2,y2 ...".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.label".into(),
            summary: "Add a net label at coordinates".into(),
            params: vec![
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name".into(),
                    required: true,
                },
                Param {
                    name: "x".into(),
                    ptype: "integer".into(),
                    description: "X coordinate".into(),
                    required: false,
                },
                Param {
                    name: "y".into(),
                    ptype: "integer".into(),
                    description: "Y coordinate".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.pin".into(),
            summary: "Add a pin to a net".into(),
            params: vec![
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name".into(),
                    required: true,
                },
                Param {
                    name: "direction".into(),
                    ptype: "string".into(),
                    description: "Pin direction: input, output, inputOutput".into(),
                    required: true,
                },
                Param {
                    name: "x".into(),
                    ptype: "integer".into(),
                    description: "X coordinate".into(),
                    required: false,
                },
                Param {
                    name: "y".into(),
                    ptype: "integer".into(),
                    description: "Y coordinate".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.save".into(),
            summary: "Save the current schematic".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.check".into(),
            summary: "Run schematic check (schCheck)".into(),
            params: vec![],
            returns: "schCheck output".into(),
        },
        Method {
            name: "schematic.list_instances".into(),
            summary: "List all instances in the open cellview".into(),
            params: vec![],
            returns: "JSON array of instances".into(),
        },
        Method {
            name: "schematic.list_nets".into(),
            summary: "List all nets in the open cellview".into(),
            params: vec![],
            returns: "JSON array of net names".into(),
        },
        Method {
            name: "schematic.list_pins".into(),
            summary: "List all pins in the open cellview".into(),
            params: vec![],
            returns: "JSON array of pins".into(),
        },
        Method {
            name: "schematic.get_params".into(),
            summary: "Get parameters of a specific instance".into(),
            params: vec![Param {
                name: "inst".into(),
                ptype: "string".into(),
                description: "Instance name (e.g. M1)".into(),
                required: true,
            }],
            returns: "JSON object of param name→value".into(),
        },
        // ── Window ────────────────────────────────────────────────────
        Method {
            name: "window.list".into(),
            summary: "List all open Virtuoso windows".into(),
            params: vec![],
            returns: "JSON array of window names".into(),
        },
        Method {
            name: "window.screenshot".into(),
            summary: "Capture screenshot of current window".into(),
            params: vec![Param {
                name: "path".into(),
                ptype: "string".into(),
                description: "Output PNG file path".into(),
                required: true,
            }],
            returns: "file path on success".into(),
        },
        Method {
            name: "window.screenshot_by_pattern".into(),
            summary: "Capture screenshot of window matching regex pattern".into(),
            params: vec![
                Param {
                    name: "path".into(),
                    ptype: "string".into(),
                    description: "Output PNG file path".into(),
                    required: true,
                },
                Param {
                    name: "pattern".into(),
                    ptype: "string".into(),
                    description: "Regex pattern to match window name".into(),
                    required: true,
                },
            ],
            returns: "file path on success, or no-match".into(),
        },
        Method {
            name: "window.dismiss_dialog".into(),
            summary: "Dismiss the current blocking dialog".into(),
            params: vec![Param {
                name: "action".into(),
                ptype: "string".into(),
                description: "Action: 'ok' or 'cancel'".into(),
                required: false,
            }],
            returns: "action taken or no-dialog".into(),
        },
        Method {
            name: "window.get_dialog_info".into(),
            summary: "Get current dialog name without dismissing".into(),
            params: vec![],
            returns: "dialog name or null".into(),
        },
        Method {
            name: "window.dismiss_dialog_x11".into(),
            summary:
                "Dismiss blocking dialog(s) via X11 SSH bypass (works when SKILL is deadlocked)"
                    .into(),
            params: vec![
                Param {
                    name: "action".into(),
                    ptype: "string".into(),
                    description: "enter|escape|alt-y|alt-n (default: enter)".into(),
                    required: false,
                },
                Param {
                    name: "dry_run".into(),
                    ptype: "bool".into(),
                    description: "List dialogs without sending keypress".into(),
                    required: false,
                },
                Param {
                    name: "display".into(),
                    ptype: "string".into(),
                    description: "Override the detected DISPLAY".into(),
                    required: false,
                },
            ],
            returns: "{status, found, dismissed, errors, display, raw_log}".into(),
        },
        // ── Cell ─────────────────────────────────────────────────────
        Method {
            name: "cell.open".into(),
            summary: "Open a cellview".into(),
            params: vec![
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name".into(),
                    required: false,
                },
                Param {
                    name: "mode".into(),
                    ptype: "string".into(),
                    description: "Open mode: r(ead), o(verwrite), a(ppend)".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "cell.save".into(),
            summary: "Save the current cellview".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "cell.close".into(),
            summary: "Close the current cellview".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "cell.info".into(),
            summary: "Get current cellview info (lib/cell/view)".into(),
            params: vec![],
            returns: "JSON object with lib, cell, view".into(),
        },
        Method {
            name: "cell.create".into(),
            summary: "Create a new cellview".into(),
            params: vec![
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "cell.read_path".into(),
            summary: "Return the on-disk readPath of a registered OA library".into(),
            params: vec![Param {
                name: "lib".into(),
                ptype: "string".into(),
                description: "Library name (must be registered in remote cds.lib)".into(),
                required: true,
            }],
            returns: "{lib, read_path: string|null}".into(),
        },
        // ── Maestro ───────────────────────────────────────────────────
        Method {
            name: "maestro.open_session".into(),
            summary: "Open a Maestro session".into(),
            params: vec![
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name".into(),
                    required: false,
                },
            ],
            returns: "session handle string".into(),
        },
        Method {
            name: "maestro.close_session".into(),
            summary: "Close a Maestro session".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID (e.g. fnxSession4)".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.list_sessions".into(),
            summary: "List all active Maestro sessions".into(),
            params: vec![],
            returns: "JSON array of session objects".into(),
        },
        Method {
            name: "maestro.set_var".into(),
            summary: "Set a design variable".into(),
            params: vec![
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Variable name".into(),
                    required: true,
                },
                Param {
                    name: "value".into(),
                    ptype: "string".into(),
                    description: "Variable value".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.get_var".into(),
            summary: "Get a design variable".into(),
            params: vec![Param {
                name: "name".into(),
                ptype: "string".into(),
                description: "Variable name".into(),
                required: true,
            }],
            returns: "variable value string".into(),
        },
        Method {
            name: "maestro.list_vars".into(),
            summary: "List all design variables".into(),
            params: vec![],
            returns: "JSON array of {name, value}".into(),
        },
        Method {
            name: "maestro.run".into(),
            summary: "Run simulation asynchronously".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.save".into(),
            summary: "Save Maestro setup to disk".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.export".into(),
            summary: "Export results to CSV".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session ID".into(),
                    required: true,
                },
                Param {
                    name: "path".into(),
                    ptype: "string".into(),
                    description: "Output CSV file path".into(),
                    required: true,
                },
                Param {
                    name: "test_name".into(),
                    ptype: "string".into(),
                    description: "Test name (optional)".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        // ── Maestro Result Reading ──────────────────────────────────────
        Method {
            name: "maestro.open_results".into(),
            summary: "Open simulation results for a history run".into(),
            params: vec![Param {
                name: "history".into(),
                ptype: "string".into(),
                description: "History name (e.g. ExplorerRun.0)".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.close_results".into(),
            summary: "Close the currently open simulation results".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.get_result_tests".into(),
            summary: "List all test names with results".into(),
            params: vec![],
            returns: "JSON array of test names".into(),
        },
        Method {
            name: "maestro.get_result_outputs".into(),
            summary: "List all output names for a test".into(),
            params: vec![Param {
                name: "test".into(),
                ptype: "string".into(),
                description: "Test name".into(),
                required: true,
            }],
            returns: "JSON array of output names".into(),
        },
        Method {
            name: "maestro.get_output_value".into(),
            summary: "Get the value of a simulation output".into(),
            params: vec![
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Output name".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test name".into(),
                    required: true,
                },
                Param {
                    name: "corner".into(),
                    ptype: "string".into(),
                    description: "Corner name (optional)".into(),
                    required: false,
                },
            ],
            returns: "value as string".into(),
        },
        Method {
            name: "maestro.get_history_list".into(),
            summary: "List available simulation history runs".into(),
            params: vec![],
            returns: "JSON array of history names".into(),
        },
        Method {
            name: "maestro.get_analyses".into(),
            summary: "Get enabled analysis types".into(),
            params: vec![],
            returns: "analysis types string".into(),
        },
        Method {
            name: "maestro.get_outputs".into(),
            summary: "List all outputs for a test".into(),
            params: vec![Param {
                name: "test".into(),
                ptype: "string".into(),
                description: "Test name".into(),
                required: true,
            }],
            returns: "JSON array of output objects".into(),
        },
        Method {
            name: "maestro.get_sim_messages".into(),
            summary: "Get simulation log messages".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID".into(),
                required: true,
            }],
            returns: "messages string".into(),
        },
        Method {
            name: "maestro.set_analysis".into(),
            summary: "Set simulation analysis parameters".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session ID".into(),
                    required: true,
                },
                Param {
                    name: "type".into(),
                    ptype: "string".into(),
                    description: "Analysis type (e.g. ac, tran, dc)".into(),
                    required: true,
                },
                Param {
                    name: "options".into(),
                    ptype: "string".into(),
                    description: "Options alist (e.g. '((freq \"1k\"))')".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.add_output".into(),
            summary: "Add a measurement output".into(),
            params: vec![
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Output name".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test name".into(),
                    required: true,
                },
                Param {
                    name: "expr".into(),
                    ptype: "string".into(),
                    description: "Expression (e.g. bandwidth(vf(\"/out\") 3))".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.set_design".into(),
            summary: "Set the simulation design target".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session ID".into(),
                    required: true,
                },
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.save_setup".into(),
            summary: "Save the simulation setup".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.get_spec_status".into(),
            summary: "Get spec pass/fail status for an output".into(),
            params: vec![
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Output name".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test name".into(),
                    required: true,
                },
            ],
            returns: "pass/fail status string".into(),
        },
        Method {
            name: "maestro.get_current_session".into(),
            summary: "Get the current Maestro session name".into(),
            params: vec![],
            returns: "session name or null".into(),
        },
        // ── Transaction ───────────────────────────────────────────────
        Method {
            name: "tx.begin".into(),
            summary: "Begin a schematic transaction (snapshot)".into(),
            params: vec![
                Param {
                    name: "id".into(),
                    ptype: "string".into(),
                    description: "Transaction ID".into(),
                    required: true,
                },
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library name".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell name".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View name".into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "tx.commit".into(),
            summary: "Commit the current transaction".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "tx.rollback".into(),
            summary: "Rollback the current transaction".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "tx.diff".into(),
            summary: "Get the diff between current and snapshot".into(),
            params: vec![],
            returns: "JSON object with added/removed/modified lists".into(),
        },
        Method {
            name: "tx.status".into(),
            summary: "Get current transaction status".into(),
            params: vec![],
            returns: "JSON object with active/id/snapshot info".into(),
        },
        // ── File Transfer ─────────────────────────────────────────────
        Method {
            name: "file.upload".into(),
            summary: "Upload a local file to Virtuoso server".into(),
            params: vec![
                Param {
                    name: "local".into(),
                    ptype: "string".into(),
                    description: "Local file path".into(),
                    required: true,
                },
                Param {
                    name: "remote".into(),
                    ptype: "string".into(),
                    description: "Remote file path on server".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "file.download".into(),
            summary: "Download a file from Virtuoso server".into(),
            params: vec![
                Param {
                    name: "remote".into(),
                    ptype: "string".into(),
                    description: "Remote file path on server".into(),
                    required: true,
                },
                Param {
                    name: "local".into(),
                    ptype: "string".into(),
                    description: "Local destination path".into(),
                    required: true,
                },
            ],
            returns: "null on success".into(),
        },
        // ── Utility ──────────────────────────────────────────────────
        Method {
            name: "util.version".into(),
            summary: "Get Virtuoso version info".into(),
            params: vec![],
            returns: "version object with is_ic23/is_ic25 flags".into(),
        },
        Method {
            name: "util.ping".into(),
            summary: "Check connection to Virtuoso".into(),
            params: vec![],
            returns: "ok on success".into(),
        },
        Method {
            name: "util.ciw_print".into(),
            summary: "Print message to CIW console".into(),
            params: vec![Param {
                name: "message".into(),
                ptype: "string".into(),
                description: "Message to print".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "util.reconnect".into(),
            summary: "Reconnect to a session".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID".into(),
                required: true,
            }],
            returns: "ok/failed status".into(),
        },
        // ── Skill ───────────────────────────────────────────────────
        Method {
            name: "skill.exec".into(),
            summary: "Execute raw SKILL code (Admin only)".into(),
            params: vec![
                Param {
                    name: "code".into(),
                    ptype: "string".into(),
                    description: "SKILL code to execute".into(),
                    required: true,
                },
                Param {
                    name: "timeout".into(),
                    ptype: "integer".into(),
                    description: "Timeout in seconds (optional)".into(),
                    required: false,
                },
            ],
            returns: "SKILL output".into(),
        },
        Method {
            name: "skill.load".into(),
            summary: "Load a SKILL (.il) file".into(),
            params: vec![Param {
                name: "path".into(),
                ptype: "string".into(),
                description: "Path to .il file".into(),
                required: true,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "skill.eval".into(),
            summary: "Execute inline SKILL expressions (supports multi-statement)".into(),
            params: vec![
                Param {
                    name: "code".into(),
                    ptype: "string".into(),
                    description: "SKILL expression to evaluate (omit when using stdin)".into(),
                    required: false,
                },
                Param {
                    name: "stdin".into(),
                    ptype: "boolean".into(),
                    description: "Read expression from stdin instead of code argument".into(),
                    required: false,
                },
            ],
            returns: "VirtuosoResult JSON".into(),
        },
        Method {
            name: "maestro.snapshot".into(),
            summary: "Snapshot Maestro run artifacts to local directory (YAML-filtered)".into(),
            params: vec![
                Param {
                    name: "output_dir".into(),
                    ptype: "string".into(),
                    description: "Output directory path".into(),
                    required: true,
                },
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Maestro session name (optional; auto-detects)".into(),
                    required: false,
                },
                Param {
                    name: "history".into(),
                    ptype: "string".into(),
                    description: "History run name (optional; picks newest)".into(),
                    required: false,
                },
                Param {
                    name: "filter_path".into(),
                    ptype: "string".into(),
                    description: "Custom filter YAML path (optional; uses built-in)".into(),
                    required: false,
                },
            ],
            returns: "Snapshot result with files_copied count".into(),
        },
        Method {
            name: "schematic.polish_label".into(),
            summary: "Polish net labels with cosmetic presets, auto-rotation, or offset".into(),
            params: vec![
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name whose labels to polish".into(),
                    required: true,
                },
                Param {
                    name: "preset".into(),
                    ptype: "string".into(),
                    description: "Preset: 'readable' (0.125 font) or 'compact' (0.0625)".into(),
                    required: false,
                },
                Param {
                    name: "auto_rotate".into(),
                    ptype: "boolean".into(),
                    description: "Apply auto-rotation based on wire direction".into(),
                    required: false,
                },
                Param {
                    name: "offset".into(),
                    ptype: "string".into(),
                    description: "Offset: 'small' (+5), 'medium' (+10), 'large' (+20) DBU".into(),
                    required: false,
                },
            ],
            returns: "Number of labels updated".into(),
        },
    ])
}

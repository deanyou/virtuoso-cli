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
            name: "schematic.move_instance".into(),
            summary: "Move a placed instance to an absolute position".into(),
            params: vec![
                Param {
                    name: "name".into(),
                    ptype: "string".into(),
                    description: "Instance name".into(),
                    required: true,
                },
                Param {
                    name: "x".into(),
                    ptype: "number".into(),
                    description: "Target X coordinate (absolute, same frame as place)".into(),
                    required: true,
                },
                Param {
                    name: "y".into(),
                    ptype: "number".into(),
                    description: "Target Y coordinate (absolute, same frame as place)".into(),
                    required: true,
                },
                Param {
                    name: "orient".into(),
                    ptype: "string".into(),
                    description: "Absolute orientation (R0, R90, R180, R270, MY, MX, ...); omit to keep the current one".into(),
                    required: false,
                },
            ],
            returns: "{status, name, x, y, orient}".into(),
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
            name: "schematic.assign_net".into(),
            summary: "Connect an instance terminal to a named net, logically only — ERASED by the next schematic.check or GUI Check&Save (connectivity is derived from geometry); use schematic.label_term to build connectivity that lasts".into(),
            params: vec![
                Param {
                    name: "inst".into(),
                    ptype: "string".into(),
                    description: "Instance name (e.g. M0)".into(),
                    required: true,
                },
                Param {
                    name: "term".into(),
                    ptype: "string".into(),
                    description: "Instance terminal name (e.g. D, G, S, B, PLUS, MINUS)".into(),
                    required: true,
                },
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name to connect the terminal to".into(),
                    required: true,
                },
            ],
            returns: "JSON object {instance, term, net, status}".into(),
        },
        Method { name: "symbol.inspect".into(), summary: "Inspect a symbol view".into(), params: vec![Param{name:"lib".into(),ptype:"string".into(),description:"Library".into(),required:true}, Param{name:"cell".into(),ptype:"string".into(),description:"Cell".into(),required:true}, Param{name:"view".into(),ptype:"string".into(),description:"Symbol view".into(),required:false}, Param{name:"view_type".into(),ptype:"string".into(),description:"View type".into(),required:false}], returns:"Symbol inspection result".into() },
        Method { name: "symbol.generate".into(), summary: "Generate a symbol from a schematic".into(), params: vec![Param{name:"lib".into(),ptype:"string".into(),description:"Library".into(),required:true}, Param{name:"cell".into(),ptype:"string".into(),description:"Cell".into(),required:true}, Param{name:"schematic_view".into(),ptype:"string".into(),description:"Source schematic view".into(),required:false}, Param{name:"symbol_view".into(),ptype:"string".into(),description:"Target symbol view".into(),required:false}, Param{name:"sort_pins".into(),ptype:"string".into(),description:"alphanumeric or geometric".into(),required:false}], returns:"Symbol generation result".into() },
        Method { name: "library.list".into(), summary: "List registered OA libraries (read-only)".into(), params: vec![], returns: "JSON array of library names".into() },
        Method {
            name: "library.list_cells".into(),
            summary: "List the cells in a library with their views (read-only)".into(),
            params: vec![
                Param { name: "lib".into(), ptype: "string".into(), required: true, description: "Library name, as reported by library.list".into() },
                Param { name: "pattern".into(), ptype: "string".into(), required: false, description: "SKILL regular expression matched against the cell name; a plain word acts as a substring filter".into() },
            ],
            returns: "{lib, count, cells:[{name, views:[…]}]}; errors if the library does not exist, empty list if it has no cells".into(),
        },
        Method {
            name: "schematic.save".into(),
            summary: "Save the current schematic".into(),
            params: vec![],
            returns: "null on success".into(),
        },
        Method {
            name: "schematic.check".into(),
            summary: "Run schematic check (schCheck) — re-extracts connectivity from geometry, so any connection made only with schematic.assign_net is discarded here".into(),
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
        Method {
            name: "schematic.list_cdf_params".into(),
            summary: "List an instance's CDF parameters with their GUI labels, units and defaults"
                .into(),
            params: vec![Param {
                name: "inst".into(),
                ptype: "string".into(),
                description: "Instance name (e.g. M1)".into(),
                required: true,
            }],
            returns: "JSON array of {name, prompt (the GUI label), type, units, default, \
                      value, choices? (cyclic values), description?} — the authoritative \
                      parameter list for this instance's master, and the only source that \
                      covers PDK devices, which no Cadence manual documents. `description` \
                      is empty on analogLib, so read prose from libref.info and treat \
                      this as the final word on names and values."
                .into(),
        },
        Method {
            name: "schematic.set_param".into(),
            summary: "Set one CDF parameter of a specific instance".into(),
            params: vec![
                Param {
                    name: "inst".into(),
                    ptype: "string".into(),
                    description: "Instance name (e.g. M1)".into(),
                    required: true,
                },
                Param {
                    name: "param".into(),
                    ptype: "string".into(),
                    description: "Parameter name (e.g. w, l, nf, fingers)".into(),
                    required: true,
                },
                Param {
                    name: "value".into(),
                    ptype: "string".into(),
                    description: "New value (e.g. 4u)".into(),
                    required: true,
                },
            ],
            returns: "JSON object {instance, param, value, status}".into(),
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
                    description: "enter|escape|alt-y|alt-n|alt-o (default: enter)".into(),
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
                Param {
                    name: "window_id".into(),
                    ptype: "string".into(),
                    description:
                        "Dismiss only this X11 window id instead of every dialog-sized window"
                            .into(),
                    required: false,
                },
            ],
            returns: "{status, found, dismissed, errors, display, raw_log}".into(),
        },
        Method {
            name: "window.list_windows_x11".into(),
            summary: "Enumerate all Virtuoso-related X11 windows (no keypress sent)"
                .into(),
            params: vec![Param {
                name: "display".into(),
                ptype: "string".into(),
                description: "Override the detected DISPLAY".into(),
                required: false,
            }],
            returns: "{display, xauthority, windows, count}".into(),
        },
        Method {
            name: "window.dismiss_window_x11".into(),
            summary:
                "Dismiss a SPECIFIC X11 window by id (typically the dismiss_id from list_windows_x11). Bypasses the dialog-size filter."
                    .into(),
            params: vec![
                Param {
                    name: "window_id".into(),
                    ptype: "string".into(),
                    description:
                        "X11 window id (e.g. 0x2e01f16) from list_windows_x11. Required unless pid is given; wins if both are."
                            .into(),
                    required: false,
                },
                Param {
                    name: "pid".into(),
                    ptype: "integer".into(),
                    description:
                        "Resolve the window by owning PID instead. Errors if it matches zero or several windows."
                            .into(),
                    required: false,
                },
                Param {
                    name: "action".into(),
                    ptype: "string".into(),
                    description: "enter|escape|alt-y|alt-n|alt-o (default: enter)".into(),
                    required: false,
                },
                Param {
                    name: "display".into(),
                    ptype: "string".into(),
                    description: "Override the detected DISPLAY".into(),
                    required: false,
                },
            ],
            returns: "{status, dismissed, errors, display, raw_log}".into(),
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
            params: vec![Param {
                name: "save".into(),
                ptype: "boolean".into(),
                description: "Save before closing (default true). Pass false to discard \
                              edits (silently, via dbReopen to read mode). One of the two must \
                              happen: closing a modified cellview otherwise pops a modal that \
                              freezes the bridge."
                    .into(),
                required: false,
            }],
            returns: "null on success".into(),
        },
        Method {
            name: "cell.info".into(),
            summary: "Get current cellview info (lib/cell/view)".into(),
            params: vec![],
            returns: "JSON object with lib, cell, view".into(),
        },
        Method {
            name: "cell.list_open".into(),
            summary: "List every cellview this Virtuoso holds open, and whether a window shows it"
                .into(),
            params: vec![],
            returns: "JSON array of {lib, cell, view, mode, window}. mode \"a\" holds an edit \
                      lock, \"r\" does not; a row with mode \"a\" and window false is an orphan \
                      that no GUI action can close and that locks a human out of that cell."
                .into(),
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
                    description: "View name (default schematic)".into(),
                    required: false,
                },
                Param {
                    name: "view_type".into(),
                    ptype: "string".into(),
                    description:
                        "DFII cellViewType, needed to create the view. Inferred for schematic, \
                         symbol (schematicSymbol), layout (maskLayout) and netlist; required \
                         for any other view name."
                            .into(),
                    required: false,
                },
            ],
            returns: "{lib, cell, view, view_type}; errors if the view already exists".into(),
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
                Param {
                    name: "mode".into(),
                    ptype: "string".into(),
                    description: "\"r\" (default) opens read-only and takes NO edit lock, so a \
                                  human keeps edit access to the cellview; the setup is still \
                                  fully readable. \"a\" opens editable and TAKES the edit lock — \
                                  and a SKILL-opened session has no window, so that locks a human \
                                  out of the cell with nothing to click. Prefer \"r\" plus \
                                  maestro.set_session_mode around the writes. Note \"a\" is \
                                  required to open a maestro view that does not exist yet, since \
                                  maeOpenSetup creates it and cannot create in read mode."
                        .into(),
                    required: false,
                },
            ],
            returns: "session handle string".into(),
        },
        Method {
            name: "maestro.set_session_mode".into(),
            summary: "Switch an open Maestro session between read-only and editable".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session handle from maestro.open_session".into(),
                    required: true,
                },
                Param {
                    name: "mode".into(),
                    ptype: "string".into(),
                    description: "\"a\" takes the edit lock (maeMakeEditable), \"r\" hands it \
                                  back (maeMakeReadonly). Both act in place with no reopen, so \
                                  hold \"a\" only across the writes themselves."
                        .into(),
                    required: true,
                },
            ],
            returns: "{status, session, mode} — the mode now in effect".into(),
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
            name: "maestro.list_tests".into(),
            summary: "List the test names in a Maestro session".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID (e.g. fnxSession4)".into(),
                required: true,
            }],
            returns: "JSON array of test-name strings".into(),
        },
        Method {
            name: "maestro.list_corners".into(),
            summary: "List the corner and corner-group names in a Maestro session's setup".into(),
            params: vec![Param {
                name: "session".into(),
                ptype: "string".into(),
                description: "Session ID (e.g. fnxSession4)".into(),
                required: true,
            }],
            returns: "JSON array of corner names as they appear in ADE Assembler (e.g. C0, \
                      Nominal) — these are setup labels, not PDK model sections; \
                      maestro.create_corner_netlist takes one of these. Empty array if the \
                      setup defines no corners; errors if the session does not exist"
                .into(),
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
            summary: "List all Assembler-global design variables".into(),
            params: vec![],
            returns: "JSON array of {name, value}".into(),
        },
        Method {
            name: "maestro.delete_var".into(),
            summary: "Delete an Assembler-global design variable".into(),
            params: vec![Param {
                name: "name".into(),
                ptype: "string".into(),
                description: "Variable name".into(),
                required: true,
            }],
            returns: "{status}".into(),
        },
        Method {
            name: "maestro.delete_output".into(),
            summary: "Delete an output from a test's setup".into(),
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
            returns: "{status}".into(),
        },
        Method {
            name: "maestro.delete_analysis".into(),
            summary: "Delete an analysis from the current test".into(),
            params: vec![Param {
                name: "analysis".into(),
                ptype: "string".into(),
                description: "Analysis type (e.g. ac, dc, tran)".into(),
                required: true,
            }],
            returns: "{status}".into(),
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
            summary: "Get enabled analysis types for a test".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session ID".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test name. Optional only when the session holds exactly \
                                  one test; with several it is required, because analyses \
                                  are per-test and picking one silently would report another \
                                  test's setup."
                        .into(),
                    required: false,
                },
            ],
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
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test the analysis belongs to. Optional only when the \
                                  session holds exactly one test; with several it is \
                                  required, since maeSetAnalysis is per-test and would \
                                  otherwise configure whichever test comes first."
                        .into(),
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
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test to retarget; omit to set the design for every test \
                                  in the session"
                        .into(),
                    required: false,
                },
            ],
            returns: "null on success".into(),
        },
        Method {
            name: "maestro.create_test".into(),
            summary: "Create a test in an open Maestro session".into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Session ID".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Name for the new test".into(),
                    required: true,
                },
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Library of the design under test".into(),
                    required: true,
                },
                Param {
                    name: "cell".into(),
                    ptype: "string".into(),
                    description: "Cell of the design under test".into(),
                    required: true,
                },
                Param {
                    name: "view".into(),
                    ptype: "string".into(),
                    description: "View of the design under test (default \"schematic\")".into(),
                    required: false,
                },
                Param {
                    name: "simulator".into(),
                    ptype: "string".into(),
                    description: "Simulator name (default \"spectre\")".into(),
                    required: false,
                },
            ],
            returns: "{status, test, lib, cell, view, simulator}".into(),
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
            summary: "Load a SKILL (.il/.ils) file (Admin only)".into(),
            params: vec![Param {
                name: "path".into(),
                ptype: "string".into(),
                description: "Local file path; uploaded when using an SSH tunnel".into(),
                required: true,
            }],
            returns: "status, output, loaded_path; error if loading fails".into(),
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
        // ── SKILL Finder ─────────────────────────────────────────────
        Method {
            name: "skill.find".into(),
            summary: "Search SKILL function database (fuzzy/prefix/suffix/exact/regex)".into(),
            params: vec![
                Param {
                    name: "query".into(),
                    ptype: "string".into(),
                    description: "Search string".into(),
                    required: true,
                },
                Param {
                    name: "mode".into(),
                    ptype: "string".into(),
                    description: "Search mode: fuzzy (default), prefix, suffix, exact, regex".into(),
                    required: false,
                },
                Param {
                    name: "limit".into(),
                    ptype: "integer".into(),
                    description: "Max results (default: 50)".into(),
                    required: false,
                },
                Param {
                    name: "include_desc".into(),
                    ptype: "boolean".into(),
                    description: "Also search in description field".into(),
                    required: false,
                },
                Param {
                    name: "refresh".into(),
                    ptype: "boolean".into(),
                    description: "Force refresh remote cache".into(),
                    required: false,
                },
            ],
            returns: "{query, mode, count, entries[]}".into(),
        },
        Method {
            name: "skill.info".into(),
            summary: "Look up a SKILL function's signature and description in the \
                      local .fnd databases (no Virtuoso, no Admin)"
            .into(),
            params: vec![Param {
                name: "func".into(),
                ptype: "string".into(),
                description: "Function name".into(),
                required: true,
            }],
            returns: "{func_name, found, name, syntax, description, source} | \
                      {func_name, found:false, reason, entries_loaded, suggestions[], hint}"
            .into(),
        },
        Method {
            name: "skill.sync".into(),
            summary: "Sync SKILL Finder cache from remote host".into(),
            params: vec![Param {
                name: "host".into(),
                ptype: "string".into(),
                description: "Remote host to sync from (optional; uses VB_REMOTE_HOST)".into(),
                required: false,
            }],
            returns: "{status, cached_count}".into(),
        },
        Method {
            name: "skill.cache".into(),
            summary: "Show or clear local SKILL Finder cache".into(),
            params: vec![
                Param {
                    name: "host".into(),
                    ptype: "string".into(),
                    description: "Host to show cache for (optional)".into(),
                    required: false,
                },
                Param {
                    name: "clear".into(),
                    ptype: "boolean".into(),
                    description: "Clear the cache for this host".into(),
                    required: false,
                },
            ],
            returns: "{cache_dir, file_count, modified}".into(),
        },
        // Virtuoso library references. Local documentation reads — no SKILL is
        // executed and Virtuoso is never contacted, hence no capability gate.
        // analogLib and basic are parsed; the other registered libraries report
        // why they yield nothing rather than coming back empty.
        Method {
            name: "libref.list".into(),
            summary: "List documented library cells — analogLib (vsin, idc, cap, nmos4, ...) \
                      and basic (ipin, opin, gnd, vdd, ...)"
                .into(),
            params: vec![
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Virtuoso library to restrict to: analogLib (sources, \
                                  passives, actives), basic (pins, supplies). rfLib, \
                                  fBlockLib, pcLib and ahdlLib are registered but not \
                                  parsed — they answer with the reason and what to use \
                                  instead, never with an empty list. Omit to search all."
                        .into(),
                    required: false,
                },
                Param {
                    name: "category".into(),
                    ptype: "string".into(),
                    description: "Filter by category substring, e.g. 'Passive', 'Sources', 'Pins'"
                        .into(),
                    required: false,
                },
                Param {
                    name: "refresh".into(),
                    ptype: "boolean".into(),
                    description: "Force re-sync of the cached documentation".into(),
                    required: false,
                },
            ],
            returns: "{lib, count, symbols[{lib, name, cell ('analogLib/vsin' — what \
                       schematic.place takes), category, title, param_count, primitives[], \
                       description?}], symbols_loaded, symbols_indexed, source, \
                       libraries[{lib, state, symbols}], libraries_without_symbols?}"
            .into(),
        },
        Method {
            name: "libref.info".into(),
            summary: "Full CDF parameter table for one library cell — the lookup to run \
                      BEFORE schematic.set_param, so parameter names are read, not guessed"
            .into(),
            params: vec![
                Param {
                    name: "symbol".into(),
                    ptype: "string".into(),
                    description: "Cell name, e.g. vsin, idc, cap, res, ipin, gnd. Optional \
                                  when `lib` is given: that form asks about the library \
                                  itself and returns its documentation status."
                        .into(),
                    required: false,
                },
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Virtuoso library to restrict to: analogLib (sources, \
                                  passives, actives), basic (pins, supplies). rfLib, \
                                  fBlockLib, pcLib and ahdlLib are registered but not \
                                  parsed — they answer with the reason and what to use \
                                  instead, never with an empty list. Omit to search all."
                        .into(),
                    required: false,
                },
                Param {
                    name: "refresh".into(),
                    ptype: "boolean".into(),
                    description: "Force re-sync of the cached documentation".into(),
                    required: false,
                },
            ],
            returns: "{lib, symbol, cell, found, category, title, description, primitives[], \
                       param_count, params[{name, label, spectre, description, default, \
                       expands_to?}], note?, verify_with} | {symbol, found:true, \
                       ambiguous:true, libraries[], matches[]} when the name is in more \
                       than one library | {symbol, found:false, reason, suggestions[], hint} | \
                       {lib, found:false, state:'no_parser'|'no_manual'|'docs_missing', reason}"
            .into(),
        },
        Method {
            name: "libref.find".into(),
            summary: "Search CDF parameters or cells — 'amplitude' finds va (Amplitude 1 \
                      (Vpk)), vaDBm, ia, each with the cell it belongs to"
            .into(),
            params: vec![
                Param {
                    name: "query".into(),
                    ptype: "string".into(),
                    description: "Search string: a CDF name, a GUI label, or a plain word".into(),
                    required: true,
                },
                Param {
                    name: "lib".into(),
                    ptype: "string".into(),
                    description: "Virtuoso library to restrict to: analogLib (sources, \
                                  passives, actives), basic (pins, supplies). rfLib, \
                                  fBlockLib, pcLib and ahdlLib are registered but not \
                                  parsed — they answer with the reason and what to use \
                                  instead, never with an empty list. Omit to search all."
                        .into(),
                    required: false,
                },
                Param {
                    name: "scope".into(),
                    ptype: "string".into(),
                    description: "params (default), symbols, or both".into(),
                    required: false,
                },
                Param {
                    name: "mode".into(),
                    ptype: "string".into(),
                    description: "Search mode: fuzzy (default), prefix, suffix, exact, regex"
                        .into(),
                    required: false,
                },
                Param {
                    name: "limit".into(),
                    ptype: "integer".into(),
                    description: "Max results (default: 50)".into(),
                    required: false,
                },
                Param {
                    name: "refresh".into(),
                    ptype: "boolean".into(),
                    description: "Force re-sync of the cached documentation".into(),
                    required: false,
                },
            ],
            returns: "{query, mode, scope, lib, param_count (total matches, not the \
                       truncated page), params_shown, params_truncated?, \
                       params[{lib, symbol, cell, name, label, spectre, description, \
                       default}], symbol_count, symbols_shown, symbols_truncated?, symbols[]}"
            .into(),
        },
        Method {
            name: "sim.check_license".into(),
            summary: "Check Spectre license availability and version".into(),
            params: vec![],
            returns: "{ok, remote, version, spectre_path, licenses[]}".into(),
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
            name: "maestro.create_corner_netlist".into(),
            summary:
                "Export a single corner's netlist for a Maestro session (remote temp dir, download to local)"
                    .into(),
            params: vec![
                Param {
                    name: "session".into(),
                    ptype: "string".into(),
                    description: "Maestro session name (e.g. fnxSession4)".into(),
                    required: true,
                },
                Param {
                    name: "test".into(),
                    ptype: "string".into(),
                    description: "Test name (e.g. AC)".into(),
                    required: true,
                },
                Param {
                    name: "corner".into(),
                    ptype: "string".into(),
                    description: "Corner name as it appears in the Assembler setup (e.g. C0, \
                                  Nominal) — list them with maestro.list_corners. NOT a PDK \
                                  model section: passing tt/ss/ff fails with a bare nil"
                        .into(),
                    required: true,
                },
                Param {
                    name: "output_dir".into(),
                    ptype: "string".into(),
                    description: "Local destination directory for downloaded netlist files".into(),
                    required: true,
                },
            ],
            returns:
                "JSON {status, session, test, corner, output_dir, remote_dir, files[], remote_cleaned}"
                    .into(),
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
        Method {
            name: "schematic.net_stub".into(),
            summary: "Create a short labeled net stub in a given direction".into(),
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
                    description: "X origin in DBU".into(),
                    required: true,
                },
                Param {
                    name: "y".into(),
                    ptype: "integer".into(),
                    description: "Y origin in DBU".into(),
                    required: true,
                },
                Param {
                    name: "direction".into(),
                    ptype: "string".into(),
                    description: "Direction: right (default), left, up, down".into(),
                    required: false,
                },
                Param {
                    name: "length".into(),
                    ptype: "number".into(),
                    description: "Stub length in grid units (default 0.5)".into(),
                    required: false,
                },
                Param {
                    name: "cosmetic".into(),
                    ptype: "string".into(),
                    description: "'default' (0.0625, centerCenter) or 'clean' (0.125, lowerCenter)".into(),
                    required: false,
                },
            ],
            returns: "{status, net, direction, origin}".into(),
        },
        Method {
            name: "schematic.label_term".into(),
            summary: "Draw a wire stub out of an instance terminal and name it; nets merge by label, so this is the coordinate-free way to build connectivity that survives schematic.check".into(),
            params: vec![
                Param {
                    name: "inst".into(),
                    ptype: "string".into(),
                    description: "Instance name (e.g. M1)".into(),
                    required: true,
                },
                Param {
                    name: "term".into(),
                    ptype: "string".into(),
                    description: "Terminal name on the master (e.g. D, G, S, B, PLUS, VDD)".into(),
                    required: true,
                },
                Param {
                    name: "net".into(),
                    ptype: "string".into(),
                    description: "Net name to label the stub with".into(),
                    required: true,
                },
                Param {
                    name: "cosmetic".into(),
                    ptype: "string".into(),
                    description: "'default' (0.0625 font) or 'clean' (0.125 font)".into(),
                    required: false,
                },
                Param {
                    name: "auto_rotate".into(),
                    ptype: "boolean".into(),
                    description: "Rotate labels on vertical stubs to R90; default leaves them upright".into(),
                    required: false,
                },
            ],
            returns: "{status, instance, terminal, net, output}".into(),
        },
    ])
}

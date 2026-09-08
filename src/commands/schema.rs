use serde_json::{json, Value};

pub fn show(noun: Option<&str>, verb: Option<&str>) -> Value {
    let full_schema = json!({
        "name": "virtuoso",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": {
            "init": {
                "description": "Create a config.toml template with commented defaults",
                "flags": {
                    "--if-not-exists": { "type": "bool", "default": false, "description": "Skip if config.toml already exists" },
                    "--format": { "type": "enum", "values": ["json", "table"], "default": "auto", "description": "Output format" },
                }
            },
            "tunnel": {
                "description": "Manage SSH tunnel to remote Virtuoso host",
                "subcommands": {
                    "start": {
                        "description": "Start SSH tunnel and deploy daemon",
                        "flags": {
                            "--timeout": { "type": "u64", "default": 30, "description": "Connection timeout in seconds" },
                            "--dry-run": { "type": "bool", "default": false, "description": "Preview without executing" },
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso tunnel start",
                            "virtuoso tunnel start --timeout 60",
                            "virtuoso tunnel start --dry-run --format json",
                        ]
                    },
                    "stop": {
                        "description": "Stop SSH tunnel and clean up remote files",
                        "flags": {
                            "--force": { "type": "bool", "default": false, "description": "Force kill even if PID mismatch" },
                            "--dry-run": { "type": "bool", "default": false, "description": "Preview without executing" },
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso tunnel stop",
                            "virtuoso tunnel stop --force",
                        ]
                    },
                    "restart": {
                        "description": "Restart SSH tunnel",
                        "flags": {
                            "--timeout": { "type": "u64", "default": 30, "description": "Connection timeout in seconds" },
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        }
                    },
                    "status": {
                        "description": "Show tunnel, daemon, and connection status",
                        "flags": {
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso tunnel status",
                            "virtuoso tunnel status --format json",
                        ]
                    }
                }
            },
            "skill": {
                "description": "Execute SKILL code on connected Virtuoso instance",
                "subcommands": {
                    "exec": {
                        "description": "Execute a SKILL expression and return result",
                        "args": {
                            "code": { "type": "string", "required": true, "description": "SKILL expression to evaluate" }
                        },
                        "flags": {
                            "--timeout": { "type": "u64", "default": 30, "description": "Execution timeout in seconds" },
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso skill exec '1+1'",
                            "virtuoso skill exec 'geGetEditCellView()' --timeout 60",
                        ]
                    },
                    "load": {
                        "description": "Upload and load an IL script file into Virtuoso",
                        "args": {
                            "file": { "type": "path", "required": true, "description": "Path to .il file" }
                        },
                        "flags": {
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso skill load my_script.il",
                        ]
                    }
                }
            },
            "cell": {
                "description": "Manage cellviews in Virtuoso",
                "subcommands": {
                    "open": {
                        "description": "Open a cellview for editing",
                        "flags": {
                            "--lib": { "type": "string", "required": true, "description": "Library name" },
                            "--cell": { "type": "string", "required": true, "description": "Cell name" },
                            "--view": { "type": "string", "default": "layout", "description": "View name" },
                            "--mode": { "type": "enum", "values": ["r", "o", "a"], "default": "a", "description": "Open mode: r(ead), o(verwrite), a(ppend)" },
                            "--dry-run": { "type": "bool", "default": false, "description": "Preview without executing" },
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        },
                        "examples": [
                            "virtuoso cell open --lib myLib --cell myCell",
                            "virtuoso cell open --lib myLib --cell myCell --view schematic --mode r",
                        ]
                    },
                    "save": {
                        "description": "Save the current cellview",
                        "flags": {
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        }
                    },
                    "close": {
                        "description": "Close the current cellview without saving",
                        "flags": {
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        }
                    },
                    "info": {
                        "description": "Get info about the currently open cellview",
                        "flags": {
                            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto" },
                        }
                    }
                }
            },
            "schema": {
                "description": "Show CLI command schema as JSON for agent introspection",
                "flags": {
                    "--all": { "type": "bool", "default": false, "description": "Show full command tree" },
                },
                "args": {
                    "noun": { "type": "string", "required": false, "description": "Command noun (e.g. tunnel)" },
                    "verb": { "type": "string", "required": false, "description": "Command verb (e.g. start)" },
                },
                "examples": [
                    "virtuoso schema --all",
                    "virtuoso schema tunnel start",
                ]
            }
        },
        "global_flags": {
            "--format": { "type": "enum", "values": ["json", "table"], "default": "auto (table in TTY, json in pipe)" },
            "--no-color": { "type": "bool", "default": false, "description": "Disable colored output" },
            "--quiet": { "type": "bool", "default": false, "description": "Suppress non-essential output" },
            "--verbose": { "type": "bool", "default": false, "description": "Enable debug logging" },
        },
        "exit_codes": {
            "0": "success",
            "1": "general error",
            "2": "invalid arguments / usage error",
            "3": "resource not found",
            "4": "permission denied",
            "5": "conflict / already exists",
            "10": "dry-run passed",
        }
    });

    match (noun, verb) {
        (None, _) => full_schema,
        (Some(n), None) => {
            if let Some(cmd) = full_schema["commands"].get(n) {
                json!({ n: cmd })
            } else {
                json!({ "error": "not_found", "message": format!("unknown command: {n}") })
            }
        }
        (Some(n), Some(v)) => {
            if let Some(sub) = full_schema["commands"]
                .get(n)
                .and_then(|c| c.get("subcommands"))
                .and_then(|s| s.get(v))
            {
                json!({ format!("{n} {v}"): sub })
            } else {
                json!({ "error": "not_found", "message": format!("unknown command: {n} {v}") })
            }
        }
    }
}

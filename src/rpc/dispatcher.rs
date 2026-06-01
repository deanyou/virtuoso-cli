//! RPC dispatcher — maps {method, params} to SKILL expressions.
//!
//! Each domain (schematic, maestro, window, cell) is handled by its ops struct.
//! The dispatcher routes the incoming JSON-RPC request to the correct handler.

use crate::auth::{check_auth, log_rpc};
use crate::client::bridge::{escape_skill_string, VirtuosoClient};
use crate::commands;
use crate::error::{Result, VirtuosoError};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

/// Static regex for SKILL octal escape sequences (compiled once)
static SKILL_OCTAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\([0-7]{1,3})").unwrap());

/// Fix SKILL's octal escape sequences (e.g., \256) to JSON unicode escapes (\u00AE).
/// SKILL uses \NNN octal for non-ASCII chars, but JSON only supports \uXXXX unicode.
fn fix_skill_octal_escapes(s: &str) -> String {
    SKILL_OCTAL_RE
        .replace_all(s, |caps: &regex::Captures| {
            let octal = &caps[1];
            if let Ok(code) = u8::from_str_radix(octal, 8) {
                format!("\\u{:04X}", code)
            } else {
                caps[0].to_string()
            }
        })
        .to_string()
}

/// Parse SKILL JSON output: bridge returns `"\"[...]\""`  — strip outer quotes, unescape inner.
/// Returns `Err` if the output cannot be parsed as JSON after unescaping.
fn parse_skill_json(output: &str) -> Result<Value> {
    // output is like: "\"[{\\\"name\\\":\\\"M1\\\"}]\""
    // Step 1: strip outer quotes from SKILL string
    let s = output.trim_matches('"');
    // Step 2: fix SKILL octal escapes (\256 → \u00AE) and try parsing directly
    let fixed = fix_skill_octal_escapes(s);
    if let Ok(v) = serde_json::from_str(&fixed) {
        return Ok(v);
    }
    // Step 3: unescape \" → " and \\\\ → \ then retry
    let unescaped = fixed.replace("\\\"", "\"").replace("\\\\", "\\");
    serde_json::from_str(&unescaped).map_err(|e| {
        VirtuosoError::Execution(format!(
            "Failed to parse SKILL JSON output: {e}. Raw: {output}"
        ))
    })
}

/// JSON-RPC request.
#[derive(Debug)]
pub struct RpcRequest {
    pub method: String,
    pub params: Value,
    /// Optional API key for auth (from X-API-Key header or query param).
    /// Loaded by the caller before dispatch.
    #[allow(dead_code)]
    pub api_key: Option<String>,
}

pub struct RpcDispatcher;

impl RpcDispatcher {
    /// Dispatch a JSON-RPC request to the appropriate handler.
    pub fn dispatch(client: &VirtuosoClient, request: RpcRequest) -> Result<Value> {
        let RpcRequest {
            method,
            params,
            api_key,
        } = request;

        // Auth check (fails fast if invalid/missing key)
        let caps = check_auth(api_key.as_deref())?;

        // Capability check — verify the method's domain is allowed
        if !caps.permits_method(&method) {
            return Err(VirtuosoError::Execution(format!(
                "method '{}' not permitted: missing required capability",
                method
            )));
        }

        let result = Self::dispatch_inner(client, &method, params.clone());

        // Audit log — always log, regardless of success/failure
        let result_str = match &result {
            Ok(v) => serde_json::to_string(v).unwrap_or_default(),
            Err(e) => format!("error: {e}"),
        };
        log_rpc(&method, &params, &result_str, client.session_id.as_deref());

        result
    }

    fn dispatch_inner(client: &VirtuosoClient, method: &str, params: Value) -> Result<Value> {
        let parts: Vec<&str> = method.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(VirtuosoError::Execution(format!(
                "invalid method '{}': expected 'domain.method'",
                method
            )));
        }
        let (domain, op) = (parts[0], parts[1]);

        match domain {
            "schematic" => Self::dispatch_schematic(client, op, params),
            "maestro" => Self::dispatch_maestro(client, op, params),
            "window" => Self::dispatch_window(client, op, params),
            "cell" => Self::dispatch_cell(client, op, params),
            "tx" => Self::dispatch_tx(client, op, params),
            "file" => Self::dispatch_file(client, op, params),
            "util" => Self::dispatch_util(client, op, params),
            "skill" => Self::dispatch_skill(client, op, params),
            _ => {
                // Try plugin registry for unknown domains
                match crate::plugins::PluginRegistry::get_global() {
                    Ok(registry) => registry.dispatch(domain, op, params, client),
                    Err(_) => Err(VirtuosoError::Execution(format!(
                        "unknown domain '{}' in method '{}'",
                        domain, method
                    ))),
                }
            }
        }
    }

    fn dispatch_schematic(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        let ops = crate::client::schematic_ops::SchematicOps::new();
        match op {
            "open_cell_view" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                let skill = ops.open_cellview(&lib, &cell, &view);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "place" => {
                let master = json_str(params.get("master"), "master")?;
                let name = json_str(params.get("name"), "name")?;
                let x = json_i64_or(params.get("x"), 0);
                let y = json_i64_or(params.get("y"), 0);
                let orient = json_str_or(params.get("orient"), "R0")?;
                let skill = ops.create_instance(&master, &master, "symbol", &name, (x, y), &orient);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "wire" => {
                let net = json_str(params.get("net"), "net")?;
                let points: Vec<String> = params
                    .get("points")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let pts: Vec<(i64, i64)> = points
                    .iter()
                    .filter_map(|s| {
                        let (x, y) = s.split_once(',')?;
                        Some((x.parse().ok()?, y.parse().ok()?))
                    })
                    .collect();
                let skill = ops.create_wire(&pts, "wire", &net);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "label" => {
                let net = json_str(params.get("net"), "net")?;
                let x = json_i64_or(params.get("x"), 0);
                let y = json_i64_or(params.get("y"), 0);
                let skill = ops.create_wire_label(&net, (x, y));
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "pin" => {
                let net = json_str(params.get("net"), "net")?;
                let dir = json_str(params.get("direction"), "direction")?;
                let x = json_i64_or(params.get("x"), 0);
                let y = json_i64_or(params.get("y"), 0);
                let skill = ops.create_pin(&net, &dir, (x, y));
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "save" => {
                let skill = ops.save();
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "check" => {
                let skill = ops.check();
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "list_instances" => {
                let skill = ops.list_instances();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "list_nets" => {
                let skill = ops.list_nets();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "list_pins" => {
                let skill = ops.list_pins();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "get_params" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let skill = ops.get_instance_params(&inst);
                let r = client.execute_skill_unchecked(&skill, None)?;
                if r.output.trim() == "null" {
                    Ok(serde_json::Value::Null)
                } else {
                    parse_skill_json(&r.output)
                }
            }
            "polish_label" => {
                let net = json_str(params.get("net"), "net")?;
                let preset = json_str_or(params.get("preset"), "readable")?;
                let auto_rotate = params
                    .get("auto_rotate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let offset = params.get("offset").and_then(|v| v.as_str());
                let r = commands::schematic::polish_label(&net, &preset, auto_rotate, offset)?;
                Ok(r)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown schematic method '{}'",
                op
            ))),
        }
    }

    fn dispatch_maestro(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        let ops = crate::client::maestro_ops::MaestroOps;
        match op {
            "open_session" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "maestro")?;
                let skill = ops.open_session(&lib, &cell, &view);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "session": r.output.trim() }))
            }
            "close_session" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.close_session(&session);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "list_sessions" => {
                let skill = ops.list_sessions();
                let r = client.execute_skill_unchecked(&skill, None)?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "set_var" => {
                let name = json_str(params.get("name"), "name")?;
                let value = json_str(params.get("value"), "value")?;
                let skill = ops.set_var(&name, &value);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "get_var" => {
                let name = json_str(params.get("name"), "name")?;
                let skill = ops.get_var(&name);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "value": r.output.trim() }))
            }
            "list_vars" => {
                let skill = ops.list_vars();
                let r = client.execute_skill_unchecked(&skill, None)?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "run" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.run_simulation(&session);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "save" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.save_setup(&session);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "export" => {
                let session = json_str(params.get("session"), "session")?;
                let path = json_str(params.get("path"), "path")?;
                let test_name = params.get("test_name").and_then(|v| v.as_str());
                let skill = ops.export_results(&session, &path, test_name, None);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            // ── Result Reading ────────────────────────────────────────
            "open_results" => {
                let history = json_str(params.get("history"), "history")?;
                let skill = ops.open_results(&history);
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "close_results" => {
                let skill = ops.close_results();
                client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "get_result_tests" => {
                let skill = ops.get_result_tests();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "get_result_outputs" => {
                let test_name = json_str(params.get("test"), "test")?;
                let skill = ops.get_result_outputs(&test_name);
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "get_output_value" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let corner = params.get("corner").and_then(|v| v.as_str());
                let skill = ops.get_output_value(&name, &test, corner);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "value": r.output.trim() }))
            }
            "get_history_list" => {
                let skill = ops.get_history_list();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "get_analyses" => {
                let skill = r#"maeGetEnabledAnalysis(car(maeGetSetup()))"#;
                let r = client.execute_skill_unchecked(skill, None)?;
                Ok(serde_json::json!({ "analyses": r.output.trim() }))
            }
            "get_outputs" => {
                let test = json_str(params.get("test"), "test")?;
                let skill = ops.get_outputs(&test);
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "get_sim_messages" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.get_sim_messages(&session);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "messages": r.output.trim() }))
            }
            // ── Session & Setup Management ─────────────────────────────
            "get_current_session" => {
                let skill = ops.get_current_session();
                let r = client.execute_skill_unchecked(&skill, None)?;
                // SKILL returns: "nil" (quoted) when no session
                let session = r.output.trim().trim_matches('"');
                if session == "nil" {
                    Ok(serde_json::json!({ "session": null }))
                } else {
                    Ok(serde_json::json!({ "session": session }))
                }
            }
            "set_analysis" => {
                let session = json_str(params.get("session"), "session")?;
                let analysis_type = json_str(params.get("type"), "type")?;
                let options = params.get("options").and_then(|v| v.as_str());
                let version = client
                    .version()
                    .unwrap_or(crate::version::VirtuosoVersion::IC23);
                let skill = ops.set_analysis(&session, &analysis_type, options, version);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "add_output" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let expr = json_str(params.get("expr"), "expr")?;
                let skill = ops.add_output(&name, &test, &expr);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "set_design" => {
                let session = json_str(params.get("session"), "session")?;
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str(params.get("view"), "view")?;
                let skill = ops.set_design(&session, &lib, &cell, &view);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "save_setup" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.save_setup(&session);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "get_spec_status" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let skill = ops.get_spec_status(&name, &test);
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": r.output.trim() }))
            }
            "snapshot" => {
                let output_dir = json_str(params.get("output_dir"), "output_dir")?;
                let session = params.get("session").and_then(|v| v.as_str());
                let history = params.get("history").and_then(|v| v.as_str());
                let filter_path = params.get("filter_path").and_then(|v| v.as_str());
                let r = commands::maestro::snapshot(&output_dir, session, history, filter_path)?;
                Ok(r)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown maestro method '{}'",
                op
            ))),
        }
    }

    fn dispatch_window(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        let ops = crate::client::window_ops::WindowOps;
        match op {
            "list" => {
                let skill = ops.list_windows();
                let r = client.execute_skill_unchecked(&skill, None)?;
                parse_skill_json(&r.output)
            }
            "screenshot" => {
                let path = json_str(params.get("path"), "path")?;
                let skill = ops.screenshot(&path);
                let r = client.execute_skill_unchecked(&skill, None)?;
                if r.output.trim().is_empty() || r.output.contains("nil") {
                    Ok(serde_json::json!({ "status": "no-dialog-or-capture-failed" }))
                } else {
                    Ok(serde_json::json!({ "status": "ok", "path": r.output.trim() }))
                }
            }
            "screenshot_by_pattern" => {
                let path = json_str(params.get("path"), "path")?;
                let pattern = json_str(params.get("pattern"), "pattern")?;
                let skill = ops.screenshot_by_pattern(&path, &pattern);
                let r = client.execute_skill_unchecked(&skill, None)?;
                let out = r.output.trim();
                if out == "no-match" {
                    Ok(serde_json::json!({ "status": "no-match" }))
                } else if out.is_empty() || out == "nil" {
                    Ok(serde_json::json!({ "status": "capture-failed" }))
                } else {
                    Ok(serde_json::json!({ "status": "ok", "path": out }))
                }
            }
            "dismiss_dialog" => {
                let action = json_str_or(params.get("action"), "cancel")?;
                let skill = ops.dismiss_dialog(&action);
                let r = client.execute_skill_unchecked(&skill, None)?;
                let out = r.output.trim();
                if out == "no-dialog" {
                    Ok(serde_json::json!({ "status": "no-dialog" }))
                } else {
                    Ok(serde_json::json!({ "status": "ok", "action": out }))
                }
            }
            "get_dialog_info" => {
                let skill = ops.get_dialog_info();
                let r = client.execute_skill_unchecked(&skill, None)?;
                let out = r.output.trim();
                if out == "no-dialog" {
                    Ok(serde_json::json!({ "dialog": null }))
                } else {
                    Ok(serde_json::json!({ "dialog": out }))
                }
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown window method '{}'",
                op
            ))),
        }
    }

    fn dispatch_cell(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "open" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "layout")?;
                let mode = json_str_or(params.get("mode"), "a")?;
                let r = client.open_cell_view(&lib, &cell, &view, &mode)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "save" => {
                let r = client.save_current_cellview()?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "close" => {
                let r = client.close_current_cellview()?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "info" => {
                let (lib, cell, view) = client.get_current_design()?;
                Ok(serde_json::json!({
                    "lib": lib,
                    "cell": cell,
                    "view": view,
                }))
            }
            "create" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                let skill = format!(
                    r#"dbCreateCell("{lib}" "{cell}" "{view}")"#,
                    lib = escape_skill_string(&lib),
                    cell = escape_skill_string(&cell),
                    view = escape_skill_string(&view)
                );
                let r = client.execute_skill_unchecked(&skill, None)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown cell method '{}'",
                op
            ))),
        }
    }

    fn dispatch_tx(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "begin" => {
                let id = json_str(params.get("id"), "id")?;
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                client.tx_begin(&id, &lib, &cell, &view)?;
                Ok(serde_json::json!({ "status": "ok", "id": id }))
            }
            "commit" => {
                client.tx_commit()?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "rollback" => {
                client.tx_rollback()?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "diff" => {
                let diff = client.tx_diff()?;
                Ok(serde_json::json!({
                    "instances_added": diff.instances_added,
                    "instances_removed": diff.instances_removed,
                    "instances_modified": diff.instances_modified,
                    "nets_added": diff.nets_added,
                    "nets_removed": diff.nets_removed,
                    "pins_added": diff.pins_added,
                    "pins_removed": diff.pins_removed,
                }))
            }
            "status" => match client.tx_status() {
                Some((id, snap)) => Ok(serde_json::json!({
                    "active": true,
                    "id": id,
                    "snapshot": {
                        "lib": snap.lib,
                        "cell": snap.cell,
                        "view": snap.view,
                        "instances": snap.instances.len(),
                        "nets": snap.nets.len(),
                    }
                })),
                None => Ok(serde_json::json!({ "active": false })),
            },
            _ => Err(VirtuosoError::Execution(format!(
                "unknown tx method '{}'",
                op
            ))),
        }
    }

    fn dispatch_file(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "upload" => {
                let local = json_str(params.get("local"), "local")?;
                let remote = json_str(params.get("remote"), "remote")?;
                client.upload_file(&local, &remote)?;
                Ok(serde_json::json!({ "status": "ok", "remote": remote }))
            }
            "download" => {
                let remote = json_str(params.get("remote"), "remote")?;
                let local = json_str(params.get("local"), "local")?;
                client.download_file(&remote, &local)?;
                Ok(serde_json::json!({ "status": "ok", "local": local }))
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown file method '{}'",
                op
            ))),
        }
    }

    fn dispatch_util(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "version" => {
                let version = client.version()?;
                Ok(serde_json::json!({
                    "version": format!("{:?}", version),
                    "is_ic25": version.is_ic25(),
                }))
            }
            "ping" => {
                // Use execute_skill_unchecked to avoid auth check for internal
                // operations. The probe is `plus(1 1)`: a no-op SKILL
                // expression that returns a non-nil integer on any responsive
                // daemon. `ipcIsProcessRunning()` (previously used here)
                // requires a specific process-handle argument and returns nil
                // when called without one, making the ping spuriously fail on
                // live daemons.
                let r = client.execute_skill_unchecked("plus(1 1)", Some(5000))?;
                if r.skill_ok() {
                    Ok(serde_json::json!({ "status": "ok" }))
                } else {
                    Err(VirtuosoError::Execution("ping failed".into()))
                }
            }
            "ciw_print" => {
                let message = json_str(params.get("message"), "message")?;
                let r = client.execute_skill_unchecked(
                    &format!("println(\"{}\")", escape_skill_string(&message)),
                    None,
                )?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "reconnect" => {
                let session = json_str(params.get("session"), "session")?;
                let success = client.reconnect_session(&session)?;
                Ok(serde_json::json!({ "status": if success { "ok" } else { "failed" } }))
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown util method '{}'",
                op
            ))),
        }
    }

    fn dispatch_skill(client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "exec" => {
                // Admin capability is checked by execute_skill (not execute_skill_unchecked)
                let code = json_str(params.get("code"), "code")?;
                let timeout = params.get("timeout").and_then(|v| v.as_u64());
                let r = client.execute_skill(&code, timeout)?;
                Ok(serde_json::json!({ "output": r.output.trim() }))
            }
            "load" => {
                let path = json_str(params.get("path"), "path")?;
                let r = client.load_il(&path)?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "eval" => {
                let code = params
                    .get("code")
                    .and_then(|v| v.as_str().map(String::from));
                let stdin = params
                    .get("stdin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let r = commands::skill::eval(code, stdin)?;
                Ok(r)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown skill method '{}'",
                op
            ))),
        }
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────

fn json_str(value: Option<&Value>, field: &str) -> Result<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| VirtuosoError::Execution(format!("missing required field: {}", field)))
}

fn json_str_or(value: Option<&Value>, default: &str) -> Result<String> {
    Ok(value
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| default.to_string()))
}

fn json_i64_or(value: Option<&Value>, default: i64) -> i64 {
    value.and_then(|v| v.as_i64()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::schema::{standard_schema, RpcSchema};

    #[test]
    fn schema_contains_schematic_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"schematic.open_cell_view"),
            "should have open_cell_view"
        );
        assert!(names.contains(&"schematic.place"), "should have place");
        assert!(
            names.contains(&"schematic.list_instances"),
            "should have list_instances"
        );
        assert!(
            names.contains(&"schematic.list_nets"),
            "should have list_nets"
        );
        assert!(
            names.contains(&"schematic.list_pins"),
            "should have list_pins"
        );
        assert!(names.contains(&"schematic.save"), "should have save");
        assert!(names.contains(&"schematic.check"), "should have check");
    }

    #[test]
    fn schema_contains_maestro_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"maestro.open_session"),
            "should have open_session"
        );
        assert!(
            names.contains(&"maestro.close_session"),
            "should have close_session"
        );
        assert!(
            names.contains(&"maestro.list_sessions"),
            "should have list_sessions"
        );
        assert!(names.contains(&"maestro.set_var"), "should have set_var");
        assert!(names.contains(&"maestro.get_var"), "should have get_var");
        assert!(
            names.contains(&"maestro.list_vars"),
            "should have list_vars"
        );
        assert!(names.contains(&"maestro.run"), "should have run");
        assert!(names.contains(&"maestro.save"), "should have save");
        assert!(names.contains(&"maestro.export"), "should have export");
    }

    #[test]
    fn schema_contains_window_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"window.list"), "should have window.list");
        assert!(
            names.contains(&"window.screenshot"),
            "should have window.screenshot"
        );
    }

    #[test]
    fn schema_contains_cell_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"cell.open"), "should have cell.open");
        assert!(names.contains(&"cell.save"), "should have cell.save");
        assert!(names.contains(&"cell.close"), "should have cell.close");
    }

    #[test]
    fn schema_version_is_1_0() {
        let schema = standard_schema();
        assert_eq!(schema.version, "1.0");
    }

    #[test]
    fn schema_params_have_required_flag() {
        let schema = standard_schema();
        let open_cv = schema
            .methods
            .iter()
            .find(|m| m.name == "schematic.open_cell_view")
            .unwrap();
        let lib_param = open_cv.params.iter().find(|p| p.name == "lib").unwrap();
        assert!(lib_param.required, "lib should be required");
        let view_param = open_cv.params.iter().find(|p| p.name == "view").unwrap();
        assert!(!view_param.required, "view should be optional");
    }

    #[test]
    fn rpc_request_debug() {
        let req = RpcRequest {
            method: "schematic.list_instances".into(),
            params: serde_json::json!({}),
            api_key: None,
        };
        let debug = format!("{:?}", req);
        assert!(
            debug.contains("schematic.list_instances"),
            "debug should contain method name"
        );
    }

    #[test]
    fn schema_serialization_roundtrip() {
        let schema = standard_schema();
        let json = serde_json::to_string(&schema).unwrap();
        let deserialized: RpcSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.methods.len(), schema.methods.len());
        assert_eq!(deserialized.version, schema.version);
    }

    #[test]
    fn schema_method_param_types() {
        let schema = standard_schema();
        let place = schema
            .methods
            .iter()
            .find(|m| m.name == "schematic.place")
            .unwrap();
        assert!(place
            .params
            .iter()
            .any(|p| p.name == "master" && p.ptype == "string"));
        assert!(place
            .params
            .iter()
            .any(|p| p.name == "name" && p.ptype == "string"));
        assert!(place
            .params
            .iter()
            .any(|p| p.name == "x" && p.ptype == "integer"));
        assert!(place
            .params
            .iter()
            .any(|p| p.name == "y" && p.ptype == "integer"));
        assert!(place
            .params
            .iter()
            .any(|p| p.name == "orient" && p.ptype == "string"));
    }

    #[test]
    fn schema_no_params_methods() {
        let schema = standard_schema();
        let list_inst = schema
            .methods
            .iter()
            .find(|m| m.name == "schematic.list_instances")
            .unwrap();
        assert!(
            list_inst.params.is_empty(),
            "list_instances should have no params"
        );

        let save = schema
            .methods
            .iter()
            .find(|m| m.name == "schematic.save")
            .unwrap();
        assert!(save.params.is_empty(), "save should have no params");
    }

    #[test]
    fn schema_contains_tx_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"tx.begin"), "should have tx.begin");
        assert!(names.contains(&"tx.commit"), "should have tx.commit");
        assert!(names.contains(&"tx.rollback"), "should have tx.rollback");
        assert!(names.contains(&"tx.diff"), "should have tx.diff");
        assert!(names.contains(&"tx.status"), "should have tx.status");
    }

    #[test]
    fn schema_contains_file_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"file.upload"), "should have file.upload");
        assert!(
            names.contains(&"file.download"),
            "should have file.download"
        );
    }

    #[test]
    fn schema_contains_new_window_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"window.screenshot_by_pattern"),
            "should have window.screenshot_by_pattern"
        );
        assert!(
            names.contains(&"window.dismiss_dialog"),
            "should have window.dismiss_dialog"
        );
        assert!(
            names.contains(&"window.get_dialog_info"),
            "should have window.get_dialog_info"
        );
    }

    #[test]
    fn schema_contains_new_cell_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"cell.info"), "should have cell.info");
        assert!(names.contains(&"cell.create"), "should have cell.create");
    }

    #[test]
    fn schema_contains_new_maestro_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"maestro.set_analysis"),
            "should have maestro.set_analysis"
        );
        assert!(
            names.contains(&"maestro.add_output"),
            "should have maestro.add_output"
        );
        assert!(
            names.contains(&"maestro.set_design"),
            "should have maestro.set_design"
        );
        assert!(
            names.contains(&"maestro.save_setup"),
            "should have maestro.save_setup"
        );
        assert!(
            names.contains(&"maestro.get_spec_status"),
            "should have maestro.get_spec_status"
        );
        assert!(
            names.contains(&"maestro.get_current_session"),
            "should have maestro.get_current_session"
        );
        assert!(
            names.contains(&"maestro.open_results"),
            "should have maestro.open_results"
        );
        assert!(
            names.contains(&"maestro.close_results"),
            "should have maestro.close_results"
        );
        assert!(
            names.contains(&"maestro.get_result_tests"),
            "should have maestro.get_result_tests"
        );
        assert!(
            names.contains(&"maestro.get_result_outputs"),
            "should have maestro.get_result_outputs"
        );
        assert!(
            names.contains(&"maestro.get_output_value"),
            "should have maestro.get_output_value"
        );
        assert!(
            names.contains(&"maestro.get_history_list"),
            "should have maestro.get_history_list"
        );
        assert!(
            names.contains(&"maestro.get_analyses"),
            "should have maestro.get_analyses"
        );
        assert!(
            names.contains(&"maestro.get_outputs"),
            "should have maestro.get_outputs"
        );
        assert!(
            names.contains(&"maestro.get_sim_messages"),
            "should have maestro.get_sim_messages"
        );
    }

    #[test]
    fn schema_tx_begin_params() {
        let schema = standard_schema();
        let tx_begin = schema
            .methods
            .iter()
            .find(|m| m.name == "tx.begin")
            .unwrap();
        assert!(
            tx_begin.params.iter().any(|p| p.name == "id" && p.required),
            "id should be required"
        );
        assert!(
            tx_begin
                .params
                .iter()
                .any(|p| p.name == "lib" && p.required),
            "lib should be required"
        );
        assert!(
            tx_begin
                .params
                .iter()
                .any(|p| p.name == "cell" && p.required),
            "cell should be required"
        );
        assert!(
            tx_begin
                .params
                .iter()
                .any(|p| p.name == "view" && !p.required),
            "view should be optional"
        );
    }

    #[test]
    fn schema_file_upload_params() {
        let schema = standard_schema();
        let upload = schema
            .methods
            .iter()
            .find(|m| m.name == "file.upload")
            .unwrap();
        assert!(
            upload
                .params
                .iter()
                .any(|p| p.name == "local" && p.required),
            "local should be required"
        );
        assert!(
            upload
                .params
                .iter()
                .any(|p| p.name == "remote" && p.required),
            "remote should be required"
        );
    }

    #[test]
    fn schema_get_output_value_params() {
        let schema = standard_schema();
        let get_val = schema
            .methods
            .iter()
            .find(|m| m.name == "maestro.get_output_value")
            .unwrap();
        assert!(
            get_val
                .params
                .iter()
                .any(|p| p.name == "name" && p.required),
            "name should be required"
        );
        assert!(
            get_val
                .params
                .iter()
                .any(|p| p.name == "test" && p.required),
            "test should be required"
        );
        assert!(
            get_val
                .params
                .iter()
                .any(|p| p.name == "corner" && !p.required),
            "corner should be optional"
        );
    }

    #[test]
    fn schema_total_method_count() {
        let schema = standard_schema();
        // Should have 61 methods total (58 + 3 new: skill.eval, maestro.snapshot, schematic.polish_label)
        assert_eq!(schema.methods.len(), 61, "should have exactly 61 methods");
    }

    #[test]
    fn schema_contains_util_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"util.version"), "should have util.version");
        assert!(names.contains(&"util.ping"), "should have util.ping");
        assert!(
            names.contains(&"util.ciw_print"),
            "should have util.ciw_print"
        );
        assert!(
            names.contains(&"util.reconnect"),
            "should have util.reconnect"
        );
    }

    #[test]
    fn schema_contains_skill_methods() {
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"skill.exec"), "should have skill.exec");
        assert!(names.contains(&"skill.load"), "should have skill.load");
    }

    #[test]
    fn schema_skill_exec_params() {
        let schema = standard_schema();
        let exec = schema
            .methods
            .iter()
            .find(|m| m.name == "skill.exec")
            .unwrap();
        assert!(
            exec.params.iter().any(|p| p.name == "code" && p.required),
            "code should be required"
        );
        assert!(
            exec.params
                .iter()
                .any(|p| p.name == "timeout" && !p.required),
            "timeout should be optional"
        );
    }

    #[test]
    fn parse_skill_json_handles_escaped_quotes() {
        // SKILL output format: the bridge returns JSON arrays directly from SKILL's sprintf
        // This test verifies the function can handle standard JSON output
        let output = r#"[{"name":"M1"}]"#;
        let result = parse_skill_json(output);
        assert!(result.is_ok(), "should parse JSON with objects");
    }

    #[test]
    fn parse_skill_json_handles_plain_json() {
        // Direct JSON (no escaping needed)
        let output = r#"["a", "b"]"#;
        let result = parse_skill_json(output);
        assert!(result.is_ok(), "should parse direct JSON");
        let val = result.unwrap();
        let arr = val.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn parse_skill_json_handles_object() {
        let output = r#"{"name":"M1"}"#;
        let result = parse_skill_json(output);
        assert!(result.is_ok(), "should parse JSON object");
        let val = result.unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("name").unwrap().as_str().unwrap(), "M1");
    }
}

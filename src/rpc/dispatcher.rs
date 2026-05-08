//! RPC dispatcher — maps {method, params} to SKILL expressions.
//!
//! Each domain (schematic, maestro, window, cell) is handled by its ops struct.
//! The dispatcher routes the incoming JSON-RPC request to the correct handler.

use crate::auth::{check_auth, log_rpc};
use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use regex::Regex;
use serde_json::Value;

/// Fix SKILL's octal escape sequences (e.g., \256) to JSON unicode escapes (\u00AE).
/// SKILL uses \NNN octal for non-ASCII chars, but JSON only supports \uXXXX unicode.
fn fix_skill_octal_escapes(s: &str) -> String {
    // Match \NNN where N is 0-7, up to 3 digits
    let re = Regex::new(r"\\([0-7]{1,3})").unwrap();
    re.replace_all(s, |caps: &regex::Captures| {
        let octal = &caps[1];
        if let Ok(code) = u8::from_str_radix(octal, 8) {
            format!("\\u{:04X}", code)
        } else {
            caps[0].to_string()
        }
    })
    .to_string()
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
        check_auth(api_key.as_deref())?;

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
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "list_nets" => {
                let skill = ops.list_nets();
                let r = client.execute_skill_unchecked(&skill, None)?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "list_pins" => {
                let skill = ops.list_pins();
                let r = client.execute_skill_unchecked(&skill, None)?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "get_params" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let skill = ops.get_instance_params(&inst);
                let r = client.execute_skill_unchecked(&skill, None)?;
                if r.output.trim() == "null" {
                    Ok(serde_json::Value::Null)
                } else {
                    serde_json::from_str(&r.output).map_err(VirtuosoError::Json)
                }
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
                let fixed = fix_skill_octal_escapes(&r.output);
                let parsed: Value = serde_json::from_str(&fixed).map_err(VirtuosoError::Json)?;
                Ok(parsed)
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
            _ => Err(VirtuosoError::Execution(format!(
                "unknown cell method '{}'",
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
}

//! RPC dispatcher — maps {method, params} to SKILL expressions.
//!
//! Each domain (schematic, maestro, window, cell) is handled by its ops struct.
//! The dispatcher routes the incoming JSON-RPC request to the correct handler.

use crate::auth::{check_auth, log_rpc};
use crate::client::bridge::{escape_skill_string, VirtuosoClient};
use crate::commands;
use crate::context::CommandContext;
use crate::error::{Result, VirtuosoError};
use crate::models::VirtuosoResult;
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

/// Require both a successful bridge transport and a non-nil SKILL result.
/// RPC write operations use this rather than treating an STX frame as success.
fn require_skill_result(result: VirtuosoResult, operation: &str) -> Result<VirtuosoResult> {
    result.ok_or_exec(operation)
}

fn execute_required_skill(
    client: &VirtuosoClient,
    skill: &str,
    operation: &str,
) -> Result<VirtuosoResult> {
    require_skill_result(client.execute_skill_unchecked(skill, None)?, operation)
}

/// Require a successful bridge transport while preserving documented nil query results.
fn require_transport_result(result: VirtuosoResult, operation: &str) -> Result<VirtuosoResult> {
    crate::client::skill_runtime::require_transport(&result, operation)?;
    Ok(result)
}

fn execute_query_skill(
    client: &VirtuosoClient,
    skill: &str,
    operation: &str,
) -> Result<VirtuosoResult> {
    require_transport_result(client.execute_skill_unchecked(skill, None)?, operation)
}

fn require_cell_write_result(result: VirtuosoResult, operation: &str) -> Result<VirtuosoResult> {
    require_skill_result(result, operation)
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

pub struct RpcDispatcher {
    ctx: CommandContext,
}

/// `method` → (summary, declared parameter names), built once from the schema.
///
/// Only the built-in schema is indexed. Plugin domains are absent on purpose —
/// see [`reject_undeclared_params`].
static DECLARED_PARAMS: Lazy<std::collections::HashMap<String, (String, Vec<String>)>> =
    Lazy::new(|| {
        crate::rpc::schema::standard_schema()
            .methods
            .into_iter()
            .map(|m| {
                let names = m.params.into_iter().map(|p| p.name).collect();
                (m.name, (m.summary, names))
            })
            .collect()
    });

/// Refuse a request carrying a parameter the method does not declare.
///
/// Before this, extra keys were dropped without a trace, so "the argument was
/// wrong" and "the argument was honoured" produced identical output:
/// `schematic.list_instances {"lib":"DESIGN_LIB","cell":"no_such_cell"}`
/// answered with the twelve instances of whatever cellview happened to be
/// open. The schema declares every method's parameters already, so the check
/// is mechanical — and any dispatch arm that reads an undeclared key is caught
/// by `every_param_a_dispatch_arm_reads_is_declared_in_the_schema` below.
///
/// Methods the built-in schema does not know about are plugin-provided and
/// carry their own contract; leave them alone.
fn reject_undeclared_params(method: &str, params: &Value) -> Result<()> {
    let (Some((summary, declared)), Some(obj)) =
        (DECLARED_PARAMS.get(method), params.as_object())
    else {
        return Ok(());
    };
    let unknown: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !declared.iter().any(|d| d == k))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let accepted = if declared.is_empty() {
        "it takes no parameters".to_string()
    } else {
        format!("it accepts: {}", declared.join(", "))
    };
    Err(VirtuosoError::Config(format!(
        "method '{}' does not accept {} — {} ({})",
        method,
        unknown
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(", "),
        accepted,
        summary
    )))
}

impl RpcDispatcher {
    pub fn new(ctx: CommandContext) -> Self {
        Self { ctx }
    }

    /// Dispatch a JSON-RPC request to the appropriate handler.
    pub fn dispatch(&self, client: &VirtuosoClient, request: RpcRequest) -> Result<Value> {
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

        let result = reject_undeclared_params(&method, &params)
            .and_then(|()| Self::dispatch_inner(self, client, &method, params.clone()));

        // Audit log — always log, regardless of success/failure
        let result_str = match &result {
            Ok(v) => serde_json::to_string(v).unwrap_or_default(),
            Err(e) => format!("error: {e}"),
        };
        log_rpc(&method, &params, &result_str, client.session_id.as_deref());

        result
    }

    fn dispatch_inner(
        &self,
        client: &VirtuosoClient,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let parts: Vec<&str> = method.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(VirtuosoError::Execution(format!(
                "invalid method '{}': expected 'domain.method'",
                method
            )));
        }
        let (domain, op) = (parts[0], parts[1]);

        match domain {
            "schematic" => self.dispatch_schematic(client, op, params),
            "symbol" => {
                let ctx = &self.ctx;
                match op {
                    "inspect" => {
                        let lib = json_str(params.get("lib"), "lib")?;
                        let cell = json_str(params.get("cell"), "cell")?;
                        let view = json_str_or(params.get("view"), "symbol")?;
                        let view_type = json_str_or(params.get("view_type"), "schematicSymbol")?;
                        crate::commands::symbol::inspect(ctx, &lib, &cell, &view, &view_type)
                    }
                    "generate" => {
                        let lib = json_str(params.get("lib"), "lib")?;
                        let cell = json_str(params.get("cell"), "cell")?;
                        let src = json_str_or(params.get("schematic_view"), "schematic")?;
                        let dst = json_str_or(params.get("symbol_view"), "symbol")?;
                        let sort = params.get("sort_pins").and_then(Value::as_str);
                        crate::commands::symbol::generate(ctx, &lib, &cell, &src, &dst, sort)
                    }
                    _ => Err(VirtuosoError::NotFound(format!(
                        "unknown symbol method '{op}'"
                    ))),
                }
            }
            "library" => match op {
                "list"
                    if params.is_null()
                        || params.as_object().map(|m| m.is_empty()).unwrap_or(false) =>
                {
                    let ctx = &self.ctx;
                    crate::commands::library::list(ctx)
                }
                _ => Err(VirtuosoError::NotFound(format!(
                    "unknown library method '{op}'"
                ))),
            },
            "maestro" => self.dispatch_maestro(client, op, params),
            "window" => self.dispatch_window(client, op, params),
            "cell" => self.dispatch_cell(client, op, params),
            "tx" => self.dispatch_tx(client, op, params),
            "file" => self.dispatch_file(client, op, params),
            "util" => self.dispatch_util(client, op, params),
            "skill" => self.dispatch_skill(client, op, params),
            "libref" => Self::dispatch_libref(op, params),
            "sim" => self.dispatch_sim(client, op, params),
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

    fn dispatch_schematic(
        &self,
        client: &VirtuosoClient,
        op: &str,
        params: Value,
    ) -> Result<Value> {
        let ops = crate::client::schematic_ops::SchematicOps::new();
        match op {
            "open_cell_view" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                let skill = ops.open_cellview(&lib, &cell, &view);
                let r = execute_required_skill(client, &skill, "open cell view")?;
                // Hand back the bound target and whether a window shows it.
                // `status: ok` alone told the caller nothing about where the
                // following `schematic.*` calls would actually land.
                Ok(merge_status_ok(r.output.trim()))
            }
            "place" => {
                let master = json_str(params.get("master"), "master")?;
                // `master` is documented as "lib/cell" (see standard_schema()).
                // Split it into the separate library and cell names dbOpenCellView-
                // ByType needs; previously the whole "lib/cell" string was passed as
                // BOTH the lib and the cell, so no real cell (lib != cell, e.g.
                // analogLib/res) could ever be resolved. The view is always "symbol"
                // — placing a symbol is the correct behaviour for an instance.
                let (lib, cell) = master.split_once('/').ok_or_else(|| {
                    VirtuosoError::Execution(format!(
                        "master must be in lib/cell format (e.g. analogLib/res), got '{master}'"
                    ))
                })?;
                let name = json_str(params.get("name"), "name")?;
                let x = json_coord_or(params.get("x"), "x", 0.0)?;
                let y = json_coord_or(params.get("y"), "y", 0.0)?;
                let orient = json_str_or(params.get("orient"), "R0")?;
                let skill = ops.create_instance(lib, cell, "symbol", &name, (x, y), &orient);
                execute_required_skill(client, &skill, "place instance")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "move_instance" => {
                let name = json_str(params.get("name"), "name")?;
                // Absolute target; the SKILL converts to the relative
                // displacement dbMoveFig actually applies.
                let x = json_f64(params.get("x"), "x")?;
                let y = json_f64(params.get("y"), "y")?;
                // Absolute orientation; omit to leave the placement as-is.
                let orient = params.get("orient").and_then(|v| v.as_str());
                let skill = ops.move_instance(&name, (x, y), orient);
                execute_required_skill(client, &skill, "move instance")?;
                Ok(serde_json::json!({
                    "status": "ok", "name": name, "x": x, "y": y, "orient": orient
                }))
            }
            "wire" => {
                let net = json_str(params.get("net"), "net")?;
                // Every point must parse. The two `filter_map`s that used to be
                // here dropped whatever did not — a typo in one coordinate
                // shortened the wire and still reported ok, so the missing
                // segment only showed up as a disconnected net much later.
                let pts = parse_wire_points(params.get("points"))?;
                let skill = ops.create_wire(&pts, &net);
                execute_required_skill(client, &skill, "create wire")?;
                Ok(serde_json::json!({
                    "status": "ok", "net": net, "segments": pts.len() - 1
                }))
            }
            "label" => {
                let net = json_str(params.get("net"), "net")?;
                let x = json_coord_or(params.get("x"), "x", 0.0)?;
                let y = json_coord_or(params.get("y"), "y", 0.0)?;
                let skill = ops.create_wire_label(&net, (x, y));
                execute_required_skill(client, &skill, "create label")?;
                Ok(serde_json::json!({ "status": "ok", "net": net, "x": x, "y": y }))
            }
            "pin" => {
                let net = json_str(params.get("net"), "net")?;
                let dir = json_str(params.get("direction"), "direction")?;
                // Reject unknown directions instead of quietly falling back to
                // a default: the direction decides the pin master and the
                // terminal direction that `symbol.generate` later reads, so a
                // silent substitution produces a wrong symbol with no error.
                if crate::client::schematic_ops::pin_master_for(&dir).is_none() {
                    return Err(VirtuosoError::Execution(format!(
                        "unknown pin direction '{dir}': expected one of \
                         input, output, inputOutput, switch, jumper"
                    )));
                }
                let x = json_coord_or(params.get("x"), "x", 0.0)?;
                let y = json_coord_or(params.get("y"), "y", 0.0)?;
                let skill = ops.create_pin(&net, &dir, (x, y));
                execute_required_skill(client, &skill, "create pin")?;
                Ok(serde_json::json!({ "status": "ok", "net": net, "direction": dir }))
            }
            "save" => {
                let skill = ops.save();
                execute_required_skill(client, &skill, "save schematic")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "check" => {
                let skill = ops.check();
                let r = execute_required_skill(client, &skill, "check schematic")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "list_instances" => {
                let skill = ops.list_instances();
                let r = execute_query_skill(client, &skill, "list schematic instances")?;
                parse_skill_json(&r.output)
            }
            "list_nets" => {
                let skill = ops.list_nets();
                let r = execute_query_skill(client, &skill, "list schematic nets")?;
                parse_skill_json(&r.output)
            }
            "list_pins" => {
                let skill = ops.list_pins();
                let r = execute_query_skill(client, &skill, "list schematic pins")?;
                parse_skill_json(&r.output)
            }
            "list_cdf_params" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let skill = ops.list_cdf_params(&inst);
                let r = execute_query_skill(client, &skill, "list CDF parameters")?;
                parse_skill_json(&r.output)
            }
            "get_params" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let skill = ops.get_instance_params(&inst);
                let r = execute_query_skill(client, &skill, "get instance parameters")?;
                if r.output.trim() == "null" {
                    Ok(serde_json::Value::Null)
                } else {
                    parse_skill_json(&r.output)
                }
            }
            "set_param" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let param = json_str(params.get("param"), "param")?;
                let value = json_str(params.get("value"), "value")?;
                let skill = ops.set_instance_param(&inst, &param, &value);
                execute_required_skill(client, &skill, "set instance parameter")?;
                Ok(serde_json::json!({
                    "instance": inst,
                    "param": param,
                    "value": value,
                    "status": "ok"
                }))
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
            "net_stub" => {
                let net = json_str(params.get("net"), "net")?;
                let x = json_coord_or(params.get("x"), "x", 0.0)?;
                let y = json_coord_or(params.get("y"), "y", 0.0)?;
                let direction = json_str_or(params.get("direction"), "right")?;
                let length = params.get("length").and_then(|v| v.as_f64()).unwrap_or(0.5);
                let cosmetic = json_str_or(params.get("cosmetic"), "default")?;
                let r = commands::schematic::net_stub(&net, x, y, &direction, length, &cosmetic)?;
                Ok(r)
            }
            "label_term" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let term = json_str(params.get("term"), "term")?;
                let net = json_str(params.get("net"), "net")?;
                let cosmetic = json_str_or(params.get("cosmetic"), "default")?;
                let auto_rotate = params
                    .get("auto_rotate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let r =
                    commands::schematic::label_term(&inst, &term, &net, &cosmetic, auto_rotate)?;
                Ok(r)
            }
            "assign_net" => {
                let inst = json_str(params.get("inst"), "inst")?;
                let term = json_str(params.get("term"), "term")?;
                let net = json_str(params.get("net"), "net")?;
                let skill = ops.assign_net(&inst, &term, &net);
                execute_required_skill(client, &skill, "assign net")?;
                Ok(serde_json::json!({
                    "instance": inst,
                    "term": term,
                    "net": net,
                    "status": "ok"
                }))
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown schematic method '{}'",
                op
            ))),
        }
    }

    fn dispatch_maestro(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        let ops = crate::client::maestro_ops::MaestroOps;
        match op {
            "open_session" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "maestro")?;
                // Read-only by default: append mode takes an edit lock on a
                // session that has no window, which locks a human out of the
                // cell with nothing to click (EXPLORER-1642).
                let mode = json_str_or(params.get("mode"), "r")?;
                crate::commands::maestro::check_session_mode(&mode)?;
                let skill = ops.open_session(&lib, &cell, &view, &mode);
                let r = execute_required_skill(client, &skill, "open Maestro session")?;
                // `output_unquoted`, not `output.trim()`: the bridge hands back
                // the SKILL string literal `"fnxSession12"` with its quotes, and
                // a caller who passes that straight into `close_session` or
                // `set_session_mode` gets `nil` back, because the quotes end up
                // inside the session name. Measured 2026-09-09.
                Ok(serde_json::json!({
                    "status": "ok",
                    "session": r.output_unquoted(),
                    "mode": mode,
                }))
            }
            "set_session_mode" => {
                let session = json_str(params.get("session"), "session")?;
                let mode = json_str(params.get("mode"), "mode")?;
                crate::commands::maestro::check_session_mode(&mode)?;
                let skill = ops.set_session_mode(&session, mode == "a");
                execute_required_skill(client, &skill, "set Maestro session mode")?;
                Ok(serde_json::json!({ "status": "ok", "session": session, "mode": mode }))
            }
            "close_session" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.close_session(&session);
                execute_required_skill(client, &skill, "close Maestro session")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "list_sessions" => {
                let skill = ops.list_sessions();
                let r = execute_query_skill(client, &skill, "list Maestro sessions")?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "list_tests" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.list_tests(&session);
                let r = execute_query_skill(client, &skill, "list Maestro tests")?;
                let parsed: Value = serde_json::from_str(&r.output).map_err(VirtuosoError::Json)?;
                Ok(parsed)
            }
            "set_var" => {
                let name = json_str(params.get("name"), "name")?;
                let value = json_str(params.get("value"), "value")?;
                let skill = ops.set_var(&name, &value);
                execute_required_skill(client, &skill, "set Maestro variable")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "get_var" => {
                let name = json_str(params.get("name"), "name")?;
                let skill = ops.get_var(&name);
                let r = execute_query_skill(client, &skill, "get Maestro variable")?;
                Ok(serde_json::json!({ "value": r.output.trim() }))
            }
            "list_vars" => {
                let skill = ops.list_vars();
                let r = execute_query_skill(client, &skill, "list Maestro variables")?;
                // `parse_skill_json`, not a bare `from_str`: the bridge hands
                // back the SKILL string still quoted, which `from_str` would
                // happily parse as a JSON *string* rather than the array.
                parse_skill_json(&r.output)
            }
            "delete_var" => {
                let name = json_str(params.get("name"), "name")?;
                let skill = ops.delete_var(&name);
                execute_required_skill(client, &skill, "delete Maestro variable")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "delete_output" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let skill = ops.delete_output(&name, &test);
                execute_required_skill(client, &skill, "delete Maestro output")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "delete_analysis" => {
                let analysis = json_str(params.get("analysis"), "analysis")?;
                let skill = ops.delete_analysis(&analysis);
                // Verified delete: `asiDeleteAnalysis` throws even when it
                // succeeds, so the SKILL re-reads the enabled list and reports
                // `"t"` only if the analysis is actually gone.
                let r = execute_query_skill(client, &skill, "delete Maestro analysis")?;
                if r.output.trim().trim_matches('"') == "t" {
                    Ok(serde_json::json!({ "status": "ok" }))
                } else {
                    Err(VirtuosoError::Execution(format!(
                        "analysis '{analysis}' is still enabled after delete"
                    )))
                }
            }
            "run" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.run_simulation(&session);
                execute_required_skill(client, &skill, "run Maestro simulation")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "save" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.save_setup(&session);
                execute_required_skill(client, &skill, "save Maestro setup")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "export" => {
                let session = json_str(params.get("session"), "session")?;
                let path = json_str(params.get("path"), "path")?;
                let test_name = params.get("test_name").and_then(|v| v.as_str());
                let skill = ops.export_results(&session, &path, test_name, None);
                execute_required_skill(client, &skill, "export Maestro results")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            // ── Result Reading ────────────────────────────────────────
            "open_results" => {
                let history = json_str(params.get("history"), "history")?;
                let skill = ops.open_results(&history);
                execute_required_skill(client, &skill, "open Maestro results")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "close_results" => {
                let skill = ops.close_results();
                execute_required_skill(client, &skill, "close Maestro results")?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "get_result_tests" => {
                let skill = ops.get_result_tests();
                let r = execute_query_skill(client, &skill, "get Maestro result tests")?;
                parse_skill_json(&r.output)
            }
            "get_result_outputs" => {
                let test_name = json_str(params.get("test"), "test")?;
                let skill = ops.get_result_outputs(&test_name);
                let r = execute_query_skill(client, &skill, "get Maestro result outputs")?;
                parse_skill_json(&r.output)
            }
            "get_output_value" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let corner = params.get("corner").and_then(|v| v.as_str());
                let skill = ops.get_output_value(&name, &test, corner);
                let r = execute_query_skill(client, &skill, "get Maestro output value")?;
                Ok(serde_json::json!({ "value": r.output.trim() }))
            }
            "get_history_list" => {
                let skill = ops.get_history_list();
                let r = execute_query_skill(client, &skill, "get Maestro history list")?;
                parse_skill_json(&r.output)
            }
            "get_analyses" => {
                let session = json_str(params.get("session"), "session")?;
                let test = params.get("test").and_then(|v| v.as_str());
                let version = client
                    .version()
                    .unwrap_or(crate::version::VirtuosoVersion::IC23);
                let skill = ops.get_analyses(&session, test, version);
                let r = execute_query_skill(client, &skill, "get Maestro analyses")?;
                Ok(serde_json::json!({ "analyses": r.output.trim() }))
            }
            "get_outputs" => {
                let test = json_str(params.get("test"), "test")?;
                let skill = ops.get_outputs(&test);
                let r = execute_query_skill(client, &skill, "get Maestro outputs")?;
                parse_skill_json(&r.output)
            }
            "get_sim_messages" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.get_sim_messages(&session);
                let r = execute_query_skill(client, &skill, "get Maestro simulation messages")?;
                Ok(serde_json::json!({ "messages": r.output.trim() }))
            }
            // ── Session & Setup Management ─────────────────────────────
            "get_current_session" => {
                let skill = ops.get_current_session();
                let r = execute_query_skill(client, &skill, "get current Maestro session")?;
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
                let test = params.get("test").and_then(|v| v.as_str());
                let version = client
                    .version()
                    .unwrap_or(crate::version::VirtuosoVersion::IC23);
                let skill = ops.set_analysis(&session, &analysis_type, options, test, version);
                let r = execute_required_skill(client, &skill, "set Maestro analysis")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "add_output" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let expr = json_str(params.get("expr"), "expr")?;
                let skill = ops.add_output(&name, &test, &expr);
                let r = execute_required_skill(client, &skill, "add Maestro output")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "set_design" => {
                let session = json_str(params.get("session"), "session")?;
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str(params.get("view"), "view")?;
                // Optional: without it every test in the session is retargeted.
                let test = params
                    .get("test")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let skill = ops.set_design(&session, &lib, &cell, &view, test.as_deref());
                let r = execute_required_skill(client, &skill, "set Maestro design")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "create_test" => {
                let session = json_str(params.get("session"), "session")?;
                let test = json_str(params.get("test"), "test")?;
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                let simulator = json_str_or(params.get("simulator"), "spectre")?;
                let skill = ops.create_test(&session, &test, &lib, &cell, &view, &simulator);
                execute_required_skill(client, &skill, "create Maestro test")?;
                Ok(serde_json::json!({
                    "status": "ok",
                    "test": test,
                    "lib": lib,
                    "cell": cell,
                    "view": view,
                    "simulator": simulator,
                }))
            }
            "save_setup" => {
                let session = json_str(params.get("session"), "session")?;
                let skill = ops.save_setup(&session);
                let r = execute_required_skill(client, &skill, "save Maestro setup")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output.trim() }))
            }
            "get_spec_status" => {
                let name = json_str(params.get("name"), "name")?;
                let test = json_str(params.get("test"), "test")?;
                let skill = ops.get_spec_status(&name, &test);
                let r = execute_query_skill(client, &skill, "get Maestro spec status")?;
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
            "create_corner_netlist" => {
                // Dispatch the same helper the CLI uses. RPC params mirror
                // the CLI flags exactly: `session`, `test`, `corner`, `output_dir`,
                // all required and shell-quoted downstream by the helper.
                let session = json_str(params.get("session"), "session")?;
                let test = json_str(params.get("test"), "test")?;
                let corner = json_str(params.get("corner"), "corner")?;
                let output_dir = json_str(params.get("output_dir"), "output_dir")?;
                let r = crate::commands::maestro::create_corner_netlist(
                    &session,
                    &test,
                    &corner,
                    &output_dir,
                )?;
                Ok(r)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown maestro method '{}'",
                op
            ))),
        }
    }

    fn dispatch_window(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        let ops = crate::client::window_ops::WindowOps;
        match op {
            "list" => {
                let skill = ops.list_windows();
                let r = execute_query_skill(client, &skill, "list windows")?;
                parse_skill_json(&r.output)
            }
            "screenshot" => {
                let path = json_str(params.get("path"), "path")?;
                let skill = ops.screenshot(&path);
                let r = execute_query_skill(client, &skill, "capture screenshot")?;
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
                let r = execute_query_skill(client, &skill, "capture screenshot by pattern")?;
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
                let r = execute_required_skill(client, &skill, "dismiss dialog")?;
                // The bridge hands back the SKILL string with its quotes, so the
                // sentinel only matches after trimming them — same as
                // `commands::window`.
                let out = r.output.trim().trim_matches('"');
                if out == "no-dialog" {
                    Ok(serde_json::json!({ "status": "no-dialog" }))
                } else {
                    Ok(serde_json::json!({ "status": "ok", "action": out }))
                }
            }
            "get_dialog_info" => {
                let skill = ops.get_dialog_info();
                let r = execute_query_skill(client, &skill, "get dialog information")?;
                let out = r.output.trim().trim_matches('"');
                if out == "no-dialog" {
                    Ok(serde_json::json!({ "dialog": null }))
                } else {
                    Ok(serde_json::json!({ "dialog": out }))
                }
            }
            "dismiss_dialog_x11" => {
                // SKILL channel cannot unstick a deadlocked CIW; this dispatches
                // to the SSH+X11 bypass (transport::x11::dismiss).
                let action = json_str_or(params.get("action"), "enter")?;
                let dry_run = params
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let display = params.get("display").and_then(|v| v.as_str());
                let window_id = params.get("window_id").and_then(|v| v.as_str());
                let ctx = &self.ctx;
                crate::commands::window::dismiss_dialog_x11(
                    ctx, &action, dry_run, window_id, display,
                )
            }
            "list_windows_x11" => {
                // Enumerate every Virtuoso-related X11 window (no keypress).
                // Returns { display, windows, count }. Use the `dismiss_id`
                // from each entry to feed `dismiss_window_x11` next.
                let display = params.get("display").and_then(|v| v.as_str());
                let ctx = &self.ctx;
                crate::commands::window::list_windows_x11(ctx, display)
            }
            "dismiss_window_x11" => {
                // Dismiss a SPECIFIC window by id (typically the dismiss_id
                // returned by list_windows_x11). Unlike dismiss_dialog_x11
                // this does NOT apply the dialog-size filter — the caller is
                // expected to have already identified the target window.
                // Accepts window_id, or --pid to resolve the window by PID.
                let window_id = params.get("window_id").and_then(|v| v.as_str());
                let pid = params.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);
                let action = json_str_or(params.get("action"), "enter")?;
                let display = params.get("display").and_then(|v| v.as_str());
                let ctx = &self.ctx;
                crate::commands::window::dismiss_window_x11(ctx, window_id, pid, &action, display)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown window method '{}'",
                op
            ))),
        }
    }

    fn dispatch_cell(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "open" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "layout")?;
                let mode = json_str_or(params.get("mode"), "a")?;
                let r = require_cell_write_result(
                    client.open_cell_view(&lib, &cell, &view, &mode)?,
                    "open cell",
                )?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "save" => {
                let r = require_cell_write_result(client.save_current_cellview()?, "save cell")?;
                Ok(serde_json::json!({ "status": "ok", "output": r.output }))
            }
            "close" => {
                // Default to saving: an unsaved close is the path that can pop
                // a "save changes?" modal, and a modal freezes the bridge.
                let save = params
                    .get("save")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                let r =
                    require_cell_write_result(client.close_current_cellview(save)?, "close cell")?;
                Ok(serde_json::json!({ "status": "ok", "saved": save, "output": r.output }))
            }
            "info" => {
                let (lib, cell, view) = client.get_current_design()?;
                Ok(serde_json::json!({
                    "lib": lib,
                    "cell": cell,
                    "view": view,
                }))
            }
            "list_open" => {
                let r = execute_query_skill(
                    client,
                    crate::client::bridge::OPEN_CELLVIEWS,
                    "list open cellviews",
                )?;
                parse_skill_json(&r.output)
            }
            "create" => {
                let lib = json_str(params.get("lib"), "lib")?;
                let cell = json_str(params.get("cell"), "cell")?;
                let view = json_str_or(params.get("view"), "schematic")?;
                // `dbOpenCellViewByType` refuses to *create* without a view
                // type ("You need to specify a cellViewType to create a new
                // cellview", skdfref), and the type is not the view name.
                let view_type = match params.get("view_type").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(),
                    None => cell_view_type_for(&view).ok_or_else(|| {
                        VirtuosoError::Config(format!(
                            "cell.create: don't know the cellViewType for view '{view}' — \
                             pass 'view_type' explicitly (e.g. schematic, schematicSymbol, \
                             maskLayout, netlist)"
                        ))
                    })?,
                };
                let skill = format!(
                    // Mode "w" *wipes* an existing cellview, so refuse when one
                    // is already there rather than silently emptying it — the
                    // old `dbCreateCell` never got far enough to have this
                    // hazard, because it does not exist in IC23.1 at all
                    // (`undefined function dbCreateCell`, verified live).
                    // `dbOpenCellViewByType` only builds the cellview in
                    // memory; `dbSave` is what puts it on disk, and `dbClose`
                    // keeps the call from leaking an edit lock.
                    r#"let((existing cv) existing = nil errset(existing = ddGetObj("{lib}" "{cell}" "{view}")) when(existing error("cell.create: {lib}/{cell}/{view} already exists — delete it first, this would overwrite it")) cv = dbOpenCellViewByType("{lib}" "{cell}" "{view}" "{view_type}" "w") when(!cv error("cell.create: dbOpenCellViewByType failed for {lib}/{cell}/{view} (type {view_type})")) when(!dbSave(cv) dbClose(cv) error("cell.create: dbSave failed for {lib}/{cell}/{view}")) dbClose(cv) sprintf(nil "{lib}/{cell}/{view} ({view_type})"))"#,
                    lib = escape_skill_string(&lib),
                    cell = escape_skill_string(&cell),
                    view = escape_skill_string(&view),
                    view_type = escape_skill_string(&view_type)
                );
                let r = execute_required_skill(client, &skill, "create cell")?;
                Ok(serde_json::json!({
                    "status": "ok", "lib": lib, "cell": cell, "view": view,
                    "view_type": view_type, "output": r.output.trim()
                }))
            }
            "read_path" => {
                // Return the on-disk readPath of a registered library. Used by
                // `vcli diag cdslck` to know where to look for lock files.
                let lib = json_str(params.get("lib"), "lib")?;
                let skill = format!(
                    r#"ddGetObj("{lib}")~>readPath"#,
                    lib = escape_skill_string(&lib)
                );
                let r = execute_query_skill(client, &skill, "read library path")?;
                let raw = r.output.trim().trim_matches('"');
                Ok(serde_json::json!({
                    "lib": lib,
                    "read_path": if raw == "nil" || raw.is_empty() { None } else { Some(raw) },
                }))
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown cell method '{}'",
                op
            ))),
        }
    }

    fn dispatch_tx(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
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

    fn dispatch_file(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
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

    fn dispatch_util(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
        match op {
            "version" => {
                let version = client.version()?;
                Ok(serde_json::json!({
                    "version": format!("{:?}", version),
                    "is_ic25": version.is_ic25(),
                }))
            }
            "ping" => {
                // Ping uses the idempotent probe, NOT
                // `execute_skill_unchecked`: a ping must survive a stale
                // queued ticket — that is exactly the stuck state it is
                // meant to detect.
                // The probe expression is `plus(1 1)`: a no-op SKILL form
                // that returns a non-nil integer on any responsive daemon.
                // `ipcIsProcessRunning()` (previously used here) does not
                // exist on IC23.1 — it is in none of the 41 SKILL Finder
                // databases and `getd` returns nil — so the call errored and
                // the ping failed spuriously on live daemons. The old note
                // here blamed a missing process-handle argument; that was
                // the wrong diagnosis for the right fix.
                let r = client.execute_skill_idempotent_probe("plus(1 1)", Some(5000))?;
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
                let r = require_skill_result(r, "print to CIW")?;
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

    fn dispatch_skill(&self, client: &VirtuosoClient, op: &str, params: Value) -> Result<Value> {
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
                let r = client.load_il(&path, false)?;
                Ok(serde_json::json!({
                    "status": "ok", "output": r.output.trim(),
                    "loaded_path": r.metadata.get("loaded_path")
                }))
            }
            "eval" => {
                let ctx = &self.ctx;
                let code = params
                    .get("code")
                    .and_then(|v| v.as_str().map(String::from));
                let stdin = params
                    .get("stdin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let r = commands::skill::eval(ctx, code, stdin)?;
                Ok(r)
            }
            "find" => {
                let ctx = &self.ctx;
                let query = json_str(params.get("query"), "query")?;
                let mode = params
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fuzzy");
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let include_desc = params
                    .get("include_desc")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let refresh = params
                    .get("refresh")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                commands::skill::find(ctx, mode, &query, limit, include_desc, refresh)
            }
            "info" => {
                let ctx = &self.ctx;
                let func = json_str(params.get("func"), "func")?;
                commands::skill::info(ctx, &func)
            }
            "sync" => {
                let ctx = &self.ctx;
                let host = params.get("host").and_then(|v| v.as_str());
                commands::skill::sync_cache(ctx, None, host, false)
            }
            "cache" => {
                let ctx = &self.ctx;
                let host = params.get("host").and_then(|v| v.as_str());
                let clear = params
                    .get("clear")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                commands::skill::show_cache(ctx, host, clear)
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown skill method '{}'",
                op
            ))),
        }
    }

    /// `libref.*` — the Virtuoso library references (analogLib, basic, …).
    ///
    /// Takes no `client`: every operation is a local documentation read. That
    /// is the point — looking up a CDF parameter name must not require a live
    /// Virtuoso, an Admin token, or a guess.
    fn dispatch_libref(op: &str, params: Value) -> Result<Value> {
        let refresh = params
            .get("refresh")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let lib = params.get("lib").and_then(|v| v.as_str()).unwrap_or("");

        match op {
            "list" => {
                let category = params.get("category").and_then(|v| v.as_str());
                commands::libref::list(lib, category, refresh)
            }
            // `symbol` is optional only when `lib` is given: `libref.info
            // {lib: "rfLib"}` asks about the library itself, and the answer is
            // why it has no symbols here.
            "info" => {
                let symbol = match params.get("symbol").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None if !lib.is_empty() => String::new(),
                    None => json_str(params.get("symbol"), "symbol")?,
                };
                commands::libref::info(&symbol, lib, refresh)
            }
            "find" => {
                let query = json_str(params.get("query"), "query")?;
                let mode = params
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fuzzy");
                let scope = params
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("params");
                commands::libref::find(&query, mode, scope, lib, limit, refresh)
            }
            _ => Err(VirtuosoError::NotFound(format!(
                "unknown libref method '{op}'"
            ))),
        }
    }

    fn dispatch_sim(&self, _client: &VirtuosoClient, op: &str, _params: Value) -> Result<Value> {
        match op {
            "check_license" => {
                let _ = _client;
                commands::sim::check_license()
            }
            _ => Err(VirtuosoError::Execution(format!(
                "unknown sim method '{}'",
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

/// Optional schematic coordinate, in user units.
///
/// Absent means `default`; **present-but-not-a-number is an error**, not the
/// default. The previous `json_i64_or` did `as_i64().unwrap_or(0)`, so both
/// `{"x": -1.5}` and `{"x": "7"}` placed the instance at the origin and
/// returned `status: ok` — a schematic that is quietly wrong, which costs more
/// than one that refuses to build.
///
/// Floats are accepted because `dbCreateInst`'s `l_point` is in user units
/// (IC23.1 `skdfref.fnd`: *"an origin and orientation specified by l_point"*),
/// and the schematic grid is 0.0625 — half the placements in `amp` are not
/// integers.
fn json_coord_or(value: Option<&Value>, field: &str, default: f64) -> Result<f64> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| VirtuosoError::Execution(format!(
                "field '{field}' must be a finite number in user units, got {v}"
            ))),
    }
}

/// Parse `points: ["x1,y1", "x2,y2", …]` into user-unit coordinate pairs.
///
/// A wire with fewer points than the caller listed is not a wire the caller
/// asked for, so every failure is reported with the offending element rather
/// than skipped.
/// Map a view *name* onto the DFII cellViewType `dbOpenCellViewByType` needs
/// to create it.
///
/// These are not the same string, and getting it wrong is not a soft failure:
/// the manual says "An error occurs if you try to create a cellview with an
/// unsupported type". `symbol` → `schematicSymbol` and `layout` → `maskLayout`
/// are the two that catch people out.
///
/// Only the view names whose type is unambiguous are mapped. Anything else —
/// `maestro`, `config`, a site-specific view — returns `None` so the caller is
/// told to name the type instead of having one guessed for it.
fn cell_view_type_for(view: &str) -> Option<String> {
    let t = match view {
        "schematic" => "schematic",
        "symbol" => "schematicSymbol",
        "layout" => "maskLayout",
        "netlist" => "netlist",
        _ => return None,
    };
    Some(t.to_string())
}

fn parse_wire_points(value: Option<&Value>) -> Result<Vec<(f64, f64)>> {
    let arr = value.and_then(|v| v.as_array()).ok_or_else(|| {
        VirtuosoError::Execution("field 'points' must be an array of \"x,y\" strings".into())
    })?;
    let bad = |i: usize, what: &str| {
        VirtuosoError::Execution(format!("points[{i}]: {what}"))
    };
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            let s = v.as_str().ok_or_else(|| bad(i, "expected a \"x,y\" string"))?;
            let (x, y) = s
                .split_once(',')
                .ok_or_else(|| bad(i, &format!("'{s}' is not in x,y form")))?;
            let num = |t: &str| -> Result<f64> {
                t.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .ok_or_else(|| bad(i, &format!("'{t}' is not a finite number")))
            };
            Ok((num(x)?, num(y)?))
        })
        .collect::<Result<Vec<_>>>()
        .and_then(|pts| {
            // One point is not a wire. Rejecting it here names the parameter;
            // letting it through makes `schCreateWire` return nil, which the
            // caller sees as a cellview problem.
            if pts.len() < 2 {
                return Err(VirtuosoError::Execution(format!(
                    "field 'points' needs at least two points to make a wire, got {}",
                    pts.len()
                )));
            }
            Ok(pts)
        })
}

/// Fold a SKILL-produced JSON object into a `{"status":"ok", ...}` reply.
///
/// The payload arrives wrapped in whatever quoting the bridge used to ship a
/// SKILL string back, so `sprintf`'s `{"lib":...}` reaches us as the literal
/// `"{\"lib\":...}"` — a JSON *string* whose contents are themselves JSON.
/// One unwrap pass turns that back into an object; anything else falls back to
/// `{"status":"ok","output":<raw>}`, so a SKILL change can never turn a
/// successful call into a dispatch error.
fn merge_status_ok(payload: &str) -> Value {
    let parsed = match serde_json::from_str::<Value>(payload) {
        Ok(Value::String(inner)) => serde_json::from_str::<Value>(&inner).ok(),
        Ok(other) => Some(other),
        Err(_) => None,
    };
    match parsed {
        Some(Value::Object(mut map)) => {
            map.insert("status".into(), Value::String("ok".into()));
            Value::Object(map)
        }
        _ => serde_json::json!({ "status": "ok", "output": payload }),
    }
}

/// Required numeric field. Accepts integers as well as reals — JSON has no
/// distinct integer type, so `{"x": 2}` and `{"x": 2.0}` must both work.
fn json_f64(value: Option<&Value>, field: &str) -> Result<f64> {
    value
        .and_then(|v| v.as_f64())
        .ok_or_else(|| VirtuosoError::Execution(format!("missing required field: {}", field)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::VirtuosoResult;
    use crate::rpc::schema::{standard_schema, RpcSchema};

    /// Defect N: an argument the method never reads used to vanish, and the
    /// answer looked exactly like a real one — `list_instances` reported the
    /// open cellview's instances for a cell that does not exist.
    #[test]
    fn an_undeclared_param_is_refused_not_silently_dropped() {
        use serde_json::json;

        let err = reject_undeclared_params(
            "schematic.list_instances",
            &json!({"lib": "DESIGN_LIB", "cell": "no_such_cell_xyz"}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("'cell'") && err.contains("'lib'"), "{err}");
        // Say what it does instead, or the caller just guesses again.
        assert!(err.contains("takes no parameters"), "{err}");
        assert!(err.contains("open cellview"), "{err}");

        let err = reject_undeclared_params("schematic.place", &json!({"master": "analogLib/res", "name": "R1", "z": 3}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("'z'"), "{err}");
        assert!(err.contains("orient"), "expected the accepted list: {err}");
        assert!(!err.contains("'master'"), "declared params must not be listed: {err}");
    }

    /// The check must stay out of the way of every legitimate call: declared
    /// params (required or optional), empty params, and plugin domains the
    /// built-in schema knows nothing about.
    #[test]
    fn declared_params_and_plugin_domains_pass_through() {
        use serde_json::json;

        for params in [
            json!({"lib": "DESIGN_LIB", "cell": "amp"}),
            json!({"lib": "L", "cell": "C", "view": "schematic"}),
        ] {
            assert!(reject_undeclared_params("schematic.open_cell_view", &params).is_ok());
        }
        for params in [json!({}), json!(null), json!("nonsense")] {
            assert!(reject_undeclared_params("schematic.list_instances", &params).is_ok());
        }
        // Not in the built-in schema => plugin contract, not ours to police.
        assert!(reject_undeclared_params("myplugin.do_thing", &json!({"anything": 1})).is_ok());
    }

    /// The mechanical half of defect N's fix: the schema can only be trusted to
    /// gate parameters if it declares every parameter the code actually reads.
    /// Scan each `dispatch_*` arm for `params.get("…")` and require the schema
    /// to know that name. Two were missing when this was written
    /// (`dismiss_dialog_x11`'s `window_id`, `dismiss_window_x11`'s `pid`) —
    /// enforcing without this test would have broken both.
    #[test]
    fn every_param_a_dispatch_arm_reads_is_declared_in_the_schema() {
        let src = include_str!("dispatcher.rs");
        let declared: std::collections::HashMap<String, Vec<String>> = standard_schema()
            .methods
            .into_iter()
            .map(|m| (m.name, m.params.into_iter().map(|p| p.name).collect()))
            .collect();

        let mut domain = String::new();
        let mut method = String::new();
        let mut arm_indent = usize::MAX;
        let mut checked = 0usize;

        for line in src.lines() {
            // Stop before this very module: it quotes `params.get("…")` in a
            // doc comment, and self-inclusion would scan that as real code.
            if line.starts_with("#[cfg(test)]") {
                break;
            }
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("fn ") {
                // Any other function ends the arm-scanning window.
                domain = rest
                    .strip_prefix("dispatch_")
                    .and_then(|r| r.split('(').next())
                    .filter(|name| *name != "inner") // routes domains, owns no arms
                    .unwrap_or("")
                    .to_string();
                method.clear();
                arm_indent = usize::MAX;
            }
            if domain.is_empty() {
                continue;
            }
            let indent = line.len() - trimmed.len();
            // A match arm on `op`: `"name" => ...`, possibly with `| "alias"`
            // or an `if` guard between. Nested matches sit deeper and are
            // skipped by the indent rule.
            if trimmed.starts_with('"') && line.contains("=>") && indent <= arm_indent {
                if let Some(head) = line.split("=>").next() {
                    if let Some(op) = head.split('"').nth(1) {
                        arm_indent = indent;
                        method = format!("{domain}.{op}");
                    }
                }
            }
            if method.is_empty() {
                continue;
            }
            let mut rest = line;
            while let Some(i) = rest.find("params.get(\"") {
                rest = &rest[i + "params.get(\"".len()..];
                let Some(end) = rest.find('"') else { break };
                let name = &rest[..end];
                if let Some(names) = declared.get(&method) {
                    checked += 1;
                    assert!(
                        names.iter().any(|d| d == name),
                        "{method} reads params[{name:?}] but schema.rs does not declare it \
                         (declared: {names:?}). Add it to standard_schema(), or the \
                         dispatcher will now reject every call that passes it."
                    );
                }
            }
        }
        // A parse that silently matched nothing would pass vacuously.
        assert!(checked > 100, "only {checked} param reads found — parser is broken");
    }

    /// The MCP surface advertises its own JSON Schema per tool. Anything it
    /// tells a client to send has to be a parameter the RPC schema declares,
    /// or the dispatcher will refuse the very call MCP invited.
    #[test]
    fn every_mcp_tool_input_is_declared_in_the_rpc_schema() {
        let declared: std::collections::HashMap<String, Vec<String>> = standard_schema()
            .methods
            .into_iter()
            .map(|m| (m.name, m.params.into_iter().map(|p| p.name).collect()))
            .collect();

        // Unfiltered: a tool hidden from the default capability set would still
        // be advertised to an Admin client, so it has to satisfy the contract.
        for tool in crate::mcp::tools::all_tools_unfiltered() {
            let Some(names) = declared.get(&tool.rpc_method) else {
                continue; // plugin tool
            };
            let Some(props) = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.as_object())
            else {
                continue;
            };
            for key in props.keys() {
                assert!(
                    names.iter().any(|d| d == key),
                    "MCP tool '{}' advertises input {key:?} but {} does not declare it \
                     (declared: {names:?})",
                    tool.name,
                    tool.rpc_method
                );
            }
        }
    }

    /// Omitted is 0; present-but-junk is an error.
    ///
    /// This is the whole of defect B: `{"x": "7"}` used to place at the origin
    /// and answer `status: ok`.
    #[test]
    fn a_coordinate_is_either_absent_or_a_number() {
        use serde_json::json;
        assert_eq!(json_coord_or(None, "x", 0.0).unwrap(), 0.0);
        assert_eq!(json_coord_or(Some(&json!(null)), "x", 0.0).unwrap(), 0.0);
        assert_eq!(json_coord_or(Some(&json!(-1.5)), "x", 0.0).unwrap(), -1.5);
        assert_eq!(json_coord_or(Some(&json!(4)), "x", 0.0).unwrap(), 4.0);

        for junk in [json!("7"), json!(true), json!([1]), json!({})] {
            let err = json_coord_or(Some(&junk), "x", 0.0).unwrap_err();
            assert!(err.to_string().contains("must be a finite number"), "{junk}: {err}");
        }
    }

    /// A wire is the points the caller listed, or it is an error.
    #[test]
    fn every_wire_point_must_parse_or_the_call_fails() {
        use serde_json::json;
        let pts = parse_wire_points(Some(&json!(["0,0", "10,10", "-1.5, 4.0625"]))).unwrap();
        assert_eq!(pts, vec![(0.0, 0.0), (10.0, 10.0), (-1.5, 4.0625)]);

        // The old filter_map answered ok with a two-point wire here.
        let err = parse_wire_points(Some(&json!(["0,0", "10,ten", "20,20"]))).unwrap_err();
        assert!(err.to_string().contains("points[1]"), "{err}");
        assert!(parse_wire_points(Some(&json!(["0,0", 5]))).is_err());
        assert!(parse_wire_points(Some(&json!(["00"]))).is_err());
        assert!(parse_wire_points(None).is_err());
    }

    #[test]
    fn required_skill_result_rejects_skill_nil() {
        assert!(require_skill_result(VirtuosoResult::success("nil"), "save").is_err());
    }

    /// The bridge hands back a SKILL `sprintf` result still wrapped in quotes,
    /// so the payload is a JSON string containing JSON. Measured 2026-09-09:
    /// before the unwrap pass, `schematic.open_cell_view` replied with a
    /// `"output"` blob of backslashes instead of the `lib`/`cell`/`view` fields
    /// it exists to surface.
    #[test]
    fn merge_status_ok_unwraps_a_quoted_json_payload() {
        let v = merge_status_ok(r#""{\"lib\":\"SIM_LIB\",\"windowed\":false}""#);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["lib"], "SIM_LIB");
        assert_eq!(v["windowed"], false);
    }

    #[test]
    fn merge_status_ok_accepts_a_bare_json_object() {
        let v = merge_status_ok(r#"{"cell":"amp"}"#);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["cell"], "amp");
    }

    #[test]
    fn merge_status_ok_keeps_unparsable_payloads_as_output() {
        let v = merge_status_ok("t");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["output"], "t");
    }

    #[test]
    fn cell_write_operations_reject_skill_nil() {
        for operation in ["open cell", "save cell", "close cell"] {
            let error =
                require_cell_write_result(VirtuosoResult::success("nil"), operation).unwrap_err();
            assert!(error.to_string().contains(operation));
        }
    }

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
        assert!(
            names.contains(&"schematic.get_params"),
            "should have get_params"
        );
        assert!(
            names.contains(&"schematic.set_param"),
            "should have set_param"
        );
        assert!(
            names.contains(&"schematic.assign_net"),
            "should have assign_net"
        );
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
        assert!(
            names.contains(&"window.dismiss_dialog_x11"),
            "should have window.dismiss_dialog_x11"
        );
        assert!(
            names.contains(&"window.list_windows_x11"),
            "should have window.list_windows_x11"
        );
        assert!(
            names.contains(&"window.dismiss_window_x11"),
            "should have window.dismiss_window_x11"
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
    fn schema_contains_maestro_create_test() {
        let schema = standard_schema();
        let m = schema
            .methods
            .iter()
            .find(|m| m.name == "maestro.create_test")
            .expect("maestro.create_test should exist");
        // lib/cell/view come from maeCreateTest's own keywords, so a fresh test
        // needs no follow-up set_design call.
        let names: Vec<&str> = m.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["session", "test", "lib", "cell", "view", "simulator"]
        );
        let required = |n: &str| m.params.iter().find(|p| p.name == n).unwrap().required;
        assert!(!required("view"), "view defaults to schematic");
        assert!(!required("simulator"), "simulator defaults to spectre");
    }

    #[test]
    fn schema_set_design_test_param_is_optional() {
        let schema = standard_schema();
        let m = schema
            .methods
            .iter()
            .find(|m| m.name == "maestro.set_design")
            .unwrap();
        let p = m
            .params
            .iter()
            .find(|p| p.name == "test")
            .expect("set_design should accept an optional test");
        assert!(
            !p.required,
            "omitting it must keep the set-all-tests behaviour"
        );
    }

    #[test]
    fn schema_total_method_count() {
        let schema = standard_schema();
        // 76 upstream base
        //  + 2 set_param packet   (schematic.set_param, schematic.assign_net)
        //  + 4 maestro packet     (list_tests, delete_var, delete_output, delete_analysis)
        //  + 4 this packet        (schematic.list_cdf_params, cell.list_open,
        //                          maestro.set_session_mode, maestro.create_test)
        //  + 3 libref             (list, info, find) — the library references
        assert_eq!(schema.methods.len(), 90, "should have exactly 90 methods");
    }

    #[test]
    fn schema_contains_cell_list_open() {
        // The diagnostic for orphaned edit locks: a cellview open in mode "a"
        // with no window is invisible to both `cell.info` (current cellview
        // only) and `window.list` (windows only), so without this method the
        // only way to find one is to list OA lock files over SSH.
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"cell.list_open"),
            "should have cell.list_open"
        );
    }

    #[test]
    fn schema_maestro_open_session_defaults_to_read_only() {
        // The default is the whole point: append mode takes an OA edit lock on
        // a session that has no window, and a human then cannot open the cell
        // in the GUI with nothing anywhere to click. Whoever changes this
        // default should have to change this test too.
        let schema = standard_schema();
        let m = schema
            .methods
            .iter()
            .find(|m| m.name == "maestro.open_session")
            .expect("should have maestro.open_session");
        let mode = m
            .params
            .iter()
            .find(|p| p.name == "mode")
            .expect("open_session should take a mode param");
        assert!(!mode.required, "mode must be optional");
        assert!(
            mode.description.contains("\"r\""),
            "mode description should document the read-only default: {}",
            mode.description
        );
    }

    #[test]
    fn schema_contains_maestro_set_session_mode() {
        // This is what makes the read-only default affordable: the lock is
        // taken for the duration of the writes instead of the session.
        let schema = standard_schema();
        let names: Vec<&str> = schema.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"maestro.set_session_mode"),
            "should have maestro.set_session_mode"
        );
    }

    #[test]
    fn schema_contains_create_corner_netlist() {
        let schema = standard_schema();
        let method = schema
            .methods
            .iter()
            .find(|m| m.name == "maestro.create_corner_netlist")
            .expect("maestro.create_corner_netlist must be registered");

        // All four params are required.
        for name in ["session", "test", "corner", "output_dir"] {
            assert!(
                method.params.iter().any(|p| p.name == name && p.required),
                "maestro.create_corner_netlist must require '{name}'"
            );
        }
        // No optional params.
        assert_eq!(
            method.params.iter().filter(|p| !p.required).count(),
            0,
            "maestro.create_corner_netlist should have no optional params"
        );
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

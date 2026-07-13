use crate::client::bridge::VirtuosoClient;
use crate::client::skill_runtime::{
    require_identifier, require_non_nil, require_transport, string_literal,
};
use crate::error::{Result, VirtuosoError};
use crate::ocean;
use crate::ocean::corner::CornerConfig;
use crate::spectre::jobs::Job;
use crate::spectre::runner::SpectreSimulator;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn setup(lib: &str, cell: &str, view: &str, simulator: &str) -> Result<Value> {
    require_identifier(simulator, "simulator")?;
    let client = VirtuosoClient::from_env()?;
    let skill = ocean::setup_skill(lib, cell, view, simulator);
    let result = client.execute_skill(&skill, None)?;

    let output = require_non_nil(&result, "set up simulation")?;

    Ok(json!({
        "status": "success",
        "simulator": simulator,
        "design": { "lib": lib, "cell": cell, "view": view },
        "results_dir": output.trim_matches('"'),
    }))
}

pub fn run(analysis: &str, params: &HashMap<String, String>, timeout: u64) -> Result<Value> {
    require_identifier(analysis, "analysis")?;
    for name in params.keys() {
        require_identifier(name, "analysis parameter")?;
    }
    let client = VirtuosoClient::from_env()?;

    // Check if resultsDir is set — do NOT override if it is, as changing
    // resultsDir while an ADE session is active causes run() to silently
    // return nil (ADE binds the session to a specific results path).
    let rdir = client.execute_skill("resultsDir()", None)?;
    let rdir_val = require_transport(&rdir, "read results directory")?.trim_matches('"');
    if rdir_val == "nil" || rdir_val.is_empty() {
        return Err(VirtuosoError::Execution(
            "resultsDir is not set. Run `virtuoso sim setup` first, or open \
             ADE L for your testbench and run at least one simulation to \
             establish the session path."
                .into(),
        ));
    }

    // Send analysis setup
    let analysis_skill = ocean::analysis_skill_simple(analysis, params);
    let analysis_result = client.execute_skill(&analysis_skill, None)?;
    require_non_nil(&analysis_result, "configure simulation analysis")?;

    // Send save
    let _ = client.execute_skill("save('all)", None);

    // Execute run
    let result = client.execute_skill("run()", Some(timeout))?;
    let run_output = require_transport(&result, "run simulation")?;

    // Get actual results dir
    let rdir = client.execute_skill("resultsDir()", None)?;
    let results_dir = require_non_nil(&rdir, "read simulation results directory")?
        .trim_matches('"')
        .to_string();

    // Validate: run() returning nil usually means simulation didn't execute
    if run_output.trim_matches('"') == "nil" {
        let spectre_out = format!("{results_dir}/psf/spectre.out");
        let check = client.execute_skill(
            &format!("isFile({})", string_literal(&spectre_out)),
            None,
        )?;
        let has_spectre_out =
            require_transport(&check, "check spectre output")?.trim_matches('"');
        if has_spectre_out == "nil" || has_spectre_out == "0" {
            return Err(VirtuosoError::Execution(
                "Simulation failed: run() returned nil and no spectre.out found. \
                 The netlist may be missing or stale — regenerate via ADE \
                 (Simulation → Netlist and Run) or `virtuoso sim netlist`."
                    .into(),
            ));
        }
    }

    Ok(json!({
        "status": "success",
        "analysis": analysis,
        "params": params,
        "results_dir": results_dir,
        "execution_time": result.execution_time,
    }))
}

pub fn measure(analysis: &str, exprs: &[String]) -> Result<Value> {
    require_identifier(analysis, "analysis")?;
    let client = VirtuosoClient::from_env()?;

    // Open results from resultsDir PSF and select result type
    let rdir = client.execute_skill("resultsDir()", None)?;
    let rdir_val = require_transport(&rdir, "read results directory")?.trim_matches('"');
    if rdir_val != "nil" && !rdir_val.is_empty() {
        let open_skill = format!(
            "openResults({})",
            string_literal(&format!("{rdir_val}/psf"))
        );
        let _ = client.execute_skill(&open_skill, None);
    }
    let select_skill = format!("selectResult('{analysis})");
    let _ = client.execute_skill(&select_skill, None);

    // Execute each measure expression individually for reliability
    let mut measures = Vec::new();
    for expr in exprs {
        let result = client.execute_skill(expr, None)?;
        let value = match require_transport(&result, "measure waveform") {
            Ok(output) => output.trim_matches('"').to_string(),
            Err(error) => format!("ERROR: {error}"),
        };
        measures.push(json!({
            "expr": expr,
            "value": value,
        }));
    }

    // Detect all-nil results and provide diagnostics
    let all_nil = !measures.is_empty()
        && measures.iter().all(|m| {
            m.get("value")
                .and_then(|v| v.as_str())
                .map(|s| s == "nil")
                .unwrap_or(false)
        });

    let mut warnings: Vec<String> = Vec::new();
    if all_nil {
        let spectre_out = format!("{rdir_val}/psf/spectre.out");
        let spectre_exists = client
            .execute_skill(
                &format!("isFile({})", string_literal(&spectre_out)),
                None,
            )
            .ok()
            .and_then(|r| require_transport(&r, "check spectre output").ok().map(str::to_owned))
            .map(|output| {
                let v = output.trim_matches('"');
                v != "nil" && v != "0"
            })
            .unwrap_or(false);

        if !spectre_exists {
            warnings.push(
                "All measurements returned nil. No spectre.out found — simulation \
                 may not have run. Check netlist with `virtuoso sim netlist`."
                    .into(),
            );
        } else {
            warnings.push(
                "All measurements returned nil. Spectre ran but produced no matching \
                 data — verify signal names match your schematic and that the correct \
                 analysis type is selected."
                    .into(),
            );
        }
    }

    Ok(json!({
        "status": "success",
        "measures": measures,
        "warnings": warnings,
    }))
}

pub fn sweep(
    var: &str,
    from: f64,
    to: f64,
    step: f64,
    analysis: &str,
    measure_exprs: &[String],
    timeout: u64,
) -> Result<Value> {
    require_identifier(analysis, "analysis")?;
    let client = VirtuosoClient::from_env()?;

    // Generate value list
    let mut values = Vec::new();
    let mut v = from;
    while v <= to + step * 0.01 {
        values.push(v);
        v += step;
    }

    let skill = ocean::sweep_skill(var, &values, analysis, measure_exprs);
    let result = client.execute_skill(&skill, Some(timeout))?;
    let output = require_non_nil(&result, "run simulation sweep")?;
    let parsed = ocean::parse_skill_list(output);

    let mut headers = vec![var.to_string()];
    headers.extend(measure_exprs.iter().cloned());

    let rows: Vec<Value> = parsed
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, h) in headers.iter().enumerate() {
                if let Some(val) = row.get(i) {
                    obj.insert(h.clone(), json!(val));
                }
            }
            Value::Object(obj)
        })
        .collect();

    Ok(json!({
        "status": "success",
        "variable": var,
        "points": values.len(),
        "headers": headers,
        "data": rows,
        "execution_time": result.execution_time,
    }))
}

pub fn corner(file: &str, timeout: u64) -> Result<Value> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| VirtuosoError::NotFound(format!("corner config not found: {file}: {e}")))?;

    let config: CornerConfig = serde_json::from_str(&content)
        .map_err(|e| VirtuosoError::Config(format!("invalid corner config: {e}")))?;

    if let Some(simulator) = config.simulator.as_deref() {
        require_identifier(simulator, "simulator")?;
    }
    require_identifier(&config.analysis.analysis_type, "analysis")?;
    for name in config.analysis.params.keys() {
        require_identifier(name, "analysis parameter")?;
    }

    let client = VirtuosoClient::from_env()?;
    let skill = ocean::corner_skill(&config);
    let result = client.execute_skill(&skill, Some(timeout))?;
    let output = require_non_nil(&result, "run corner simulation")?;
    let parsed = ocean::parse_skill_list(output);

    let mut headers = vec!["corner".to_string(), "temp".to_string()];
    headers.extend(config.measures.iter().map(|m| m.name.clone()));

    let rows: Vec<Value> = parsed
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, h) in headers.iter().enumerate() {
                if let Some(val) = row.get(i) {
                    obj.insert(h.clone(), json!(val));
                }
            }
            Value::Object(obj)
        })
        .collect();

    Ok(json!({
        "status": "success",
        "corners": config.corners.len(),
        "measures": config.measures.len(),
        "headers": headers,
        "data": rows,
        "execution_time": result.execution_time,
    }))
}

pub fn results() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let result = client.execute_skill("resultsDir()", None)?;
    let dir = require_non_nil(&result, "read results directory")?
        .trim_matches('"')
        .to_string();

    // Query available result types
    let types_result = client.execute_skill(
        &format!(
            "let((dir files) dir={} when(isDir(dir) files=getDirFiles(dir)) files)",
            string_literal(&dir)
        ),
        None,
    )?;
    let contents = require_transport(&types_result, "list result files")?;

    Ok(json!({
        "status": "success",
        "results_dir": dir,
        "contents": contents,
    }))
}

pub fn netlist(recreate: bool) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    // Method 1: Ocean createNetlist
    let r1 = client.execute_skill(
        if recreate {
            "createNetlist(?recreateAll t ?display nil)"
        } else {
            "createNetlist(?display nil)"
        },
        Some(60),
    )?;
    if let Ok(r1_out) = require_non_nil(&r1, "create netlist") {
        return Ok(json!({
            "status": "success",
            "method": "createNetlist",
            "output": r1_out.trim_matches('"'),
        }));
    }

    // Method 2: ASI session-based netlisting
    let r2 = client.execute_skill(
        "asiCreateNetlist(asiGetSession(hiGetCurrentWindow()))",
        Some(60),
    )?;
    if let Ok(r2_out) = require_non_nil(&r2, "create ASI netlist") {
        return Ok(json!({
            "status": "success",
            "method": "asiCreateNetlist",
            "output": r2_out.trim_matches('"'),
        }));
    }

    Err(VirtuosoError::Execution(
        "Cannot create netlist programmatically. \
         Open ADE L for this cell and run Simulation → Netlist and Run."
            .into(),
    ))
}

// ── Async job commands ──────────────────────────────────────────────

pub fn run_async(netlist_path: &str) -> Result<Value> {
    let content = std::fs::read_to_string(netlist_path)
        .map_err(|e| VirtuosoError::Config(format!("Cannot read netlist '{netlist_path}': {e}")))?;
    let sim = SpectreSimulator::from_env()?;
    let job = sim.run_async(&content)?;
    Ok(json!({
        "status": "launched",
        "job_id": job.id,
        "pid": job.pid,
        "netlist": netlist_path,
    }))
}

pub fn job_status(id: &str) -> Result<Value> {
    let mut job = Job::load(id)?;
    job.refresh()?;
    serde_json::to_value(&job).map_err(|e| VirtuosoError::Execution(e.to_string()))
}

pub fn job_list() -> Result<Value> {
    let mut jobs = Job::list_all()?;
    for job in &mut jobs {
        let _ = job.refresh();
    }
    Ok(json!({
        "count": jobs.len(),
        "jobs": serde_json::to_value(&jobs).unwrap_or_default(),
    }))
}

pub fn job_cancel(id: &str) -> Result<Value> {
    let mut job = Job::load(id)?;
    job.cancel()?;
    Ok(json!({
        "status": "cancelled",
        "job_id": id,
    }))
}

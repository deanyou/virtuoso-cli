use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use crate::ocean;
use crate::ocean::corner::CornerConfig;
use crate::spectre::jobs::Job;
use crate::spectre::runner::SpectreSimulator;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn setup(lib: &str, cell: &str, view: &str, simulator: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = ocean::setup_skill(lib, cell, view, simulator);
    let result = client.execute_skill(&skill, None)?;

    if !result.ok() {
        return Err(VirtuosoError::Execution(result.errors.join("; ")));
    }

    Ok(json!({
        "status": "success",
        "simulator": simulator,
        "design": { "lib": lib, "cell": cell, "view": view },
        "results_dir": result.output.trim().trim_matches('"'),
    }))
}

pub fn run(analysis: &str, params: &HashMap<String, String>, timeout: u64) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    // Check if resultsDir is set — do NOT override if it is, as changing
    // resultsDir while an ADE session is active causes run() to silently
    // return nil (ADE binds the session to a specific results path).
    let rdir = client.execute_skill("resultsDir()", None)?;
    let rdir_val = rdir.output.trim().trim_matches('"');
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
    if !analysis_result.ok() {
        return Err(VirtuosoError::Execution(analysis_result.errors.join("; ")));
    }

    // Send save
    let _ = client.execute_skill("save('all)", None);

    // Execute run
    let result = client.execute_skill("run()", Some(timeout))?;
    if !result.ok() {
        return Err(VirtuosoError::Execution(result.errors.join("; ")));
    }

    // Get actual results dir
    let rdir = client.execute_skill("resultsDir()", None)?;
    let results_dir = rdir.output.trim().trim_matches('"').to_string();

    // Validate: run() returning nil usually means simulation didn't execute
    let run_output = result.output.trim().trim_matches('"');
    if run_output == "nil" {
        let check =
            client.execute_skill(&format!(r#"isFile("{results_dir}/psf/spectre.out")"#), None)?;
        let has_spectre_out = check.output.trim().trim_matches('"');
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

/// Reject SKILL expressions that could cause side effects outside of waveform access.
/// `measure` is intended for read-only PSF queries; block known destructive/execution APIs.
fn validate_measure_expr(expr: &str) -> Result<()> {
    // Case-insensitive prefix patterns that indicate non-measurement operations
    let blocked: &[&str] = &[
        "system(",
        "sh(",
        "ipcbeginprocess(",
        "ipcwriteprocess(",
        "ipckillprocess(",
        "deletefile(",
        "deletedir(",
        "copyfile(",
        "movefile(",
        "writefile(",
        "createdir(",
        "load(",
        "evalstring(",
        "hiloaddmenu(",
    ];
    let lower = expr.to_lowercase();
    for pat in blocked {
        if lower.contains(pat) {
            return Err(VirtuosoError::Execution(format!(
                "measure expression contains blocked function '{pat}': \
                 only waveform access functions are allowed"
            )));
        }
    }
    Ok(())
}

pub fn measure(analysis: &str, exprs: &[String]) -> Result<Value> {
    for expr in exprs {
        validate_measure_expr(expr)?;
    }

    let client = VirtuosoClient::from_env()?;

    // Open results from resultsDir PSF and select result type
    let rdir = client.execute_skill("resultsDir()", None)?;
    let rdir_val = rdir.output.trim().trim_matches('"');
    if rdir_val != "nil" && !rdir_val.is_empty() {
        let open_skill = format!("openResults(\"{rdir_val}/psf\")");
        let _ = client.execute_skill(&open_skill, None);
    }
    let select_skill = format!("selectResult('{analysis})");
    let _ = client.execute_skill(&select_skill, None);

    // Execute each measure expression individually for reliability
    let mut measures = Vec::new();
    for expr in exprs {
        let result = client.execute_skill(expr, None)?;
        let value = if result.ok() {
            result.output.trim().trim_matches('"').to_string()
        } else {
            format!("ERROR: {}", result.errors.join("; "))
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
        let rdir_for_check = rdir_val.to_string();
        let spectre_exists = client
            .execute_skill(
                &format!(r#"isFile("{rdir_for_check}/psf/spectre.out")"#),
                None,
            )
            .map(|r| {
                let v = r.output.trim().trim_matches('"');
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

    if !result.ok() {
        return Err(VirtuosoError::Execution(result.errors.join("; ")));
    }

    let parsed = ocean::parse_skill_list(result.output.trim());

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

    let client = VirtuosoClient::from_env()?;
    let skill = ocean::corner_skill(&config);
    let result = client.execute_skill(&skill, Some(timeout))?;

    if !result.ok() {
        return Err(VirtuosoError::Execution(result.errors.join("; ")));
    }

    let parsed = ocean::parse_skill_list(result.output.trim());

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

    if !result.ok() {
        return Err(VirtuosoError::Execution(result.errors.join("; ")));
    }

    let dir = result.output.trim().trim_matches('"').to_string();

    // Query available result types
    let types_result = client.execute_skill(
        &format!(r#"let((dir files) dir="{dir}" when(isDir(dir) files=getDirFiles(dir)) files)"#),
        None,
    )?;

    Ok(json!({
        "status": "success",
        "results_dir": dir,
        "contents": types_result.output.trim(),
    }))
}

pub fn netlist(lib: &str, cell: &str, view: &str, recreate: bool) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    // Step 1: Establish Ocean session (simulator + design) so createNetlist has
    // a target even on a cold start without a prior ADE session.
    // setup_skill ends with resultsDir() — may return "nil" if not yet bound;
    // that's acceptable here since createNetlist returns the path directly.
    let setup = ocean::setup_skill(lib, cell, view, "spectre");
    let sr = client.execute_skill(&setup, None)?;
    if !sr.ok() {
        return Err(VirtuosoError::Execution(format!(
            "sim setup failed before netlisting: {}",
            sr.errors.join("; ")
        )));
    }

    // Step 2: createNetlist — returns the netlist file path on success, nil on failure.
    let create_cmd = if recreate {
        "createNetlist(?recreateAll t ?display nil)"
    } else {
        "createNetlist(?display nil)"
    };
    let nr = client.execute_skill(create_cmd, Some(60))?;
    let nr_out = nr.output.trim().trim_matches('"').to_string();

    if !nr.skill_ok() {
        let errs = if nr.errors.is_empty() { "none".into() } else { nr.errors.join("; ") };
        return Err(VirtuosoError::Execution(format!(
            "createNetlist returned nil. Errors: {errs}. \
             Ensure the schematic is saved and the PDK models are loaded."
        )));
    }

    // Step 3: Resolve the actual netlist path.
    // createNetlist returns either:
    //   (a) the full path to input.scs  — use directly
    //   (b) the resultsDir path         — append /netlist/input.scs
    //   (c) "t"                         — reuse resultsDir from setup (sr.output)
    let candidate = if nr_out.ends_with(".scs") {
        nr_out.clone()
    } else if nr_out != "t" && !nr_out.is_empty() {
        format!("{nr_out}/netlist/input.scs")
    } else {
        // createNetlist returned "t"; reuse the resultsDir captured during setup,
        // falling back to an extra SKILL call only if setup returned nil.
        let rdir_val = {
            let from_setup = sr.output.trim().trim_matches('"');
            if from_setup != "nil" && !from_setup.is_empty() {
                from_setup.to_string()
            } else {
                let rdir = client.execute_skill("resultsDir()", None)?;
                rdir.output.trim().trim_matches('"').to_string()
            }
        };
        if rdir_val == "nil" || rdir_val.is_empty() {
            return Err(VirtuosoError::Execution(
                "createNetlist returned 't' but resultsDir() is nil. \
                 Run `vcli sim setup` first or open ADE L for this cell."
                    .into(),
            ));
        }
        format!("{rdir_val}/netlist/input.scs")
    };

    // Step 4: Verify the file actually exists on disk.
    let check = client.execute_skill(&format!(r#"isFile("{candidate}")"#), None)?;
    let v = check.output.trim().trim_matches('"');
    let file_exists = v != "nil" && v != "0";

    if !file_exists {
        return Err(VirtuosoError::Execution(format!(
            "createNetlist ran but file not found at '{candidate}'. \
             createNetlist output was: '{nr_out}'. \
             Check resultsDir() and ensure write permissions."
        )));
    }

    Ok(json!({
        "status": "success",
        "netlist_path": candidate,
    }))
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

#[cfg(test)]
mod tests {
    use super::validate_measure_expr;

    #[test]
    fn safe_waveform_exprs_are_allowed() {
        for expr in &[
            "VT(\"vout\" \"VGS\")",
            "bandwidth(getData(\"vout\") 3)",
            "value(getData(\"vout\") 1e-9)",
            "getData(\"/vout\")",
            "ymax(getData(\"id\"))",
            "delay(getData(\"vout\") 0.5)",
        ] {
            assert!(
                validate_measure_expr(expr).is_ok(),
                "should be allowed: {expr}"
            );
        }
    }

    #[test]
    fn dangerous_exprs_are_blocked() {
        let cases = [
            ("system(\"id\")", "system("),
            ("sh(\"ls\")", "sh("),
            ("ipcBeginProcess(\"cmd\")", "ipcbeginprocess("),
            ("deleteFile(\"/etc/hosts\")", "deletefile("),
            ("load(\"/tmp/evil.il\")", "load("),
            ("evalstring(\"getData(1)\")", "evalstring("),
            // case-insensitive
            ("SYSTEM(\"id\")", "system("),
            ("DeleteFile(\"/tmp/x\")", "deletefile("),
        ];
        for (expr, pat) in &cases {
            let err = validate_measure_expr(expr).unwrap_err();
            assert!(
                err.to_string().contains(pat),
                "error should mention '{pat}': {err}"
            );
        }
    }
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
    let jobs_value = serde_json::to_value(&jobs)
        .map_err(|e| VirtuosoError::Execution(format!("Failed to serialize jobs: {e}")))?;
    Ok(json!({
        "count": jobs.len(),
        "jobs": jobs_value,
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

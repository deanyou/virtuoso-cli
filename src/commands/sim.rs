use crate::client::bridge::{escape_skill_string, VirtuosoClient};
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

/// Run createNetlist, auto-recovering from OSSHNL-109 ("modified since last extraction").
///
/// When a schematic is edited via SKILL (e.g. `dbSave`) without going through
/// Check & Save, Cadence marks its extraction timestamp as stale and
/// `createNetlist` returns nil.  We detect this by retrying after
/// `schCheck(cv)` + `dbSave(cv)`.
fn create_netlist_inner(
    client: &VirtuosoClient,
    lib: &str,
    cell: &str,
    view: &str,
    recreate: bool,
) -> Result<String> {
    let cmd = if recreate {
        "createNetlist(?recreateAll t ?display nil)"
    } else {
        "createNetlist(?display nil)"
    };

    // First attempt
    let nr = client.execute_skill(cmd, Some(60))?;
    let nr_out = nr.output.trim().trim_matches('"').to_string();
    if nr.skill_ok() {
        return Ok(nr_out);
    }

    // Auto-fix OSSHNL-109: run schCheck + dbSave to refresh extraction timestamp.
    // Try to open the cv in write mode; fall back to the already-open write-mode cv
    // (dbOpenCellViewByType returns nil if the cv is already held in "a" mode by Ocean).
    let lib_e = escape_skill_string(lib);
    let cell_e = escape_skill_string(cell);
    let view_e = escape_skill_string(view);
    let fix = format!(
        r#"let((cv chk) cv=dbOpenCellViewByType("{lib_e}" "{cell_e}" "{view_e}") unless(cv cv=car(setof(ocv dbGetOpenCellViews() and(ocv~>libName=="{lib_e}" ocv~>cellName=="{cell_e}" ocv~>viewName=="{view_e}" ocv~>mode=="a")))) if(cv progn(chk=schCheck(cv) when(car(chk)==0 dbSave(cv)) list(car(chk))) list(-1)))"#
    );
    let fix_r = client.execute_skill(&fix, None)?;

    // schCheck returns (errorCount warningCount); we wrapped it in list() → "(N)"
    let raw = fix_r
        .output
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let err_count: i64 = raw
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(-1);

    if err_count == -1 {
        // cv not openable — two distinct causes:
        //   (a) OSSHNL-109: library IS registered, cv held open in "a" mode by Ocean.
        //       createNetlist may have written the file; return "t" so the caller
        //       resolves via resultsDir() and verifies the file exists.
        //   (b) Library not registered in this Virtuoso session (e.g. Virtuoso was
        //       started from a directory without a cds.lib that includes the library).
        //       createNetlist silently returned nil; returning "t" would produce a
        //       confusing "file not found" error downstream.
        //
        // Distinguish by checking ddGetLibList() for the library name.
        let lib_probe = format!(r#"when(car(setof(l ddGetLibList() l~>name=="{lib_e}")) "found")"#);
        let probe_r = client.execute_skill(&lib_probe, None)?;
        let lib_found = probe_r.output.trim().trim_matches('"');
        if lib_found != "found" {
            let cwd_r = client.execute_skill("getWorkingDir()", None)?;
            let cwd = cwd_r.output.trim().trim_matches('"');
            let cwd_note = if !cwd.is_empty() && cwd != "nil" {
                format!(" Virtuoso was started from '{cwd}'.")
            } else {
                String::new()
            };
            return Err(VirtuosoError::Execution(format!(
                "Library '{lib}' is not registered in the current Virtuoso session.{cwd_note} \
                 Start Virtuoso from the project directory whose cds.lib includes '{lib}', \
                 or run hiLoadCDSLibDefs() in the CIW to register it at runtime."
            )));
        }
        return Ok("t".into());
    }
    if err_count != 0 {
        return Err(VirtuosoError::Execution(format!(
            "createNetlist failed; schematic has {err_count} check error(s) (OSSHNL-109). \
             Fix schematic connectivity errors before netlisting."
        )));
    }

    // Retry after Check and Save
    let retry = client.execute_skill(cmd, Some(60))?;
    let retry_out = retry.output.trim().trim_matches('"').to_string();
    if !retry.skill_ok() {
        let errs = if retry.errors.is_empty() {
            "none".into()
        } else {
            retry.errors.join("; ")
        };
        return Err(VirtuosoError::Execution(format!(
            "createNetlist returned nil after Check and Save. Errors: {errs}. \
             Ensure the schematic is saved and PDK models are loaded."
        )));
    }
    Ok(retry_out)
}

/// Standard analysis blocks for standalone Spectre invocation.
/// Returns `None` for unrecognised kinds so callers can warn.
fn analysis_block(kind: &str) -> Option<&'static str> {
    match kind {
        "dc" => Some(
            "dcOp dc write=\"spectre.dc\" maxiters=150 maxsteps=10000 annotate=status\n\
             dcOpInfo info what=oppoint where=rawfile\n",
        ),
        "ac" => Some("acSweep ac start=1 stop=10G dec=20 annotate=status\n"),
        "tran" => Some("tran tran stop=10u annotate=status\n"),
        _ => None,
    }
}

pub fn netlist(
    lib: &str,
    cell: &str,
    view: &str,
    recreate: bool,
    analyses: &[String],
) -> Result<Value> {
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

    // Step 2: createNetlist — auto-recovers from OSSHNL-109 via schCheck+dbSave.
    let nr_out = create_netlist_inner(&client, lib, cell, view, recreate)?;

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
        // falling back to an extra SKILL call if setup returned nil or a relative path.
        // If resultsDir is relative, prepend getWorkingDir() to make it absolute.
        let rdir_val = {
            let from_setup = sr.output.trim().trim_matches('"');
            let raw =
                if from_setup != "nil" && !from_setup.is_empty() && from_setup.starts_with('/') {
                    from_setup.to_string()
                } else {
                    let rdir = client.execute_skill("resultsDir()", None)?;
                    rdir.output.trim().trim_matches('"').to_string()
                };
            // Relative path — prepend Ocean's working directory to make it absolute.
            if !raw.is_empty() && raw != "nil" && !raw.starts_with('/') {
                let cwd_r = client.execute_skill("getWorkingDir()", None)?;
                let cwd = cwd_r.output.trim().trim_matches('"');
                if cwd != "nil" && !cwd.is_empty() {
                    format!("{cwd}/{raw}")
                } else {
                    raw
                }
            } else {
                raw
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

    // Step 5: Post-process the netlist for standalone Spectre invocation.
    let mut patched = false;
    let mut unknown_analyses: Vec<&str> = Vec::new();

    if !analyses.is_empty() {
        let mut content = std::fs::read_to_string(&candidate).map_err(|e| {
            VirtuosoError::Execution(format!("cannot read netlist '{candidate}': {e}"))
        })?;

        // Fix ADE OA-relative model path (only resolves with +adespetkn token).
        // Pattern: /oa/smic13mmrf_1233//../  →  /  (removes the indirection)
        if content.contains("/oa/smic13mmrf_1233//../") {
            content = content.replace("/oa/smic13mmrf_1233//../", "/");
            patched = true;
        }

        // Append missing analysis blocks (skip if already present).
        for kind in analyses {
            match analysis_block(kind) {
                Some(block) => {
                    let marker = match kind.as_str() {
                        "dc" => "dcOp ",
                        "ac" => "acSweep ",
                        "tran" => "tran tran",
                        _ => unreachable!(),
                    };
                    if !content.contains(marker) {
                        content.push('\n');
                        content.push_str(block);
                        patched = true;
                    }
                }
                None => unknown_analyses.push(kind),
            }
        }

        if patched {
            std::fs::write(&candidate, &content).map_err(|e| {
                VirtuosoError::Execution(format!("cannot write patched netlist '{candidate}': {e}"))
            })?;
        }
    }

    let mut out = json!({
        "status": "success",
        "netlist_path": candidate,
    });
    if patched {
        out["patched"] = json!(true);
    }
    if !unknown_analyses.is_empty() {
        out["unknown_analyses"] = json!(unknown_analyses);
    }
    Ok(out)
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

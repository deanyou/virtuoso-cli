use crate::client::bridge::{escape_skill_string, VirtuosoClient};
use crate::client::skill_runtime::{
    require_identifier, require_non_nil, require_transport, string_literal,
};
use crate::error::{Result, VirtuosoError};
use crate::models::ExecutionStatus;
use crate::ocean;
use crate::ocean::corner::CornerConfig;
use crate::spectre::jobs::Job;
use crate::spectre::runner::{ParallelSimResult, SpectreSimulator};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn setup(lib: &str, cell: &str, view: &str, simulator: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = ocean::setup_skill(lib, cell, view, simulator);
    let result = client.execute_skill(&skill, None)?;

    require_non_nil(&result, "sim setup")?;

    Ok(json!({
        "status": "success",
        "simulator": simulator,
        "design": { "lib": lib, "cell": cell, "view": view },
        "results_dir": result.output.trim().trim_matches('"'),
    }))
}

pub fn run(analysis: &str, params: &HashMap<String, String>, timeout: u64) -> Result<Value> {
    require_identifier(analysis, "analysis")?;
    for key in params.keys() {
        require_identifier(key, "analysis parameter")?;
    }
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
    require_non_nil(&analysis_result, "configure analysis")?;

    // Send save
    let _ = client.execute_skill("save('all)", None);

    // Execute run
    let result = client.execute_skill("run()", Some(timeout))?;
    require_transport(&result, "run analysis")?;

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
    require_identifier(analysis, "analysis")?;
    for expr in exprs {
        validate_measure_expr(expr)?;
    }

    let client = VirtuosoClient::from_env()?;

    // Open results from resultsDir PSF and select result type
    let rdir = client.execute_skill("resultsDir()", None)?;
    let rdir_val = rdir.output.trim().trim_matches('"');
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

/// Run multiple netlists in parallel, returning a JSON report of all outcomes.
///
/// Each input is a "label:path" pair (colon-separated, label may contain colons
/// if the netlist path is absolute). The results array preserves input order.
///
/// Example:
///   vcli sim run-parallel tt:/path/to/netlist_tt.scs ss:/path/to/netlist_ss.scs ff:/path/to/netlist_ff.scs
///
/// For each entry, the path is read as netlist content and passed to
/// SpectreSimulator::run_parallel().
pub fn run_parallel(inputs: &[(String, String)]) -> Result<Value> {
    if inputs.is_empty() {
        return Err(VirtuosoError::Config(
            "run-parallel requires at least one input (label:path pair)".into(),
        ));
    }

    // Read all netlist files into memory before parallel dispatch.
    // Each worker gets its own clone of SpectreSimulator, so we read upfront
    // to avoid file-access races and to surface I/O errors before spinning threads.
    let mut netlists: Vec<(String, String)> = Vec::with_capacity(inputs.len());
    for (label, path) in inputs {
        let content = std::fs::read_to_string(path).map_err(|e| {
            VirtuosoError::Config(format!(
                "Cannot read netlist '{path}' for label '{label}': {e}"
            ))
        })?;
        netlists.push((label.clone(), content));
    }

    let sim = SpectreSimulator::from_env()?;
    let results = sim.run_parallel(&netlists);

    Ok(parallel_report(results, inputs.len()))
}

fn parallel_report(results: Vec<ParallelSimResult>, total: usize) -> Value {
    let mut rows = Vec::with_capacity(results.len());
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for result in results {
        match result.result {
            Ok(simulation) => {
                let status = match simulation.status {
                    ExecutionStatus::Success => {
                        ok_count += 1;
                        "success"
                    }
                    ExecutionStatus::Partial => {
                        err_count += 1;
                        "partial"
                    }
                    ExecutionStatus::Failure | ExecutionStatus::Error => {
                        err_count += 1;
                        "error"
                    }
                };
                rows.push(json!({
                    "label": result.label,
                    "status": status,
                    "errors": simulation.errors,
                    "warnings": simulation.warnings,
                }));
            }
            Err(error) => {
                err_count += 1;
                rows.push(json!({
                    "label": result.label,
                    "status": "error",
                    "error": error.to_string(),
                }));
            }
        }
    }

    let summary = if ok_count == total {
        "all_ok"
    } else if err_count == total {
        "all_error"
    } else {
        "partial"
    };

    json!({
        "status": summary,
        "total": total,
        "ok": ok_count,
        "errors": err_count,
        "results": rows,
    })
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

/// Check Spectre license availability on local or remote host.
/// Queries `spectre -V` for version and `lmstat -a` for license usage.
pub fn check_license() -> Result<Value> {
    use std::process::Command;

    let cfg = crate::config::Config::from_env()?;
    let remote = cfg.is_remote();

    if remote {
        let runner = crate::transport::ssh::SSHRunner::from_config(&cfg);

        let cshrc = cfg.cadence_cshrc.as_deref();
        let cshrc_quoted = cshrc
            .map(|c| {
                shlex::try_quote(c)
                    .map(|q| q.to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let env_setup = format!(
            "HOSTNAME=`hostname 2>/dev/null || echo localhost`; export HOSTNAME; eval \"$(csh -c 'source {}; env' 2>/dev/null | grep -E '^(PATH|LM_LICENSE_FILE|CDS)=' | sed 's/^/export /')\" 2>/dev/null",
            cshrc_quoted
        );

        let check_script = format!(
            "{}; spectre -V 2>&1 | head -1; lmstat -a 2>/dev/null | grep -E 'Users of' | grep -v '0 licenses in use'",
            env_setup
        );

        let result = runner.run_command(&check_script, Some(30))?;
        let stdout = result.stdout.trim();
        let stderr = result.stderr.trim();

        let mut version = String::new();
        let mut licenses = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.starts_with("@(#)$CDS:") || line.starts_with("spectre ") {
                version = line.to_string();
            } else if line.contains("Users of") {
                licenses.push(line.to_string());
            }
        }

        Ok(json!({
            "ok": !version.is_empty(),
            "remote": true,
            "version": version,
            "licenses": licenses,
            "stderr": stderr,
        }))
    } else {
        // Local mode
        let spectre_bin = cfg.spectre_bin.as_deref().unwrap_or("spectre");

        let version_output = Command::new(spectre_bin).arg("-V").output().map_err(|e| {
            VirtuosoError::Execution(format!("Failed to run '{} -V': {}", spectre_bin, e))
        })?;

        let version = String::from_utf8_lossy(&version_output.stderr)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        let lmstat_output = Command::new("lmstat").args(["-a"]).output();

        let mut licenses = Vec::new();
        if let Ok(lm) = lmstat_output {
            for line in String::from_utf8_lossy(&lm.stdout).lines() {
                let line = line.trim();
                if line.contains("Users of") && !line.contains("0 licenses in use") {
                    licenses.push(line.to_string());
                }
            }
        }

        // Find spectre path
        let spectre_path = Command::new("which")
            .arg(spectre_bin)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Ok(json!({
            "ok": !version.is_empty(),
            "remote": false,
            "version": version,
            "spectre_path": spectre_path,
            "licenses": licenses,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{analysis_block, parallel_report, validate_measure_expr};
    use crate::models::{ExecutionStatus, SimulationResult};
    use crate::spectre::runner::ParallelSimResult;
    use std::collections::HashMap;

    fn simulation_result(status: ExecutionStatus) -> SimulationResult {
        SimulationResult {
            status,
            tool_version: None,
            data: HashMap::new(),
            operating_points: HashMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn parallel_report_is_all_ok_only_when_every_simulation_status_is_success() {
        let report = parallel_report(
            vec![
                ParallelSimResult {
                    label: "tt".to_string(),
                    result: Ok(simulation_result(ExecutionStatus::Success)),
                },
                ParallelSimResult {
                    label: "ff".to_string(),
                    result: Ok(simulation_result(ExecutionStatus::Success)),
                },
            ],
            2,
        );

        assert_eq!(report["status"], "all_ok");
        assert_eq!(report["ok"], 2);
        assert_eq!(report["errors"], 0);
    }

    #[test]
    fn parallel_report_counts_partial_simulation_result_as_non_ok() {
        let report = parallel_report(
            vec![
                ParallelSimResult {
                    label: "tt".to_string(),
                    result: Ok(simulation_result(ExecutionStatus::Success)),
                },
                ParallelSimResult {
                    label: "ss".to_string(),
                    result: Ok(simulation_result(ExecutionStatus::Partial)),
                },
            ],
            2,
        );

        assert_eq!(report["status"], "partial");
        assert_eq!(report["ok"], 1);
        assert_eq!(report["errors"], 1);
        assert_eq!(report["results"][1]["status"], "partial");
    }

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

    // ── analysis_block ────────────────────────────────────────────

    #[test]
    fn analysis_block_known_types() {
        let dc = analysis_block("dc").expect("dc should have a block");
        assert!(dc.contains("dc "), "{dc}");
        assert!(dc.contains("dcOp"), "{dc}");

        let ac = analysis_block("ac").expect("ac should have a block");
        assert!(ac.contains("acSweep"), "{ac}");
        assert!(ac.contains("dec=20"), "{ac}");

        let tran = analysis_block("tran").expect("tran should have a block");
        assert!(tran.contains("tran "), "{tran}");
        assert!(tran.contains("stop=10u"), "{tran}");
    }

    #[test]
    fn analysis_block_unknown_returns_none() {
        assert!(analysis_block("noise").is_none());
        assert!(analysis_block("xf").is_none());
        assert!(analysis_block("").is_none());
    }
}

use crate::client::bridge::VirtuosoClient;
use crate::commands::schematic::parse_skill_json;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn open(lib: &str, cell: &str, view: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.open_session(lib, cell, view);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("open session")?;
    Ok(json!({
        "status": "success",
        "session": r.output_unquoted(),
        "lib": lib,
        "cell": cell,
        "view": view,
    }))
}

pub fn close(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.close_session(session);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("close session")?;
    Ok(json!({"status": "success", "session": session}))
}

pub fn list_sessions() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_sessions();
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "list sessions failed: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn set_var(name: &str, value: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.set_var(name, value);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec(&format!("set var '{name}'"))?;
    Ok(json!({"status": "success", "variable": name, "value": value}))
}

pub fn get_var(name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_var(name);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec(&format!("get var '{name}'"))?;
    Ok(json!({
        "status": "success",
        "variable": name,
        "value": r.output_unquoted(),
    }))
}

pub fn list_vars() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_vars();
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "list vars failed: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn get_analyses(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let version = client.version()?;
    let skill = client.maestro.get_analyses(session, version);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("get analyses")?;

    // maeGetEnabledAnalysis returns a SKILL list e.g. ("ac" "dc") — parse to JSON array.
    use crate::client::skill_sexp::{parse_sexp, SexpVal};
    let analyses: Value = match parse_sexp(r.output_unquoted()) {
        Ok(SexpVal::List(items)) => {
            json!(items.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        }
        _ => json!(r.output_unquoted()),
    };

    Ok(json!({
        "status": "success",
        "session": session,
        "analyses": analyses,
    }))
}

pub fn set_analysis(session: &str, analysis_type: &str, options: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    let (options_alist, version) = match options {
        None => (None, crate::version::VirtuosoVersion::IC23),
        Some(opts) => {
            let alist = crate::client::maestro_ops::json_to_skill_alist(opts)
                .map_err(|e| VirtuosoError::Execution(format!("--options: {e}")))?;
            let ver = client.version()?;
            if !ver.is_ic25() {
                eprintln!("warning: --options is only supported on IC25; ignoring on IC23 path");
                (None, ver)
            } else {
                (Some(alist), ver)
            }
        }
    };

    let skill =
        client
            .maestro
            .set_analysis(session, analysis_type, options_alist.as_deref(), version);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("set analysis")?;
    Ok(json!({"status": "success", "session": session, "analysis": analysis_type}))
}

pub fn run(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.run_simulation(session);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("run simulation")?;
    Ok(json!({"status": "launched", "session": session}))
}

pub fn add_output(output_name: &str, test_name: &str, expr: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.add_output(output_name, test_name, expr);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("add output")?;
    Ok(json!({
        "status": "success",
        "output_name": output_name,
        "test_name": test_name,
        "expression": expr,
    }))
}

pub fn save(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.save_setup(session);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("save session")?;
    Ok(json!({"status": "success", "session": session}))
}

pub fn export(
    session: &str,
    path: &str,
    test_name: Option<&str>,
    history: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client
        .maestro
        .export_results(session, path, test_name, history);
    let r = client.execute_skill(&skill, None)?.ok_or_exec("export")?;
    Ok(json!({
        "status": "success",
        "session": session,
        "path": path,
        "test_name": test_name,
        "history": history,
        "export_path": r.output_unquoted(),
    }))
}

/// Inspect the focused ADE window and return session metadata.
///
/// Makes one SKILL call that returns the focused window title, its davSession,
/// all window names, all Maestro session names, and the run directory.
///
/// When the focused window is not an ADE window (e.g. waveform viewer), falls back to
/// auto-selecting if exactly one Maestro session exists. A second RTT is made only for
/// run_dir when the session comes from auto-select or an explicit arg != focused davSession.
pub fn session_info(session: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    let skill = client.maestro.focused_window_skill();
    let r = client.execute_skill(&skill, None)?;

    // SKILL output: (title davSession (all_titles...) (sessions...) run_dir_or_nil)
    let tokens = parse_skill_list_top_level(&r.output);
    let focused = tokens.first().and_then(|t| extract_skill_string_token(t));
    let dav_session = tokens.get(1).and_then(|t| extract_skill_string_token(t));
    let bundled_run_dir = tokens.get(4).and_then(|t| extract_skill_string_token(t));

    // Parse all available Maestro sessions from token[3] = maeGetSessions()
    let available_sessions: Vec<String> = tokens
        .get(3)
        .map(|t| {
            parse_skill_list_top_level(t)
                .into_iter()
                .filter_map(|s| extract_skill_string_token(&s))
                .collect()
        })
        .unwrap_or_default();

    let parsed = focused.as_deref().and_then(parse_ade_title);

    // Auto-select when focused window has no ADE info and exactly one session exists
    let auto_session = if parsed.is_none() && dav_session.is_none() && available_sessions.len() == 1
    {
        Some(available_sessions[0].clone())
    } else {
        None
    };

    // Resolve effective session: explicit arg → davSession from window → auto-select
    let effective_session = session
        .map(str::to_owned)
        .or_else(|| dav_session.clone())
        .or_else(|| auto_session.clone());

    // run_dir: bundled covers the focused-window case; second RTT for explicit/auto sessions
    let run_dir = if let Some(s) = session.filter(|s| Some(*s) != dav_session.as_deref()) {
        let skill2 = client.maestro.run_dir_skill(s);
        let r2 = client.execute_skill(&skill2, None)?;
        if r2.skill_ok() {
            Some(r2.output_unquoted().to_string())
        } else {
            None
        }
    } else if auto_session.is_some() {
        let s = auto_session.as_deref().unwrap();
        let skill2 = client.maestro.run_dir_skill(s);
        let r2 = client.execute_skill(&skill2, None)?;
        if r2.skill_ok() {
            Some(r2.output_unquoted().to_string())
        } else {
            None
        }
    } else {
        bundled_run_dir
    };

    Ok(json!({
        "status": "success",
        "focused_window": focused,
        "dav_session": dav_session,
        "session": effective_session,
        "application": parsed.as_ref().map(|p| p.application.as_str()),
        "lib": parsed.as_ref().map(|p| p.lib.as_str()),
        "cell": parsed.as_ref().map(|p| p.cell.as_str()),
        "view": parsed.as_ref().map(|p| p.view.as_str()),
        "editable": parsed.as_ref().map(|p| p.editable),
        "unsaved_changes": parsed.as_ref().map(|p| p.unsaved_changes),
        "run_dir": run_dir,
    }))
}

/// Tokenize the top-level elements of a SKILL list, respecting nested parens and quoted strings.
///
/// `(tok1 tok2 (sub list) "quoted str")` → `["tok1", "tok2", "(sub list)", "\"quoted str\""]`
fn parse_skill_list_top_level(s: &str) -> Vec<String> {
    let s = s.trim();
    let Some(inner) = s.strip_prefix('(') else {
        return vec![];
    };
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut current = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, in_string) {
            ('"', false) => {
                in_string = true;
                current.push(c);
            }
            ('\\', true) => {
                current.push(c);
                if let Some(n) = chars.next() {
                    current.push(n);
                }
            }
            ('"', true) => {
                in_string = false;
                current.push(c);
            }
            ('(', false) => {
                depth += 1;
                current.push(c);
            }
            (')', false) => {
                depth -= 1;
                current.push(c);
            }
            (' ' | '\t' | '\n', false) if depth == 0 => {
                let tok = current.trim().to_string();
                if !tok.is_empty() {
                    result.push(tok);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let tok = current.trim().to_string();
    if !tok.is_empty() {
        result.push(tok);
    }
    result
}

/// Extract the string value from a SKILL token: `"foo"` → `Some("foo")`, `nil` → `None`.
fn extract_skill_string_token(token: &str) -> Option<String> {
    let s = token.trim();
    if s == "nil" || s.is_empty() {
        return None;
    }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

struct AdeWindowInfo {
    application: String,
    lib: String,
    cell: String,
    view: String,
    editable: bool,
    unsaved_changes: bool,
}

/// Parse an ADE window title: `ADE Assembler Editing: LIB CELL VIEW[*]`
fn parse_ade_title(title: &str) -> Option<AdeWindowInfo> {
    let ade_pos = title.find("ADE ")?;
    let rest = &title[ade_pos + 4..];

    let (app, rest) = if let Some(r) = rest.strip_prefix("Assembler ") {
        ("assembler", r)
    } else {
        let r = rest.strip_prefix("Explorer ")?;
        ("explorer", r)
    };

    let (editable, rest) = if let Some(r) = rest.strip_prefix("Editing: ") {
        (true, r)
    } else {
        let r = rest.strip_prefix("Reading: ")?;
        (false, r)
    };

    let mut parts = rest.split_whitespace();
    let lib = parts.next()?.to_string();
    let cell = parts.next()?.to_string();
    let view_raw = parts.next()?;
    let unsaved_changes = view_raw.ends_with('*');
    let view = view_raw.trim_end_matches('*').to_string();

    Some(AdeWindowInfo {
        application: app.to_string(),
        lib,
        cell,
        view,
        editable,
        unsaved_changes,
    })
}

// ============================================================================
// Result Reading Functions
// ============================================================================

/// Open a history run for programmatic result access.
pub fn open_results(history: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.open_results(history);
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("open results")?;
    Ok(json!({"status": "success", "history": history}))
}

/// Close the currently open results.
pub fn close_results() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.close_results();
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("close results")?;
    Ok(json!({"status": "success"}))
}

/// List all test names that have results in the current history.
pub fn get_result_tests() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_result_tests();
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "get result tests failed: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

/// List all output names available for a given test.
pub fn get_result_outputs(test_name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_result_outputs(test_name);
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "get result outputs failed: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

/// Get the value of a specific output for a specific test and corner.
pub fn get_output_value(name: &str, test_name: &str, corner: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_output_value(name, test_name, corner);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec(&format!("get output '{name}'"))?;
    Ok(json!({
        "status": "success",
        "output_name": name,
        "test_name": test_name,
        "corner": corner,
        "value": r.output_unquoted(),
    }))
}

/// Get output value with results opened first (convenience method).
///
/// This combines open_results and get_output_value into a single SKILL call.
/// Use this when reading output values from a specific history run.
#[allow(dead_code)]
pub fn get_output_value_from_history(
    history: &str,
    name: &str,
    test_name: &str,
    corner: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    // Use the combined method that opens results first
    let skill = client
        .maestro
        .get_output_value_with_open(history, name, test_name, corner);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec(&format!("get output '{name}' from history '{history}'"))?;
    Ok(json!({
        "status": "success",
        "history": history,
        "output_name": name,
        "test_name": test_name,
        "corner": corner,
        "value": r.output_unquoted(),
    }))
}

/// Get the spec pass/fail status for an output.
pub fn get_spec_status(name: &str, test_name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_spec_status(name, test_name);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec(&format!("get spec status '{name}'"))?;
    Ok(json!({
        "status": "success",
        "output_name": name,
        "test_name": test_name,
        "spec_status": r.output_unquoted(),
    }))
}

/// Get simulation messages (errors/warnings) from the last run.
pub fn get_sim_messages(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_sim_messages(session);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("get sim messages")?;
    Ok(json!({"status": "success", "session": session, "messages": r.output_unquoted()}))
}

/// List available history runs for the current Maestro session.
pub fn get_history_list() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_history_list();
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("get history list")?;
    parse_skill_json(&r.output)
}

/// Snapshot run artifacts to a local directory (YAML-filtered).
///
/// Pulls files from a Maestro run directory matching the filter rules.
/// Binary waveforms (*.raw, wavedb/) are always excluded.
///
/// The built-in filter copies:
///   - maestro.sdb, active.state (session setup)
///   - state_from_sdb.xml, state_from_active_state.xml (parsed state)
///   - state_from_skill.txt (SKILL-probed summary)
///   - Per-point: *.log, *.rdb, *.msg.db (run-level logs)
///   - Per-point: netlist/{input.scs, netlist, qpInformation.ils, paramInfo.ils}
///   - Per-point: psf/{spectre.out, logFile, *.dc, *.ac, *.tran, ...}
pub fn snapshot(
    output_dir: &str,
    session: Option<&str>,
    history: Option<&str>,
    filter_path: Option<&str>,
) -> Result<Value> {
    use std::fs;
    use std::path::Path;

    let client = VirtuosoClient::from_env()?;

    // 1. Resolve session and run directory
    let session_info = snapshot_resolve_session(&client, session)?;

    let session_name = session_info
        .get("session")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VirtuosoError::Execution("no session resolved".into()))?;

    let run_dir = session_info
        .get("run_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VirtuosoError::Execution("run_dir not found for session".into()))?;

    // 2. Resolve history (default: newest by mtime sort)
    let history_name = match history {
        Some(h) => h.to_string(),
        None => {
            let skill = client.maestro.get_history_list();
            let r = client.execute_skill(&skill, None)?;
            let histories: Vec<String> = parse_skill_json(&r.output)
                .and_then(|v| {
                    v.as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| e.as_str().map(String::from))
                                .collect()
                        })
                        .ok_or_else(|| VirtuosoError::Execution("expected array".into()))
                })
                .unwrap_or_default();
            histories
                .last()
                .cloned()
                .unwrap_or_else(|| "Interactive.1".to_string())
        }
    };

    // 3. Load filter rules (built-in or custom YAML)
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct FilterRules {
        always: Vec<String>,
        state: Vec<String>,
        skill_summary: Vec<String>,
        run_level: Vec<String>,
        netlist: Vec<String>,
        psf: Vec<String>,
        exclude: Vec<String>,
    }

    impl Default for FilterRules {
        fn default() -> Self {
            Self {
                always: vec!["maestro.sdb".into(), "active.state".into()],
                state: vec![
                    "state_from_sdb.xml".into(),
                    "state_from_active_state.xml".into(),
                ],
                skill_summary: vec!["state_from_skill.txt".into()],
                run_level: vec!["*.log".into(), "*.rdb".into(), "*.msg.db".into()],
                netlist: vec![
                    "input.scs".into(),
                    "netlist".into(),
                    "qpInformation.ils".into(),
                    "paramInfo.ils".into(),
                ],
                psf: vec!["spectre.out".into(), "logFile".into()],
                exclude: vec!["*.raw".into(), "*/wavedb/*".into(), "*/psf/*.raw".into()],
            }
        }
    }

    let rules: FilterRules = if let Some(path) = filter_path {
        let yaml =
            fs::read_to_string(path).map_err(|e| VirtuosoError::Io(std::io::Error::other(e)))?;
        serde_yaml::from_str(&yaml).unwrap_or_else(|e| {
            eprintln!("warning: failed to parse filter YAML: {e}; using defaults");
            FilterRules::default()
        })
    } else {
        FilterRules::default()
    };

    // 4. Pattern matching helper
    fn matches_pattern(filename: &str, patterns: &[String]) -> bool {
        for p in patterns {
            if let Some(suffix) = p.strip_prefix('*') {
                if filename.ends_with(suffix) {
                    return true;
                }
            } else if p.contains('*') {
                // Simple prefix glob
                if let Some(star) = p.find('*') {
                    let prefix = &p[..star];
                    if filename.starts_with(prefix) {
                        return true;
                    }
                }
            } else if filename == p {
                return true;
            }
        }
        false
    }

    fn matches_any(s: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|p| {
            if let Some(suffix) = p.strip_prefix('*') {
                s.ends_with(suffix)
            } else if p.contains('*') {
                if let Some(star) = p.find('*') {
                    let prefix = &p[..star];
                    s.starts_with(prefix)
                } else {
                    false
                }
            } else {
                s == p
            }
        })
    }

    // 5. Collect files from run directory structure
    let run_base = Path::new(run_dir);
    let history_dir = run_base.join(&history_name);

    let mut collected: Vec<(String, String)> = Vec::new(); // (src_path, rel_path)

    // Session-level files
    for pattern in rules
        .always
        .iter()
        .chain(rules.state.iter())
        .chain(rules.skill_summary.iter())
    {
        let src = run_base.join(pattern);
        if src.exists() && !matches_any(pattern, &rules.exclude) {
            collected.push((src.to_string_lossy().to_string(), pattern.clone()));
        }
    }

    // Point-level files
    let pt_dirs: Vec<_> = std::fs::read_dir(&history_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();

    for pt_entry in &pt_dirs {
        let pt_name = pt_entry.file_name().to_string_lossy().into_owned();
        let pt_path = pt_entry.path();

        // Run-level logs: <pt>/run/<run_name>/<tb>/*.log etc.
        let run_dir_inner = pt_path.join("run");
        if run_dir_inner.exists() {
            if let Ok(run_dirs) = std::fs::read_dir(&run_dir_inner) {
                for run_entry in run_dirs.flatten() {
                    let run_name = run_entry.file_name().to_string_lossy().into_owned();
                    let tb_dir = run_entry.path();
                    if tb_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&tb_dir) {
                            for entry in entries.flatten() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                let rel = format!("{}/run/{}/{}", pt_name, run_name, name);
                                if matches_pattern(&name, &rules.run_level)
                                    && !matches_any(&rel, &rules.exclude)
                                {
                                    collected
                                        .push((entry.path().to_string_lossy().to_string(), rel));
                                }
                            }
                        }
                    }
                }
            }
        }

        // Netlist files
        let netlist_dir = pt_path.join("netlist");
        if netlist_dir.exists() {
            for net_pattern in &rules.netlist {
                let src = netlist_dir.join(net_pattern);
                let rel = format!("{}/netlist/{}", pt_name, net_pattern);
                if src.exists() && !matches_any(&rel, &rules.exclude) {
                    collected.push((src.to_string_lossy().to_string(), rel));
                }
            }
        }

        // PSF files
        let psf_dir = pt_path.join("psf");
        if psf_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&psf_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let rel = format!("{}/psf/{}", pt_name, name);
                    // Always include spectre.out and logFile
                    let is_fixed = name == "spectre.out" || name == "logFile";
                    let matches_psf = matches_pattern(
                        &name,
                        &[
                            "*.dc".into(),
                            "*.ac".into(),
                            "*.tran".into(),
                            "*.noise".into(),
                            "*.sp".into(),
                            "*.fb".into(),
                            "*.ft".into(),
                            "*.sw".into(),
                            "*.sh".into(),
                        ],
                    );
                    if (is_fixed || matches_psf) && !matches_any(&rel, &rules.exclude) {
                        collected.push((entry.path().to_string_lossy().to_string(), rel));
                    }
                }
            }
        }
    }

    // 6. Create output directory and copy files
    let output_path = Path::new(output_dir);
    fs::create_dir_all(output_path).map_err(|e| VirtuosoError::Io(std::io::Error::other(e)))?;

    let mut copied_count = 0;
    let mut skipped_count = 0;

    for (src, rel) in &collected {
        let dst = output_path.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::copy(src, &dst) {
            Ok(_) => copied_count += 1,
            Err(_) => skipped_count += 1,
        }
    }

    Ok(json!({
        "status": "success",
        "session": session_name,
        "history": history_name,
        "run_dir": run_dir,
        "output_dir": output_dir,
        "files_copied": copied_count,
        "files_skipped": skipped_count,
    }))
}

/// Resolve session name and run_dir from optional session arg.
fn snapshot_resolve_session(client: &VirtuosoClient, session: Option<&str>) -> Result<Value> {
    let skill = client.maestro.focused_window_skill();
    let r = client.execute_skill(&skill, None)?;

    let tokens = parse_skill_list_top_level(&r.output);
    let dav_session = tokens.get(1).and_then(|t| extract_skill_string_token(t));

    let effective = session.map(String::from).or(dav_session.clone());

    let run_dir = if let Some(ref s) = effective {
        if Some(s.as_str()) != dav_session.as_deref() {
            let skill2 = client.maestro.run_dir_skill(s);
            let r2 = client.execute_skill(&skill2, None)?;
            r2.output_unquoted().to_string()
        } else {
            tokens
                .get(4)
                .and_then(|t| extract_skill_string_token(t))
                .unwrap_or_default()
        }
    } else {
        String::new()
    };

    Ok(json!({
        "session": effective,
        "run_dir": run_dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_5_element_list() {
        let input = r#"("title str" "sess" ("w1" "w2") ("s1") nil)"#;
        let tokens = parse_skill_list_top_level(input);
        assert_eq!(tokens.len(), 5, "{tokens:?}");
        assert_eq!(tokens[0], r#""title str""#);
        assert_eq!(tokens[1], r#""sess""#);
        assert_eq!(tokens[4], "nil");
    }

    #[test]
    fn tokenizer_with_backslash_escape_in_string() {
        // SKILL octal escape \256 for ® char — must not confuse the tokenizer
        let input = r#"("Virtuoso\256 ADE Explorer Editing: LIB CELL V" "fnxSession0")"#;
        let tokens = parse_skill_list_top_level(input);
        assert_eq!(tokens.len(), 2, "{tokens:?}");
        assert_eq!(tokens[1], r#""fnxSession0""#);
    }

    #[test]
    fn tokenizer_empty_list() {
        assert_eq!(parse_skill_list_top_level("nil"), vec![] as Vec<String>);
        assert_eq!(parse_skill_list_top_level("()"), vec![] as Vec<String>);
    }

    #[test]
    fn extract_token_quoted() {
        assert_eq!(
            extract_skill_string_token(r#""fnxSession0""#),
            Some("fnxSession0".to_owned())
        );
    }

    #[test]
    fn extract_token_nil() {
        assert_eq!(extract_skill_string_token("nil"), None);
        assert_eq!(extract_skill_string_token(""), None);
    }
}

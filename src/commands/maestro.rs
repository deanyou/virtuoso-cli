use crate::client::bridge::VirtuosoClient;
use crate::client::skill_runtime::decode_json;
use crate::commands::schematic::parse_skill_json;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::transport::contract::{CommandRequest, DownloadDirRequest, RemoteTransport};
use crate::transport::ssh::shell_quote;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    decode_json(&r, "list Maestro sessions")
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

/// Run simulation with optional analysis configuration and dec injection.
///
/// Strategy: maeSetAnalysis (updates Maestro config) + maeSaveSetup (writes netlist) +
/// sed via SKILL system() (injects dec) + maeRunSimulation (blocks ~3s while Spectre reads).
/// By running sed in the SAME SKILL call before maeRunSimulation returns, dec is guaranteed
/// present when Spectre reads the netlist.
pub fn run_with_analysis(
    session: &str,
    analysis_type: Option<&str>,
    options: Option<&str>,
    dec: Option<&u32>,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    let skill = if let Some(dec_val) = dec {
        let at = analysis_type
            .ok_or_else(|| VirtuosoError::Execution("dec requires --analysis".to_string()))?;
        let alist = match options {
            None => None,
            Some(opts) => {
                let ver = client.version()?;
                if !ver.is_ic25() {
                    eprintln!("warning: --options is only supported on IC25; ignoring on IC23");
                    None
                } else {
                    Some(
                        crate::client::maestro_ops::json_to_skill_alist(opts)
                            .map_err(|e| VirtuosoError::Execution(format!("--options: {e}")))?,
                    )
                }
            }
        };
        client
            .maestro
            .run_with_dec(session, at, alist.as_deref(), *dec_val)
    } else {
        // No dec: existing path — set analysis then run
        if let Some(at) = analysis_type {
            let (alist, version) = match options {
                None => (None, crate::version::VirtuosoVersion::IC23),
                Some(opts) => {
                    let a = crate::client::maestro_ops::json_to_skill_alist(opts)
                        .map_err(|e| VirtuosoError::Execution(format!("--options: {e}")))?;
                    let ver = client.version()?;
                    if !ver.is_ic25() {
                        eprintln!("warning: --options is only supported on IC25; ignoring on IC23");
                        (None, ver)
                    } else {
                        (Some(a), ver)
                    }
                }
            };
            let skill = client
                .maestro
                .set_analysis(session, at, alist.as_deref(), version);
            client
                .execute_skill(&skill, None)?
                .ok_or_exec("set analysis")?;
        }
        client.maestro.run_simulation(session)
    };

    // dec injection uses system(sed) — requires Admin capability + whitelist bypass
    if dec.is_some() {
        client
            .execute_skill_admin(&skill, None)?
            .ok_or_exec("run simulation")?;
    } else {
        client
            .execute_skill(&skill, None)?
            .ok_or_exec("run simulation")?;
    }
    Ok(json!({
        "status": "launched",
        "session": session,
        "dec_injection": if dec.is_some() { "inline_skilled" } else { "none" }
    }))
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

// ============================================================================
// Session-scoped corner netlist export (batch 1)
//
// Generates a unique remote temp dir, asks Virtuoso (via the existing daemon)
// to write the corner netlist into that dir, verifies non-empty output,
// streams the dir back to a local destination, and cleans up the remote dir.
//
// All remote shell values go through `shell_quote` so paths and IDs cannot
// inject shell metacharacters. The remote path uses a fixed controlled
// prefix + UUIDv4 suffix — no user-supplied fragments are interpolated.
// ============================================================================

/// Fixed controlled prefix for corner-netlist remote temp dirs.
/// Public for tests; never derived from user input.
pub(crate) const CORNER_NETLIST_REMOTE_PREFIX: &str = "/tmp/vcli_corner_netlist_";

/// Generate an unpredictable, unique remote temp dir for a corner netlist export.
///
/// Format: `<CORNER_NETLIST_REMOTE_PREFIX><32 hex chars>`. Unpredictable because
/// the suffix is `Uuid::new_v4()` (random from the OS CSPRNG), unique because
/// UUIDv4 collisions have probability ~2^-122 even across millions of calls.
/// No sleep or wall-clock sleeps are used (UUID entropy is sufficient).
pub(crate) fn corner_netlist_remote_path() -> String {
    format!(
        "{}{}",
        CORNER_NETLIST_REMOTE_PREFIX,
        uuid::Uuid::new_v4().simple()
    )
}

/// Build the shell command to create a remote temp dir, quoting the path.
///
/// Used after `corner_netlist_remote_path()` returns. The full path goes
/// through `shell_quote`, so the dir can contain spaces or quotes without
/// breaking out of the shell-quoted argument.
pub(crate) fn corner_netlist_mkdir_command(remote_dir: &str) -> String {
    format!("mkdir -p {}", shell_quote(remote_dir))
}

/// Build the verification shell command. Per spec we use
/// `find <quoted> -mindepth 1 -type f -size +0c -print -quit` — this asks
/// the remote `find(1)` for at least one non-zero-byte regular file
/// anywhere under the export dir. `-print -quit` exits on the first match,
/// so the output is either empty (no valid files) or exactly one line
/// (the first match). Explicitly NOT `ls -la` — `ls` cannot be parsed
/// without fragile `contains` heuristics.
pub(crate) fn corner_netlist_verify_command(remote_dir: &str) -> String {
    format!(
        "find {} -mindepth 1 -type f -size +0c -print -quit",
        shell_quote(remote_dir)
    )
}

/// Build the cleanup shell command, quoting the dir.
pub(crate) fn corner_netlist_cleanup_command(remote_dir: &str) -> String {
    format!("rm -rf {}", shell_quote(remote_dir))
}

/// Return true iff the `find -mindepth 1 -print` output reports at least
/// one entry (i.e. the dir is non-empty).
pub(crate) fn remote_dir_has_entries(find_output: &str) -> bool {
    find_output.lines().any(|l| !l.trim().is_empty())
}

/// Enumerate every file under `root` (relative to root, forward-slashified).
/// Best-effort: a permission error on one entry does not abort the walk.
///
/// Uses `symlink_metadata` so it NEVER follows symlinks (file or
/// directory). Only real (non-symlink) regular files are included, and
/// only real (non-symlink) directories are recursed into. Symlinks and
/// other special file types (FIFO, socket, device, …) are skipped.
fn enumerate_local_files(root: &Path) -> Vec<String> {
    fn walk(p: &Path, base: &Path, out: &mut Vec<String>) {
        // `symlink_metadata` does NOT follow symlinks: for a symlink, the
        // returned `file_type()` reports `is_symlink() == true`. That lets
        // us skip links entirely (never recurse into a symlinked dir,
        // never include a symlinked file's target contents).
        if let Ok(m) = std::fs::symlink_metadata(p) {
            let ft = m.file_type();
            if ft.is_symlink() {
                // Intentionally skip; never follow.
                return;
            }
            if ft.is_dir() {
                let Ok(entries) = std::fs::read_dir(p) else {
                    return;
                };
                for e in entries.flatten() {
                    walk(&e.path(), base, out);
                }
            } else if ft.is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let s = rel.to_string_lossy().replace('\\', "/");
                    if !s.is_empty() {
                        out.push(s);
                    }
                }
            }
            // Other special file types (FIFO, socket, device, …) are
            // intentionally skipped — they are not regular files and
            // we cannot meaningfully enumerate their contents here.
        }
    }

    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Validate the local output directory BEFORE any remote side effect.
///
/// Rules (no remote side effects means none of mkdir, SSH, or SKILL have run yet):
/// - Nonexistent path → OK; `atomic_publish_no_replace` will create it via
///   the renamex_np / renameat2 rename of the sibling staging tree.
/// - Existing directory (empty OR non-empty) → REJECTED as `Conflict`.
///   The publish step uses an atomic exclusive rename, which can never
///   replace an existing destination: any pre-existing user entry would
///   be preserved byte-for-byte by EEXIST / ENOTEMPTY, and we surface
///   that refusal up front so the operator can decide whether to remove
///   it first.
/// - Existing regular file (or any non-directory) → REJECTED.
///
/// Pure helper: only inspects local filesystem metadata. No SSH, no
/// SKILL, no remote side effects.
pub(crate) fn validate_output_dir_for_download(output_dir: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(output_dir);
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Nonexistent — atomic_publish_no_replace will materialize
            // it via the rename. Safe.
            Ok(path.to_path_buf())
        }
        Err(e) => Err(VirtuosoError::Config(format!(
            "cannot stat output_dir '{output_dir}': {e}"
        ))),
        Ok(md) => {
            if !md.is_dir() {
                return Err(VirtuosoError::Conflict(format!(
                    "output_dir '{output_dir}' exists but is not a directory; \
                     refusing to use it as a download destination"
                )));
            }
            // Path exists and is a directory. Whether empty or not,
            // the atomic rename MUST refuse it (any pre-existing entry
            // is preserved byte-for-byte via EEXIST / ENOTEMPTY).
            Err(VirtuosoError::Conflict(format!(
                "output_dir '{output_dir}' already exists; \
                 refusing to overwrite (contents preserved byte-for-byte). \
                 Pass a nonexistent path instead; atomic_publish_no_replace \
                 will create it via renamex_np / renameat2."
            )))
        }
    }
}

// ============================================================================
// Local staging directory (between download and atomic publish)
//
// `SSHRunner::download_dir` is intentionally NEVER called with the user-
// supplied `output_dir`. Instead, this invocation owns a uniquely-named,
// fixed-prefix local staging directory placed as a SIBLING of `output_dir`
// in the same parent directory, so the publish step is a single-filesystem
// atomic rename (renamex_np RENAME_EXCL on macOS, renameat2 RENAME_NOREPLACE
// on Linux) — destination never exists, so EEXIST / ENOTEMPTY mean the
// operator already has something there and we surface Conflict instead of
// silently clobbering.
//
// Flow:
//   1. Allocate a sibling staging path under `output_dir`'s parent
//      (fixed prefix `.vcli_corner_netlist_staging_` + 32 hex UUIDv4).
//   2. Install a narrowly-scoped RAII guard that removes the staging dir
//      on drop, but ONLY if its leaf name still carries the verified
//      prefix + 32 lowercase hex chars AND its parent matches the parent
//      it was created for. (Remote directory is preserved on any failure;
//      staging is not.)
//   3. `ssh.download_dir(remote, staging)` writes only to staging.
//   4. `validate_staging_recursive(staging)` rejects symlinks and other
//      non-regular/non-directory entries, never following links.
//   5. `atomic_publish_no_replace(staging, output_dir, remote_dir)` uses
//      renamex_np / renameat2 to rename the entire staging tree to
//      `output_dir` in a single atomic syscall. EEXIST and ENOTEMPTY map
//      to Conflict; any other failure mentions `remote_dir` and is
//      surfaced unchanged.
//   6. On success, the caller explicitly drops the staging guard so
//      staging is removed BEFORE the existing remote `rm -rf` cleanup
//      runs (cleanup behavior is preserved).
// ============================================================================

/// Fixed prefix for local staging directories created by this command.
///
/// Includes a leading dot so the staging dir is hidden on POSIX file
/// listings and visually distinguishable from the operator-supplied
/// `output_dir` it shadows. Public for tests; never derived from user
/// input. The guard verifies both this exact prefix AND that the
/// suffix is 32 lowercase hex chars (UUIDv4) AND that the path lives
/// in the same parent it was created for, before removing anything on
/// drop.
pub(crate) const CORNER_NETLIST_STAGING_PREFIX: &str = ".vcli_corner_netlist_staging_";

/// Generate an unpredictable, unique local staging path as a sibling
/// of the operator-supplied `output_dir`.
///
/// Format: `<output_dir_parent>/.vcli_corner_netlist_staging_<32 hex chars>`.
/// The UUIDv4 suffix is random from the OS CSPRNG — collision
/// probability ~2^-122 — and the sibling placement keeps the staging
/// rename a single-filesystem renamex_np / renameat2 call so the
/// publish step is atomic (no partial destination state).
pub(crate) fn corner_netlist_staging_path(output_dir: &Path) -> PathBuf {
    let parent = output_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    parent.join(format!(
        "{}{}",
        CORNER_NETLIST_STAGING_PREFIX,
        uuid::Uuid::new_v4().simple()
    ))
}

/// Narrowly-scoped RAII guard for the local staging directory.
///
/// On `Drop`, it removes its own path — but ONLY when ALL of the
/// following hold (defence-in-depth: prefix + exact length + lowercase
/// hex suffix + same parent directory it was created for):
///   - the caller has not called `disarm()`,
///   - the leaf name has length exactly
///     `CORNER_NETLIST_STAGING_PREFIX.len() + 32`,
///   - the leaf name starts with `CORNER_NETLIST_STAGING_PREFIX`,
///   - the suffix (after the prefix) is 32 lowercase hex chars
///     (`[0-9a-f]`),
///   - the path's parent directory equals the parent it was created
///     for.
///
/// These checks mean the guard can never accidentally remove a
/// directory it does not own (e.g. if a future refactor passes a
/// different path into `StagingGuard::new`, or if the staging path
/// itself was tampered with on disk). Removal errors are silently
/// ignored (best-effort) because the guard is a defensive cleanup, not
/// a correctness primitive — publication correctness comes from
/// `atomic_publish_no_replace`.
pub(crate) struct StagingGuard {
    path: PathBuf,
    expected_parent: PathBuf,
    disarmed: bool,
}

impl StagingGuard {
    pub fn new(path: PathBuf, expected_parent: PathBuf) -> Self {
        Self {
            path,
            expected_parent,
            disarmed: false,
        }
    }

    /// Opt out of automatic cleanup. Used by tests that want to inspect
    /// the staging dir after the publish step.
    #[allow(dead_code)]
    pub fn disarm(mut self) {
        self.disarmed = true;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // Same-parent check first: refuse anything that no longer
        // lives under the parent we were created for.
        match self.path.parent() {
            Some(p) if p == self.expected_parent.as_path() => {}
            _ => return,
        }
        let name = match self.path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => return,
        };
        // Exact-length check: prefix + exactly 32 hex chars.
        let expected_len = CORNER_NETLIST_STAGING_PREFIX.len() + 32;
        if name.len() != expected_len {
            return;
        }
        // Prefix check.
        if !name.starts_with(CORNER_NETLIST_STAGING_PREFIX) {
            return;
        }
        // Suffix check: 32 lowercase hex chars (`[0-9a-f]` only —
        // uppercase `A-F` is rejected so the guard never confuses a
        // tampered or non-UUID leaf with our own).
        let suffix = &name[CORNER_NETLIST_STAGING_PREFIX.len()..];
        if !suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Validate the local staging directory recursively.
///
/// Rules:
/// - Use `symlink_metadata` for every entry; never follow links.
/// - Reject any symlink (file or directory).
/// - Reject any entry that is neither a regular file nor a directory
///   (FIFO, socket, device, …).
/// - Require at least one regular file with `len() > 0` anywhere in the
///   tree — mirrors the remote `find -type f -size +0c` contract.
///
/// On failure, the error is annotated with the staging path so the user
/// can inspect what was rejected. The remote directory is preserved
/// upstream.
pub(crate) fn validate_staging_recursive(root: &Path) -> Result<()> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut has_nonempty_regular = false;
    while let Some(p) = stack.pop() {
        let md = std::fs::symlink_metadata(&p).map_err(|e| {
            VirtuosoError::Ssh(format!(
                "failed to stat staging path '{}': {e}",
                p.display()
            ))
        })?;
        let ft = md.file_type();
        if ft.is_symlink() {
            return Err(VirtuosoError::Ssh(format!(
                "staging path '{}' is a symlink; refusing to follow (never follow links)",
                p.display()
            )));
        }
        if ft.is_dir() {
            let entries = std::fs::read_dir(&p).map_err(|e| {
                VirtuosoError::Ssh(format!(
                    "failed to read staging directory '{}': {e}",
                    p.display()
                ))
            })?;
            for e in entries {
                let e = e.map_err(|err| {
                    VirtuosoError::Ssh(format!(
                        "failed to iterate staging directory '{}': {err}",
                        p.display()
                    ))
                })?;
                stack.push(e.path());
            }
        } else if ft.is_file() {
            if md.len() > 0 {
                has_nonempty_regular = true;
            }
        } else {
            return Err(VirtuosoError::Ssh(format!(
                "staging entry '{}' is neither a regular file nor a directory (refusing)",
                p.display()
            )));
        }
    }
    if !has_nonempty_regular {
        return Err(VirtuosoError::Ssh(format!(
            "staging directory '{}' contains no non-empty regular files (mirrors remote -size +0c)",
            root.display()
        )));
    }
    Ok(())
}

/// Atomically publish the staging tree as the destination directory.
///
/// This is a SINGLE atomic filesystem rename — staging directory
/// becomes `dst` (the operator's `output_dir`) in one syscall. There
/// is no per-file copy step: bytes never appear in `dst` unless the
/// rename succeeded in full, so the operator's existing `output_dir`
/// (if any) is preserved byte-for-byte.
///
/// Pre-conditions (all enforced by the caller, NOT this helper):
///   - `src` (staging) must be a non-symlink directory in the same
///     parent directory as `dst`, so the rename is single-filesystem.
///   - `dst` MUST NOT already exist (caller removes the
///     pre-existing-empty-dir allowance — if it did exist, the rename
///     would either fail or clobber).
///   - The parent directory of `dst` must already exist; if not, the
///     caller creates it BEFORE calling this helper.
///
/// Platform support:
///   - macOS → `renamex_np(src, dst, RENAME_EXCL)`. EEXIST is Conflict.
///   - Linux → `renameat2(AT_FDCWD, src, AT_FDCWD, dst, RENAME_NOREPLACE)`.
///     EEXIST (file already exists) and ENOTEMPTY (directory already
///     exists and is non-empty) are both Conflict — the operator's
///     existing data is preserved either way.
///   - Any other platform → `VirtuosoError::Config` (unsupported).
///
/// There is NO `std::fs::rename` fallback: if the platform lacks
/// atomic-exclusive rename, we refuse to publish rather than risk a
/// non-atomic clobber.
///
/// ALL errors mention `remote_dir` so the operator can correlate a
/// publication failure with the preserved remote artifacts.
/// Build the `CString` path arguments for the exclusive-rename syscalls.
///
/// Unix-only, and only ever reached from the macOS/Linux branches of
/// [`atomic_publish_no_replace`]: `OsStrExt::as_bytes` is the one way to
/// hand a path to a C syscall without letting Rust perform a lossy UTF-8
/// conversion or pass an interior NUL byte through.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn publish_path_cstrings(
    src: &Path,
    dst: &Path,
    remote_dir: &str,
) -> Result<(std::ffi::CString, std::ffi::CString)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // CString from raw OsStrExt bytes. We never let Rust do a
    // lossy UTF-8 conversion or insert a NUL byte from a path that
    // happens to contain one — a non-UTF-8 path is a Config error.
    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|e| {
        VirtuosoError::Config(format!(
            "atomic publish unsupported: staging path contains an interior NUL byte: {e}; \
             netlist artifacts preserved at remote_dir={remote_dir}"
        ))
    })?;
    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|e| {
        VirtuosoError::Config(format!(
            "atomic publish unsupported: destination path contains an interior NUL byte: {e}; \
             netlist artifacts preserved at remote_dir={remote_dir}"
        ))
    })?;
    Ok((src_c, dst_c))
}

// On platforms with no exclusive-rename syscall, `src`/`dst` are never read
// — the function only reports "unsupported". Silence that one narrow case
// instead of weakening the signature for platforms that do use them.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(unused_variables)
)]
pub(crate) fn atomic_publish_no_replace(src: &Path, dst: &Path, remote_dir: &str) -> Result<()> {
    // Only the platforms whose exclusive-rename syscall we call below need
    // the C string arguments. Unsupported platforms take neither binding
    // and fall through to the explicit Config error at the end.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::io::Error as IoError;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let (src_c, dst_c) = publish_path_cstrings(src, dst, remote_dir)?;

    #[cfg(target_os = "macos")]
    {
        // RENAME_EXCL on macOS: 0x00000004. Returns 0 on success, -1
        // on failure with errno set. EEXIST = destination already
        // exists (file or non-empty directory); we map that to Conflict.
        const RENAME_EXCL: libc::c_uint = 0x00000004;
        let res = unsafe { libc::renamex_np(src_c.as_ptr(), dst_c.as_ptr(), RENAME_EXCL) };
        if res == 0 {
            return Ok(());
        }
        let err = IoError::last_os_error();
        let errno = err.raw_os_error().unwrap_or(0);
        // libc::EEXIST == 17 on Darwin; we still match the symbolic
        // constant rather than the literal so cross-platform review is
        // safer.
        if errno == libc::EEXIST {
            return Err(VirtuosoError::Conflict(format!(
                "atomic publish refused: destination '{}' already exists; \
                 refusing to overwrite (preserved byte-for-byte); \
                 netlist artifacts preserved at remote_dir={remote_dir}",
                dst.display()
            )));
        }
        Err(VirtuosoError::Ssh(format!(
            "atomic publish failed (renamex_np RENAME_EXCL) for staging '{}' → destination '{}': \
             errno={} ({}); \
             netlist artifacts preserved at remote_dir={remote_dir}",
            src.display(),
            dst.display(),
            errno,
            err
        )))
    }

    #[cfg(target_os = "linux")]
    {
        // RENAME_NOREPLACE on Linux: 1. renameat2 is a per-syscall
        // errno return; EEXIST means a non-directory file exists at
        // dst, ENOTEMPTY means dst is a non-empty directory. Both
        // mean "the operator already has something there" → Conflict.
        const RENAME_NOREPLACE: libc::c_uint = 1;
        let res = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                src_c.as_ptr(),
                libc::AT_FDCWD,
                dst_c.as_ptr(),
                RENAME_NOREPLACE,
            )
        };
        if res == 0 {
            return Ok(());
        }
        let err = IoError::last_os_error();
        let errno = err.raw_os_error().unwrap_or(0);
        if errno == libc::EEXIST {
            return Err(VirtuosoError::Conflict(format!(
                "atomic publish refused: destination '{}' already exists as a file; \
                 refusing to overwrite (preserved byte-for-byte); \
                 netlist artifacts preserved at remote_dir={remote_dir}",
                dst.display()
            )));
        }
        if errno == libc::ENOTEMPTY {
            return Err(VirtuosoError::Conflict(format!(
                "atomic publish refused: destination '{}' is a non-empty directory; \
                 refusing to overwrite (preserved byte-for-byte); \
                 netlist artifacts preserved at remote_dir={remote_dir}",
                dst.display()
            )));
        }
        Err(VirtuosoError::Ssh(format!(
            "atomic publish failed (renameat2 RENAME_NOREPLACE) for staging '{}' → destination '{}': \
             errno={} ({}); \
             netlist artifacts preserved at remote_dir={remote_dir}",
            src.display(),
            dst.display(),
            errno,
            err
        )))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(VirtuosoError::Config(format!(
            "atomic publish unsupported on this platform (no renamex_np / renameat2); \
             refusing to publish non-atomically; \
             netlist artifacts preserved at remote_dir={remote_dir}"
        )))
    }
}

/// Export the netlist for a single (session, test, corner) tuple.
///
/// Flow:
///   1. Validate that VB_REMOTE_HOST is configured (set in profile / .env).
///   2. Validate the local `output_dir` is safe to use — nonexistent,
///      an existing empty directory, or absent are all OK; a non-empty
///      directory or a non-directory path is rejected so existing
///      contents are preserved byte-for-byte. (Done BEFORE any remote
///      side effect.)
///   3. Initialize the VirtuosoClient early so a connection/timeout
///      error surfaces with its original variant (not flattened into
///      `Execution`). Then build the remote transport from the same Config.
///   4. Generate an unpredictable, fixed-prefix remote temp dir.
///   5. `mkdir -p` on the remote via SSH (quoted).
///   6. Execute the `maeCreateNetlistForCorner` SKILL builder against the
///      running Virtuoso daemon; require a non-nil SKILL result.
///   7. `find <quoted> -mindepth 1 -type f -size +0c -print -quit` on
///      the remote to confirm the dir has at least one non-empty regular
///      file. Otherwise → fail and **preserve the remote dir** so the
///      user can inspect what (if anything) Virtuoso wrote.
///   8. Allocate a unique local staging dir as a SIBLING of `output_dir`
///      (same parent dir, fixed prefix `.vcli_corner_netlist_staging_`
///      + UUIDv4); install an RAII guard that removes ONLY that staging
///        dir, and only when its leaf name still matches the verified prefix
///        plus 32 lowercase hex chars and its parent matches the parent it
///        was created for.
///   9. Stream the remote dir to the staging dir via `download_dir`.
///      The user `output_dir` is NEVER touched here.
///  10. Validate the staging dir recursively with `symlink_metadata` —
///      reject symlinks, FIFOs, sockets, etc.; require at least one
///      non-empty regular file. On failure, staging is removed by the
///      guard and the remote dir is preserved.
///  11. Atomically publish the staging tree as `output_dir` via
///      `atomic_publish_no_replace` — a single renamex_np RENAME_EXCL
///      (macOS) or renameat2 RENAME_NOREPLACE (Linux) syscall. The
///      destination MUST NOT exist; if it does, EEXIST/ENOTEMPTY
///      surfaces as `Conflict` and the operator's existing data is
///      preserved byte-for-byte. ALL errors mention `remote_dir`.
///  12. Explicitly disarm the staging guard — staging no longer needs
///      to exist (it has been renamed to `output_dir`).
///  13. Enumerate the published `output_dir` for the JSON reply.
///  14. Only after successful local publish/staging drop, attempt the
///      existing `rm -rf` cleanup of the remote dir. A cleanup failure
///      does NOT fake full success: the JSON reply includes
///      `remote_cleaned=false` and a `warning`.
///
/// On any failure in steps 4–13, the remote dir is preserved and the
/// error message reports `remote_dir=…` so the user can debug the export.
pub fn create_corner_netlist(
    session: &str,
    test: &str,
    corner: &str,
    output_dir: &str,
) -> Result<Value> {
    // 1. Validate config / remote host (no remote side effect yet).
    //    Use `?` directly so the original `VirtuosoError` variant from
    //    `Config::from_env` is preserved (no flattening into `Config`).
    let cfg = Config::from_env()?;
    if cfg
        .remote_host
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is required for `vcli maestro export-netlist`. \
             Set it in your profile or .env (e.g. VB_REMOTE_HOST=eda-server)."
                .into(),
        ));
    }

    // 2. Validate output_dir BEFORE any remote side effect (best-effort
    //    preflight). The authoritative guard is publish-time
    //    `atomic_publish_no_replace`, which rejects an existing
    //    destination as Conflict and preserves it byte-for-byte.
    let local_path = validate_output_dir_for_download(output_dir)?;

    // 3. Initialize client early so connection/timeout/auth errors
    //    surface with their original variant (not flattened into
    //    `Execution`). This is still a pure local step — no remote
    //    side effects until step 5.
    let client = VirtuosoClient::from_env()?;

    // 4. Build the remote transport from the same Config
    let ssh: Arc<dyn RemoteTransport> = crate::transport::backend::open_transport(&cfg)?;

    // 5. Generate a unique remote temp dir (fixed prefix + UUIDv4 suffix)
    let remote_dir = corner_netlist_remote_path();
    tracing::info!(
        "create_corner_netlist: session={session} test={test} corner={corner} remote_dir={remote_dir}"
    );

    // 6. mkdir on remote (quoted, never fails because of special chars in path)
    let mkdir_q = corner_netlist_mkdir_command(&remote_dir);
    let mkdir_r = ssh
        .run_command(&CommandRequest::untimed(&mkdir_q))
        .map_err(|e| {
            // Prefix with the structured code so a consumer can classify the
            // failure without substring-matching the message.
            VirtuosoError::Ssh(format!(
                "{}: {e}; netlist artifacts preserved at remote_dir={remote_dir}",
                e.code()
            ))
        })?;
    if !mkdir_r.success {
        return Err(VirtuosoError::Ssh(format!(
            "mkdir remote dir failed: stderr={}; remote_dir={remote_dir}",
            mkdir_r.stderr
        )));
    }

    // 7. Execute the SKILL builder via the existing daemon connection.
    //    Preserve variant/exit-code semantics: Connection/Timeout/Ssh/etc
    //    errors are NOT flattened into Execution. Only SKILL nil (via
    //    ok_or_exec) is mapped to Execution. `with_remote_dir_context`
    //    converts bare `Timeout(secs)` to `TimeoutWithContext(secs,
    //    remote_dir)` so timeout semantics are preserved.
    let skill = client
        .maestro
        .create_netlist_for_corner(test, corner, &remote_dir, session);
    let exec_result = client
        .execute_skill(&skill, None)
        .map_err(|e| with_remote_dir_context(e, &remote_dir))?;
    if let Err(e) = exec_result.ok_or_exec("create netlist for corner") {
        return Err(with_remote_dir_context(e, &remote_dir));
    }

    // 8. Verify remote dir has at least one non-empty regular file:
    //    `find <quoted> -mindepth 1 -type f -size +0c -print -quit`
    //    (NOT `ls -la`.) Empty → preserve the remote dir for forensics.
    let verify_q = corner_netlist_verify_command(&remote_dir);
    let verify_r = ssh
        .run_command(&CommandRequest::untimed(&verify_q))
        .map_err(|e| {
            VirtuosoError::Ssh(format!(
                "{}: {e}; netlist artifacts preserved at remote_dir={remote_dir}",
                e.code()
            ))
        })?;
    if !verify_r.success {
        return Err(VirtuosoError::Ssh(format!(
            "verify remote dir failed: stderr={}; remote_dir={remote_dir}",
            verify_r.stderr
        )));
    }
    if !remote_dir_has_entries(&verify_r.stdout) {
        // No non-empty regular files → preserve the remote dir so the
        // user can decide what to do.
        return Err(VirtuosoError::Execution(format!(
            "remote netlist dir {remote_dir} contains no non-empty regular files \
             (no files produced for corner '{corner}'); preserved for forensics"
        )));
    }

    // 9. Allocate a unique local staging directory owned by THIS
    //    invocation, as a SIBLING of `output_dir` (same parent dir) so
    //    the publish step is a single-filesystem atomic rename.
    //    `download_dir` is NEVER called with the user `output_dir`; the
    //    staging dir is the only place the network pipeline writes.
    let staging_path = corner_netlist_staging_path(&local_path);
    // Pass the explicit sibling-staging parent to the guard so Drop's
    // same-parent check cannot be subverted by a future refactor that
    // hands the guard a path whose parent was derived somewhere else.
    let staging_expected_parent = staging_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let staging_guard = StagingGuard::new(staging_path.clone(), staging_expected_parent);

    // 10. Stream the remote dir into the staging path (NOT output_dir).
    //     On failure the RAII guard removes staging; the remote dir is
    //     preserved upstream.
    if let Err(e) = ssh.download_dir(&DownloadDirRequest::untimed(&remote_dir, &staging_path)) {
        return Err(VirtuosoError::Ssh(format!(
            "{}: {e}; netlist artifacts preserved at remote_dir={remote_dir}",
            e.code()
        )));
    }

    // 11. Validate the staging tree. `validate_staging_recursive` uses
    //     `symlink_metadata` so it never follows links and rejects
    //     symlinks, FIFOs, sockets, devices, etc. It also requires at
    //     least one non-empty regular file.
    if let Err(e) = validate_staging_recursive(&staging_path) {
        return Err(VirtuosoError::Ssh(format!(
            "{e}; netlist artifacts preserved at remote_dir={remote_dir}"
        )));
    }

    // 12. Atomically publish the staging tree as `output_dir` in a
    //     single renamex_np RENAME_EXCL / renameat2 RENAME_NOREPLACE
    //     syscall. Destination MUST NOT already exist (caller enforced
    //     via `validate_output_dir_for_download`); if it does, EEXIST
    //     or ENOTEMPTY is mapped to Conflict and the operator's existing
    //     data is preserved byte-for-byte. ALL errors mention
    //     `remote_dir`. NO `std::fs::rename` fallback — non-atomic
    //     rename would risk partial-publish visibility.
    atomic_publish_no_replace(&staging_path, &local_path, &remote_dir)?;

    // 13. The atomic rename succeeded — staging has been consumed and
    //     is now `output_dir`. Explicitly disarm the guard so it does
    //     NOT try to `remove_dir_all` the freshly-published output.
    drop(staging_guard);

    // 14. Successful publish — enumerate files for the JSON reply.
    let files = enumerate_local_files(&local_path);

    // 15. Remote cleanup — only after successful local publish/staging
    //     drop. Existing contract: never pretend full success if the
    //     remote cleanup itself failed.
    let cleanup_q = corner_netlist_cleanup_command(&remote_dir);
    let cleanup_outcome = match ssh.run_command(&CommandRequest::untimed(&cleanup_q)) {
        Ok(r) if r.success => (true, None),
        Ok(r) => (
            false,
            Some(format!(
                "remote temp dir not cleaned (rc={}, stderr={})",
                r.exit_status, r.stderr
            )),
        ),
        Err(e) => (
            false,
            Some(format!("remote temp dir not cleaned: {}: {e}", e.code())),
        ),
    };

    if cleanup_outcome.0 {
        Ok(json!({
            "status": "success",
            "session": session,
            "test": test,
            "corner": corner,
            "output_dir": output_dir,
            "remote_dir": remote_dir,
            "files": files,
            "remote_cleaned": true,
        }))
    } else {
        Ok(json!({
            "status": "success",
            "warning": cleanup_outcome.1.unwrap_or_default(),
            "session": session,
            "test": test,
            "corner": corner,
            "output_dir": output_dir,
            "remote_dir": remote_dir,
            "files": files,
            "remote_cleaned": false,
        }))
    }
}

/// Attach `remote_dir` context to an error while preserving the original
/// `VirtuosoError` variant (and therefore its exit-code semantics).
///
/// - `Connection`, `Ssh`, `Execution` carry a string message and get the
///   context appended.
/// - `Timeout(u64)` is *converted* into `TimeoutWithContext(seconds, remote_dir)`
///   so the timeout surfaces the preserved remote dir while keeping the
///   Timeout-family exit code / error_type / retryable semantics.
/// - An existing `TimeoutWithContext(seconds, ctx)` only gets the new
///   `remote_dir` appended when the context does NOT already mention it;
///   duplicates are never introduced.
/// - All other variants are passed through unchanged. Timeouts are NEVER
///   flattened into `Execution` — that would change the exit code and
///   classification, and would hide a transport-layer failure.
fn with_remote_dir_context(err: VirtuosoError, remote_dir: &str) -> VirtuosoError {
    let ctx = format!("; netlist artifacts preserved at remote_dir={remote_dir}");
    let remote_dir_marker = format!("remote_dir={remote_dir}");
    match err {
        VirtuosoError::Connection(m) => VirtuosoError::Connection(format!("{m}{ctx}")),
        VirtuosoError::Ssh(m) => VirtuosoError::Ssh(format!("{m}{ctx}")),
        VirtuosoError::Execution(m) => VirtuosoError::Execution(format!("{m}{ctx}")),
        // Convert bare Timeout → TimeoutWithContext so the user sees
        // *which* remote dir was preserved when the timeout fired.
        VirtuosoError::Timeout(secs) => {
            VirtuosoError::TimeoutWithContext(secs, remote_dir.to_string())
        }
        // If the context already mentions this remote_dir, leave it alone.
        // Otherwise append the standard suffix exactly once.
        VirtuosoError::TimeoutWithContext(secs, m) => {
            if m.contains(&remote_dir_marker) {
                VirtuosoError::TimeoutWithContext(secs, m)
            } else {
                VirtuosoError::TimeoutWithContext(secs, format!("{m}{ctx}"))
            }
        }
        // Pass-through: Config(_), NotFound(_), Conflict(_), Auth(_),
        // Io(_), Json(_) — keep original variant.
        other => other,
    }
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

    // ── create_corner_netlist helpers ──────────────────────────────────
    //
    // These tests check the public behavior of the corner-netlist export
    // command's helpers WITHOUT touching the network. They cover:
    //   - remote path has a fixed controlled prefix + hex-only suffix
    //   - remote path is unpredictable and unique across many calls
    //   - shell commands shell-quote any user-controlled remote dir value
    //   - verification uses `find -mindepth 1 -print` (NOT `ls -la`)
    //   - cleanup strategy: the helpers are independent, so failure paths
    //     keep the remote dir (we test that cleanup_command does not depend
    //     on success of any other step).

    #[test]
    fn corner_remote_path_starts_with_fixed_prefix() {
        let p = corner_netlist_remote_path();
        assert!(
            p.starts_with(CORNER_NETLIST_REMOTE_PREFIX),
            "remote path '{p}' must start with '{CORNER_NETLIST_REMOTE_PREFIX}'"
        );
    }

    #[test]
    fn corner_remote_path_has_no_user_fragments() {
        // Suffix is UUIDv4 hex — never derived from caller input.
        for _ in 0..16 {
            let p = corner_netlist_remote_path();
            assert!(!p.contains(".."), "no parent traversal: {p}");
            assert!(!p.contains(';'), "no shell separator: {p}");
            assert!(!p.contains('|'), "no pipe: {p}");
            assert!(!p.contains(' '), "no space: {p}");
            assert!(!p.contains('$'), "no shell variable: {p}");
        }
    }

    #[test]
    fn corner_remote_path_suffix_is_32_lowercase_hex() {
        let p = corner_netlist_remote_path();
        let suffix = &p[CORNER_NETLIST_REMOTE_PREFIX.len()..];
        assert_eq!(
            suffix.len(),
            32,
            "Uuid::new_v4().simple() is exactly 32 chars"
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix not hex: {suffix}"
        );
    }

    #[test]
    fn corner_remote_paths_are_unique_across_many_calls() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..128 {
            let p = corner_netlist_remote_path();
            assert!(seen.insert(p.clone()), "duplicate remote path '{p}'");
        }
        assert_eq!(seen.len(), 128);
    }

    #[test]
    fn corner_mkdir_command_quotes_remote_dir_with_shell_metachars() {
        // A user name / test / corner with shell metachars must still produce
        // a quoted, injectable-safe mkdir command. The path itself is not
        // derived from user input here, but the helper test ensures the
        // quoting is engaged even when the value contains `;`, spaces, and quotes.
        let bad = "/tmp/would rm -rf /; touch pwned 'owned' \"q\"";
        let cmd = corner_netlist_mkdir_command(bad);
        assert!(cmd.starts_with("mkdir -p "), "{cmd}");
        let q = shell_quote(bad);
        // q itself must contain single quotes
        assert!(q.contains('\''), "shell_quote must produce quotes: {q}");
        // The cmd must embed the quoted form (so the raw `bad` does NOT appear)
        assert!(cmd.contains(&q), "command should embed quoted form: {cmd}");
        // Defensive: ensure the unquoted version of the user value is gone.
        assert!(
            !cmd.contains(bad),
            "unquoted user value leaked into command: {cmd}"
        );
    }

    #[test]
    fn corner_verify_command_uses_find_mindepth_not_ls() {
        let cmd = corner_netlist_verify_command("/tmp/x");
        assert!(cmd.starts_with("find "), "{cmd}");
        // Per spec:
        //   find <quoted> -mindepth 1 -type f -size +0c -print -quit
        assert!(cmd.contains(" -mindepth 1"), "{cmd}");
        assert!(cmd.contains(" -type f"), "{cmd}");
        assert!(cmd.contains(" -size +0c"), "{cmd}");
        assert!(cmd.contains(" -print"), "{cmd}");
        assert!(cmd.contains(" -quit"), "{cmd}");
        // MUST NOT use `ls -la` — the spec explicitly forbids it.
        assert!(!cmd.contains("ls "), "must not use ls: {cmd}");
        assert!(
            !cmd.contains("ls -la"),
            "must not use ls -la per spec: {cmd}"
        );
        assert!(!cmd.contains("-la"), "must not use any -la flag: {cmd}");
        // Quote the remote dir
        assert!(cmd.contains(&shell_quote("/tmp/x")), "{cmd}");
        // Quoted remote dir must appear before any flag
        let q = shell_quote("/tmp/x");
        let pos_q = cmd.find(&q).expect("quoted dir in cmd");
        let pos_mindepth = cmd.find(" -mindepth ").expect("-mindepth flag");
        assert!(pos_q < pos_mindepth, "quoted dir must precede flags: {cmd}");
    }

    #[test]
    fn corner_verify_command_quotes_remote_dir_with_metachars() {
        // A user-controlled remote dir (with shell metachars) must be
        // safely shell-quoted before find touches it.
        let path = "/tmp/x; rm -rf / 'owned' \"q\"";
        let quoted = shell_quote(path);
        let cmd = corner_netlist_verify_command(path);
        // Exact-form contract: nothing else can be appended to the find argument.
        // This alone proves the builder uses the `shell_quote` helper; any further
        // assertion about its specific framing must be derived from the helper
        // implementation in `src/transport/ssh.rs` and its existing tests there,
        // not from a guessed quote style.
        assert_eq!(
            cmd,
            format!("find {quoted} -mindepth 1 -type f -size +0c -print -quit")
        );
    }

    #[test]
    fn corner_cleanup_command_quotes_remote_dir() {
        let path = "/tmp/x; rm -rf /";
        let quoted = shell_quote(path);
        let cmd = corner_netlist_cleanup_command(path);
        // Exact-form contract. This alone proves the builder uses the
        // `shell_quote` helper; any further assertion about its specific
        // framing must be derived from the helper implementation in
        // `src/transport/ssh.rs` and its existing tests there, not from
        // a guessed quote style.
        assert_eq!(cmd, format!("rm -rf {quoted}"));
    }

    #[test]
    fn find_output_empty_when_no_entries() {
        assert!(!remote_dir_has_entries(""));
        assert!(!remote_dir_has_entries("\n"));
        assert!(!remote_dir_has_entries("   \n   \n"));
        // `find` on an empty dir still prints the root itself by default;
        // `-mindepth 1` ensures it prints nothing when nothing is present.
        assert!(!remote_dir_has_entries("\n\n"));
    }

    #[test]
    fn find_output_nonempty_with_at_least_one_entry() {
        assert!(remote_dir_has_entries("a\nb\n"));
        assert!(remote_dir_has_entries("netlist/file.scs\n"));
        assert!(remote_dir_has_entries("   \nnetlist/file.scs\n"));
    }

    #[test]
    fn cleanup_strategy_independent_of_other_helpers() {
        // The cleanup helper must produce a self-contained, runnable command
        // regardless of the state of mkdir/verify commands. Failure paths in
        // the export flow keep the remote dir alive; we verify that by
        // confirming the helpers are pure (don't share mutable state).
        let dir1 = corner_netlist_remote_path();
        let dir2 = corner_netlist_remote_path();
        assert_ne!(dir1, dir2);
        let c1 = corner_netlist_cleanup_command(&dir1);
        let c2 = corner_netlist_cleanup_command(&dir2);
        assert!(c1.contains(&dir1) || c1.contains(&shell_quote(&dir1)));
        assert!(c2.contains(&dir2) || c2.contains(&shell_quote(&dir2)));
        // The two cleanup commands must be independent (c1 does NOT mention c2's dir).
        assert!(
            !c1.contains(&dir2) && !c1.contains(&shell_quote(&dir2)),
            "cleanup cmd for dir1 must not target dir2: {c1}"
        );
    }

    #[test]
    fn enumerate_local_files_returns_relative_paths() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("top.txt"), b"x").unwrap();
        fs::write(root.join("sub/inner.scs"), b"y").unwrap();
        let entries = enumerate_local_files(root);
        assert!(entries.iter().any(|e| e == "top.txt"), "{entries:?}");
        assert!(entries.iter().any(|e| e == "sub/inner.scs"), "{entries:?}");
        assert_eq!(entries.len(), 2, "{entries:?}");
    }

    #[test]
    fn enumerate_local_files_missing_root_returns_empty() {
        let bogus = std::path::Path::new("/nonexistent-eee-ffl-virtuoso-corner-test-XXX");
        assert_eq!(enumerate_local_files(bogus), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn enumerate_local_files_skips_symlinks_no_follow_no_include() {
        // `enumerate_local_files` must use `symlink_metadata` so it
        // does NOT follow symlinks. We prove this by:
        //   1. placing an ordinary real file inside the enumeration root
        //      (so we can prove the real file is still included),
        //   2. placing a symlink at the root that points to an outside
        //      FILE (its contents must never be read or enumerated),
        //   3. placing a symlink at the root that points to an outside
        //      DIRECTORY whose subtree contains a sentinel file (the
        //      subtree must never be entered or enumerated),
        // and asserting that the only entry returned is the real file.

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        // Outside target files that must NEVER appear in the result, even
        // if the walker accidentally followed a symlink.
        std::fs::write(outside.path().join("outside_file.txt"), b"OUTSIDE FILE").unwrap();
        std::fs::create_dir_all(outside.path().join("outside_dir/sub")).unwrap();
        std::fs::write(
            outside.path().join("outside_dir/sub/sentinel.txt"),
            b"OUTSIDE TREE",
        )
        .unwrap();

        // One ordinary real file inside the enumeration root.
        std::fs::write(root.path().join("real.txt"), b"REAL").unwrap();

        // Symlink → outside file.
        std::os::unix::fs::symlink(
            outside.path().join("outside_file.txt"),
            root.path().join("link_file"),
        )
        .unwrap();
        // Symlink → outside directory tree.
        std::os::unix::fs::symlink(
            outside.path().join("outside_dir"),
            root.path().join("link_dir"),
        )
        .unwrap();

        let entries = enumerate_local_files(root.path());

        // The real file must be included…
        assert!(
            entries.iter().any(|e| e == "real.txt"),
            "real file must be enumerated: {entries:?}"
        );
        // …and NO entry from the outside targets must appear, including
        // the symlink names themselves and any path under them.
        for e in &entries {
            assert!(
                e != "link_file" && e != "link_dir",
                "symlink names must not be enumerated: {e}"
            );
            assert!(
                !e.starts_with("link_file/"),
                "symlink-file tree must not be followed: {e}"
            );
            assert!(
                !e.starts_with("link_dir/"),
                "symlink-dir tree must not be followed: {e}"
            );
            assert!(
                !e.contains("outside_file")
                    && !e.contains("outside_dir")
                    && !e.contains("sentinel"),
                "no entry from outside targets may leak: {e}"
            );
        }
        // Tight size assertion: exactly the real file.
        assert_eq!(
            entries,
            vec!["real.txt".to_string()],
            "only the real file must be enumerated; got {entries:?}"
        );

        // Provenance: the outside targets are untouched (the walker
        // could not have read or followed them).
        assert_eq!(
            std::fs::read(outside.path().join("outside_file.txt")).unwrap(),
            b"OUTSIDE FILE"
        );
        assert_eq!(
            std::fs::read(outside.path().join("outside_dir/sub/sentinel.txt")).unwrap(),
            b"OUTSIDE TREE"
        );
    }

    // ── validate_output_dir_for_download ─────────────────────────────
    //
    // The output-dir preflight MUST happen BEFORE any remote side effect,
    // and must reject:
    //   - existing regular file (or any non-directory)
    //   - existing empty directory (atomic rename cannot replace it)
    //   - existing nonempty directory (preserve byte-for-byte)
    // while allowing:
    //   - nonexistent path (atomic_publish_no_replace materializes it
    //     via the renamex_np / renameat2 rename of the sibling staging
    //     tree)

    #[test]
    fn validate_output_dir_nonexistent_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join("new_subdir/that/does/not/exist");
        assert!(!candidate.exists(), "precondition: must not exist");
        let result = validate_output_dir_for_download(candidate.to_str().unwrap());
        assert!(
            result.is_ok(),
            "nonexistent path should be allowed: {result:?}"
        );
        // The returned path must round-trip exactly.
        assert_eq!(result.unwrap(), candidate);
    }

    #[test]
    fn validate_output_dir_existing_empty_dir_is_rejected_with_conflict() {
        // Per the must-not-exist contract, even an existing EMPTY directory
        // is rejected by the preflight. The atomic rename would otherwise
        // fail at publish time (EEXIST / ENOTEMPTY); surfacing it up front
        // lets the operator decide whether to remove it first.
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let err = validate_output_dir_for_download(empty.to_str().unwrap())
            .expect_err("existing empty directory must be rejected");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("already exists") || msg.contains("refusing to overwrite"),
            "error must explain refusal: {msg}"
        );
        // The empty directory must remain on disk byte-for-byte (no
        // side effects from validation).
        assert!(empty.is_dir(), "empty dir must be preserved");
        assert_eq!(
            std::fs::read_dir(&empty).unwrap().count(),
            0,
            "empty dir must remain empty"
        );
    }

    #[test]
    fn validate_output_dir_existing_nonempty_dir_is_rejected_with_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let busy = tmp.path().join("busy");
        std::fs::create_dir(&busy).unwrap();
        // Create one entry → dir is non-empty.
        std::fs::write(busy.join("existing.txt"), b"DO NOT TOUCH").unwrap();
        let err = validate_output_dir_for_download(busy.to_str().unwrap())
            .expect_err("nonempty dir must be rejected");
        // Must use the Conflict variant (preserves exit code CONFLICT).
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            (msg.contains("already exists") || msg.contains("refusing"))
                && msg.contains("preserved"),
            "error message must explain refusal + preservation: {msg}"
        );
        // Contents must be preserved byte-for-byte (NO remote side
        // effect happened, but also nothing on disk changed).
        let contents = std::fs::read(busy.join("existing.txt")).unwrap();
        assert_eq!(
            contents, b"DO NOT TOUCH",
            "nonempty dir contents must not be touched by validation"
        );
    }

    #[test]
    fn validate_output_dir_existing_regular_file_is_rejected_with_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a_file");
        std::fs::write(&file, b"data").unwrap();
        let err = validate_output_dir_for_download(file.to_str().unwrap())
            .expect_err("regular file must be rejected");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("not a directory"),
            "error must explain non-directory: {msg}"
        );
        // File contents preserved.
        assert_eq!(std::fs::read(&file).unwrap(), b"data");
    }

    #[test]
    fn validate_output_dir_does_not_touch_remote() {
        // The validator must be a pure helper: it never invokes any
        // network, SSH, or SKILL. We verify that indirectly by checking
        // the helper ignores env vars and only inspects local metadata —
        // it succeeds for a fresh nonexistent path under a tmpdir without
        // touching any SSH config.
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join("nope");
        let res = validate_output_dir_for_download(candidate.to_str().unwrap());
        assert!(
            res.is_ok(),
            "pure validation must not require remote config"
        );
    }

    // ── with_remote_dir_context ──────────────────────────────────────
    //
    // The variant preservation contract is critical: Connection / Ssh /
    // Execution get the remote_dir context appended, but other variants
    // pass through unchanged so the exit code stays correct.

    #[test]
    fn with_remote_dir_context_preserves_connection_variant() {
        let e = with_remote_dir_context(
            VirtuosoError::Connection("dial tcp: timeout".into()),
            "/tmp/x",
        );
        match e {
            VirtuosoError::Connection(m) => {
                assert!(m.contains("dial tcp: timeout"), "{m}");
                assert!(m.contains("/tmp/x"), "must include remote_dir: {m}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_preserves_ssh_variant() {
        let e = with_remote_dir_context(VirtuosoError::Ssh("permission denied".into()), "/tmp/x");
        match e {
            VirtuosoError::Ssh(m) => {
                assert!(m.contains("permission denied"), "{m}");
                assert!(m.contains("/tmp/x"), "must include remote_dir: {m}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_preserves_execution_variant() {
        let e = with_remote_dir_context(
            VirtuosoError::Execution("create netlist for corner failed: nil".into()),
            "/tmp/x",
        );
        match e {
            VirtuosoError::Execution(m) => {
                assert!(m.contains("create netlist for corner"), "{m}");
                assert!(m.contains("/tmp/x"), "{m}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_converts_timeout_to_timeout_with_context() {
        // `Timeout(secs)` carries no context, so `with_remote_dir_context`
        // MUST convert it into `TimeoutWithContext(secs, remote_dir)` so
        // the operator can correlate the timeout with the preserved
        // remote artifacts. The Timeout-family exit-code / error_type /
        // retryable semantics are preserved by `TimeoutWithContext`,
        // which is why the conversion — not flattening — is the right
        // choice.
        let e = with_remote_dir_context(VirtuosoError::Timeout(42), "/tmp/x");
        match e {
            VirtuosoError::TimeoutWithContext(secs, ctx) => {
                assert_eq!(secs, 42, "timeout seconds must be preserved: {secs}");
                assert!(
                    ctx.contains("/tmp/x"),
                    "context must include remote_dir: {ctx}"
                );
            }
            other => panic!("wrong variant: {other:?}; expected TimeoutWithContext"),
        }
    }

    #[test]
    fn with_remote_dir_context_appends_remote_dir_to_existing_timeout_context() {
        // An existing `TimeoutWithContext` that does NOT already mention
        // this remote_dir must get the standard `remote_dir=…` suffix
        // appended exactly once. The original context is preserved.
        let original = VirtuosoError::TimeoutWithContext(7, "dial timeout".into());
        let e = with_remote_dir_context(original, "/tmp/y");
        match e {
            VirtuosoError::TimeoutWithContext(secs, ctx) => {
                assert_eq!(secs, 7);
                assert!(
                    ctx.contains("dial timeout"),
                    "must preserve original message: {ctx}"
                );
                assert!(
                    ctx.contains("remote_dir=/tmp/y"),
                    "must append remote_dir marker exactly once: {ctx}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_does_not_duplicate_remote_dir_in_timeout_context() {
        // If the existing context ALREADY contains the same
        // `remote_dir=…` marker (e.g. from an earlier wrapping), the
        // second wrap must NOT introduce a duplicate occurrence.
        let original = VirtuosoError::TimeoutWithContext(
            9,
            "connect refused; netlist artifacts preserved at remote_dir=/tmp/dedupe".into(),
        );
        let e = with_remote_dir_context(original, "/tmp/dedupe");
        match e {
            VirtuosoError::TimeoutWithContext(secs, ctx) => {
                assert_eq!(secs, 9);
                let marker = "remote_dir=/tmp/dedupe";
                let count = ctx.matches(marker).count();
                assert_eq!(
                    count, 1,
                    "must not duplicate the same remote_dir marker; \
                     got {count} occurrences in: {ctx}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_passes_through_config_unchanged() {
        let e = with_remote_dir_context(
            VirtuosoError::Config("VB_REMOTE_HOST required".into()),
            "/tmp/x",
        );
        match e {
            VirtuosoError::Config(m) => {
                assert_eq!(m, "VB_REMOTE_HOST required");
                assert!(
                    !m.contains("/tmp/x"),
                    "Config must NOT get remote_dir appended"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn with_remote_dir_context_passes_through_conflict_unchanged() {
        let e = with_remote_dir_context(
            VirtuosoError::Conflict("output_dir not empty".into()),
            "/tmp/x",
        );
        match e {
            VirtuosoError::Conflict(m) => {
                assert_eq!(m, "output_dir not empty");
                assert!(!m.contains("/tmp/x"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ── atomic_publish_no_replace ──────────────────────────────────
    //
    // The authoritative publication step. The contract under test:
    //   - on a supported platform (macOS / Linux), succeed for a normal
    //     nested staging tree (staging is consumed and disappears),
    //   - refuse an existing destination (file OR empty dir OR
    //     non-empty dir) with Conflict and preserve user bytes exactly,
    //   - on Unix, refuse a destination that is itself a symlink WITHOUT
    //     following it,
    //   - simulate the TOCTOU race where the destination is created
    //     AFTER `validate_output_dir_for_download` passes but BEFORE
    //     the atomic rename fires — the rename must surface Conflict
    //     and the user's bytes must survive.
    //
    // All errors MUST mention the `remote_dir` context so the operator
    // can correlate publication failures with preserved remote artifacts.

    /// Build a valid staging tree (one regular file + one nested regular
    /// file) under the given staging root. Used by every publish test.
    fn make_nested_staging(staging: &Path) {
        std::fs::create_dir(staging.join("sub")).unwrap();
        std::fs::write(staging.join("top.txt"), b"TOP").unwrap();
        std::fs::write(staging.join("sub/inner.scs"), b"NETLIST").unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn atomic_publish_nonexistent_target_publishes_nested_tree_and_staging_disappears() {
        // The staging tree is constructed under a sibling of the (not yet
        // existing) target so the rename is single-filesystem — exactly
        // what production does. Target must NOT exist; atomic_publish
        // materializes it. After publish, staging MUST be gone (it has
        // been renamed) and output MUST contain the full nested tree.
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("netlist_target");
        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        make_nested_staging(&staging);
        assert!(!target.exists(), "precondition: target must not exist");

        let res = atomic_publish_no_replace(&staging, &target, "/tmp/rd_atom_ok");
        assert!(res.is_ok(), "atomic publish must succeed: {res:?}");

        // Staging has been consumed by the rename and MUST be gone.
        assert!(
            !staging.exists(),
            "staging dir must disappear after a successful publish; \
             still present at {}",
            staging.display()
        );
        // Output materialized with full nested tree.
        assert!(target.is_dir(), "target must now be a directory");
        assert_eq!(
            std::fs::read(target.join("top.txt")).unwrap(),
            b"TOP",
            "top-level bytes must round-trip"
        );
        assert_eq!(
            std::fs::read(target.join("sub/inner.scs")).unwrap(),
            b"NETLIST",
            "nested bytes must round-trip"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn atomic_publish_existing_empty_directory_is_conflict_and_preserved() {
        // The preflight already rejects existing empty dirs, but the
        // publish step itself MUST also refuse (publish is the
        // authoritative guard) and preserve the empty directory.
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("existing_empty");
        std::fs::create_dir(&target).unwrap();
        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("only.txt"), b"NEW").unwrap();

        let err = atomic_publish_no_replace(&staging, &target, "/tmp/rd_atom_empty")
            .expect_err("existing empty dir must conflict");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        assert!(
            err.to_string().contains("/tmp/rd_atom_empty"),
            "Conflict error must include remote_dir context: {err}"
        );
        // Empty target still on disk, still empty.
        assert!(target.is_dir(), "empty target must remain");
        assert_eq!(
            std::fs::read_dir(&target).unwrap().count(),
            0,
            "empty target must stay empty"
        );
        // Staging NOT consumed on failure.
        assert!(
            staging.exists(),
            "staging must survive a Conflict so the caller can clean it up"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn atomic_publish_existing_nonempty_directory_is_conflict_and_contents_preserved() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("existing_busy");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("existing.txt"), b"DO NOT TOUCH").unwrap();
        std::fs::write(target.join("another.txt"), b"PRESERVE").unwrap();

        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("only.txt"), b"NEW").unwrap();

        let err = atomic_publish_no_replace(&staging, &target, "/tmp/rd_atom_busy")
            .expect_err("existing non-empty dir must conflict");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        assert!(
            err.to_string().contains("/tmp/rd_atom_busy"),
            "Conflict error must include remote_dir context: {err}"
        );
        // Every byte preserved.
        assert_eq!(
            std::fs::read(target.join("existing.txt")).unwrap(),
            b"DO NOT TOUCH"
        );
        assert_eq!(
            std::fs::read(target.join("another.txt")).unwrap(),
            b"PRESERVE"
        );
        // Staging NOT consumed on failure.
        assert!(staging.exists(), "staging must survive a Conflict");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn atomic_publish_existing_file_is_conflict_and_exact_bytes_preserved() {
        // Destination is a regular file at the same path. Publish MUST
        // refuse and the file's exact bytes must survive.
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("destination_file");
        std::fs::write(&target, b"ORIGINAL-12345").unwrap();

        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("only.txt"), b"NEW").unwrap();

        let err = atomic_publish_no_replace(&staging, &target, "/tmp/rd_atom_file")
            .expect_err("existing file at target must conflict");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant, got {err:?}"
        );
        assert!(
            err.to_string().contains("/tmp/rd_atom_file"),
            "Conflict error must include remote_dir context: {err}"
        );
        // Exact bytes preserved (no truncation, no overwrite).
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"ORIGINAL-12345",
            "destination file bytes must be preserved byte-for-byte"
        );
        assert!(staging.exists(), "staging must survive a Conflict");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_publish_unix_destination_is_symlink_returns_conflict_without_following() {
        // On Unix, if the destination is itself a symlink (e.g. a
        // dangling one, or one whose target the operator does NOT want
        // touched), the atomic rename MUST surface Conflict and MUST
        // NOT follow the link to clobber the target. We use a real
        // sentinel directory as the symlink target — if the publish
        // accidentally followed the link, the sentinel contents would
        // be clobbered.
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let real_target = tempfile::tempdir().unwrap();
        // Sentinel inside the symlink target that MUST NOT be touched.
        std::fs::write(real_target.path().join("sentinel"), b"DO NOT TOUCH").unwrap();

        // Build a parent whose only top-level entry is a symlink
        // pointing at real_target.path(). The publish target is that
        // symlink itself.
        let link = parent.path().join("dest_link");
        symlink(real_target.path(), &link).unwrap();

        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("publishable.txt"), b"x").unwrap();

        let err = atomic_publish_no_replace(&staging, &link, "/tmp/rd_atom_symlink")
            .expect_err("destination that is a symlink must be rejected");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant for symlinked destination, got {err:?}"
        );
        assert!(
            err.to_string().contains("/tmp/rd_atom_symlink"),
            "Conflict error must include remote_dir context: {err}"
        );
        // Sentinel inside the symlink target must remain untouched —
        // proves we never followed the link.
        assert_eq!(
            std::fs::read(real_target.path().join("sentinel")).unwrap(),
            b"DO NOT TOUCH"
        );
        // Staging NOT consumed on failure.
        assert!(staging.exists(), "staging must survive a Conflict");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn atomic_publish_race_target_created_after_preflight_succeeds_is_conflict() {
        // Race simulation:
        //   1. `validate_output_dir_for_download` runs while target is
        //      absent → returns Ok.
        //   2. Between the preflight and the atomic rename, something
        //      else (the operator, another tool, a parallel job)
        //      materializes the destination.
        //   3. The atomic rename MUST surface Conflict and the bytes
        //      the racer just wrote MUST survive.
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("racey");
        assert!(!target.exists(), "precondition: target must not exist");

        // (1) Preflight accepts the absent path.
        let pre = validate_output_dir_for_download(target.to_str().unwrap());
        assert!(pre.is_ok(), "preflight must accept absent target: {pre:?}");

        // Build staging as a sibling so the rename is single-filesystem.
        let staging = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("data.txt"), b"NEW").unwrap();

        // (2) Racer creates the destination between preflight and publish.
        std::fs::write(&target, b"WRITTEN BY RACER").unwrap();
        assert!(target.exists(), "precondition: racer materialized target");

        // (3) Atomic rename fires; the racer's bytes MUST survive.
        let err = atomic_publish_no_replace(&staging, &target, "/tmp/rd_atom_race")
            .expect_err("rename must conflict when target appears mid-flight");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict variant for TOCTOU race, got {err:?}"
        );
        assert!(
            err.to_string().contains("/tmp/rd_atom_race"),
            "Conflict error must include remote_dir context: {err}"
        );
        // Racer's bytes preserved byte-for-byte.
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"WRITTEN BY RACER",
            "racer-written target bytes must be preserved"
        );
        // Staging NOT consumed.
        assert!(
            staging.exists(),
            "staging must survive a Conflict for caller cleanup"
        );
    }

    // ── validate_staging_recursive ─────────────────────────────────
    //
    // The validator uses `symlink_metadata` so it must NEVER follow
    // links. We assert that a symlink at any level is rejected up
    // front, and that the target's contents are NOT touched (i.e. no
    // accidental follow could have occurred).

    #[cfg(unix)]
    #[test]
    fn validate_staging_recursive_rejects_staging_file_symlink_without_following() {
        use std::os::unix::fs::symlink;
        let staging = tempfile::tempdir().unwrap();
        // Outside target that validate MUST NOT read or modify; if it
        // had followed the symlink, the content would be read.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("target_file"), b"OUTSIDE").unwrap();

        let link = staging.path().join("link");
        symlink(outside.path().join("target_file"), &link).unwrap();

        let err = validate_staging_recursive(staging.path())
            .expect_err("staging file symlink must be rejected");
        assert!(
            matches!(err, VirtuosoError::Ssh(_)),
            "expected Ssh variant for symlink rejection, got {err:?}"
        );
        assert!(
            err.to_string().contains("symlink") || err.to_string().contains("link"),
            "error must explain the symlink refusal: {err}"
        );
        // Target still intact — proves we never followed the link.
        assert_eq!(
            std::fs::read(outside.path().join("target_file")).unwrap(),
            b"OUTSIDE"
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_staging_recursive_rejects_staging_directory_symlink_without_following() {
        use std::os::unix::fs::symlink;
        let staging = tempfile::tempdir().unwrap();
        // Outside directory that the validator must never enter.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.txt"), b"OUTSIDE").unwrap();

        // Place a non-empty regular file so validate does not hit the
        // "no non-empty regular file" branch before reaching the
        // symlink — we want to specifically exercise the symlink branch.
        std::fs::write(staging.path().join("ok.txt"), b"x").unwrap();
        let sub_link = staging.path().join("sub");
        symlink(outside.path(), &sub_link).unwrap();

        let err = validate_staging_recursive(staging.path())
            .expect_err("staging directory symlink must be rejected");
        assert!(
            matches!(err, VirtuosoError::Ssh(_)),
            "expected Ssh variant for symlink rejection, got {err:?}"
        );
        // Outside sentinel still intact — proves we never followed.
        assert_eq!(
            std::fs::read(outside.path().join("outside.txt")).unwrap(),
            b"OUTSIDE"
        );
    }

    #[test]
    fn validate_staging_recursive_rejects_zero_byte_only_staging() {
        // A staging tree that has ONLY zero-byte regular files (and
        // possibly empty directories) must be rejected. This mirrors
        // the remote `find -size +0c` contract.
        let staging = tempfile::tempdir().unwrap();
        std::fs::write(staging.path().join("placeholder"), b"").unwrap();
        std::fs::create_dir(staging.path().join("empty_subdir")).unwrap();

        let err = validate_staging_recursive(staging.path())
            .expect_err("zero-byte-only staging must be rejected");
        assert!(
            matches!(err, VirtuosoError::Ssh(_)),
            "expected Ssh variant for zero-byte rejection, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("non-empty") || msg.contains("size"),
            "error must reference the non-empty requirement: {msg}"
        );
    }

    // ── StagingGuard ────────────────────────────────────────────────
    //
    // The RAII guard's contract:
    //   - removes its OWN controlled-prefix path on drop,
    //   - refuses to delete any path whose leaf name does NOT start
    //     with the fixed `CORNER_NETLIST_STAGING_PREFIX`, OR whose
    //     suffix is not exactly 32 lowercase hex chars, OR whose
    //     parent directory is not the expected one.
    // The prefix + suffix + parent checks together form the single
    // defence against accidental data loss from a buggy refactor that
    // passes an arbitrary path in.

    /// Build a controlled-prefix leaf name under `parent`: exact
    /// prefix + exactly 32 lowercase hex chars (UUIDv4-style).
    fn make_valid_staging_path(parent: &Path) -> PathBuf {
        let path = parent.join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn staging_guard_removes_own_valid_prefix_path_in_expected_parent_on_drop() {
        // The happy path: leaf starts with the controlled prefix AND
        // the suffix is exactly 32 lowercase hex chars AND the parent
        // matches the parent the guard was constructed for. The guard
        // MUST remove it on drop.
        let parent = tempfile::tempdir().unwrap();
        let path = make_valid_staging_path(parent.path());
        std::fs::write(path.join("marker"), b"data").unwrap();
        assert!(path.exists(), "precondition: staging dir must exist");

        {
            let _guard = StagingGuard::new(path.clone(), path.parent().unwrap().to_path_buf());
            // Drop at end of scope.
        }
        assert!(
            !path.exists(),
            "guard MUST remove its own controlled-prefix path in the \
             expected parent on drop"
        );
    }

    #[test]
    fn staging_guard_refuses_to_delete_path_with_invalid_prefix() {
        // Construct a path whose leaf name does NOT start with the
        // controlled prefix. The guard's prefix check must save the
        // day — the directory and its contents must remain intact.
        let parent = tempfile::tempdir().unwrap();
        let invalid = parent.path().join(format!(
            "vcli_NOT_A_STAGING_DIR_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(invalid.join("marker"), b"DO NOT DELETE").unwrap();
        assert!(
            invalid.exists(),
            "precondition: invalid-prefix dir must exist"
        );

        {
            let _guard =
                StagingGuard::new(invalid.clone(), invalid.parent().unwrap().to_path_buf());
            // Drop at end of scope; guard must NOT touch this path.
        }
        assert!(
            invalid.exists(),
            "guard MUST NOT delete an invalid-prefix path: {}",
            invalid.display()
        );
        assert_eq!(
            std::fs::read(invalid.join("marker")).unwrap(),
            b"DO NOT DELETE"
        );

        // Cleanup: do it manually since the guard refused.
        let _ = std::fs::remove_dir_all(&invalid);
    }

    #[test]
    fn staging_guard_refuses_to_delete_path_with_wrong_suffix_length() {
        // The leaf name has the right prefix but the suffix is NOT
        // 32 chars. Both shorter and longer suffixes must be refused.
        let parent = tempfile::tempdir().unwrap();
        for suffix in [
            // Too short.
            "short",
            // Empty.
            "",
            // One too many.
            &"a".repeat(33),
        ] {
            let bad = parent
                .path()
                .join(format!("{}{}", CORNER_NETLIST_STAGING_PREFIX, suffix));
            std::fs::create_dir_all(&bad).unwrap();
            std::fs::write(bad.join("marker"), b"KEEP").unwrap();

            {
                let _guard = StagingGuard::new(bad.clone(), bad.parent().unwrap().to_path_buf());
                // Drop at end of scope; guard must NOT touch this path.
            }
            assert!(
                bad.exists(),
                "guard MUST NOT delete wrong-suffix-length path \
                 (suffix={suffix:?}): {}",
                bad.display()
            );
            assert_eq!(std::fs::read(bad.join("marker")).unwrap(), b"KEEP");

            let _ = std::fs::remove_dir_all(&bad);
        }
    }

    #[test]
    fn staging_guard_refuses_to_delete_path_with_nonhex_suffix() {
        // The leaf name has the right prefix and the right length but
        // the suffix contains a non-hex char (`g` is not in [0-9a-f]).
        // The guard's hex check MUST save the day.
        let parent = tempfile::tempdir().unwrap();
        let bad = parent.path().join(format!(
            "{}g{}",
            CORNER_NETLIST_STAGING_PREFIX,
            "a".repeat(31)
        ));
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("marker"), b"KEEP").unwrap();

        {
            let _guard = StagingGuard::new(bad.clone(), bad.parent().unwrap().to_path_buf());
        }
        assert!(
            bad.exists(),
            "guard MUST NOT delete non-hex-suffix path: {}",
            bad.display()
        );
        assert_eq!(std::fs::read(bad.join("marker")).unwrap(), b"KEEP");

        let _ = std::fs::remove_dir_all(&bad);
    }

    #[test]
    fn staging_guard_refuses_to_delete_path_with_uppercase_hex_suffix() {
        // The leaf name has the right prefix and length but the
        // suffix is uppercase hex (`A` instead of `a`). The guard's
        // lowercase-only hex check MUST save the day so a tampered
        // or non-UUID leaf is never confused with our own.
        let parent = tempfile::tempdir().unwrap();
        let bad = parent.path().join(format!(
            "{}{}",
            CORNER_NETLIST_STAGING_PREFIX,
            "A".repeat(32)
        ));
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("marker"), b"KEEP").unwrap();

        {
            let _guard = StagingGuard::new(bad.clone(), bad.parent().unwrap().to_path_buf());
        }
        assert!(
            bad.exists(),
            "guard MUST NOT delete uppercase-hex-suffix path: {}",
            bad.display()
        );
        assert_eq!(std::fs::read(bad.join("marker")).unwrap(), b"KEEP");

        let _ = std::fs::remove_dir_all(&bad);
    }

    #[test]
    fn staging_guard_refuses_to_delete_path_in_wrong_parent() {
        // The leaf name has the right prefix, length, and lowercase
        // hex suffix — BUT the guard was constructed with a parent
        // that does NOT match the path's actual parent. The guard's
        // same-parent check MUST refuse to remove it.
        let original_parent = tempfile::tempdir().unwrap();
        let other_parent = tempfile::tempdir().unwrap();

        // Build a valid staging-shaped path under `original_parent`.
        let path = make_valid_staging_path(original_parent.path());
        std::fs::write(path.join("marker"), b"KEEP").unwrap();
        assert!(path.exists(), "precondition: path must exist");

        {
            // Construct the guard with a DIFFERENT expected parent
            // (one that does NOT match the path's actual parent).
            // The guard's same-parent check must refuse to remove it
            // on drop because `path.parent()` (original_parent) !=
            // `other_parent`.
            let _guard = StagingGuard::new(path.clone(), other_parent.path().to_path_buf());
        }
        assert!(
            path.exists(),
            "guard MUST NOT delete a valid-name path whose parent \
             does not match the expected parent: {}",
            path.display()
        );
        assert_eq!(std::fs::read(path.join("marker")).unwrap(), b"KEEP");

        // Cleanup: do it manually since the guard refused.
        let _ = std::fs::remove_dir_all(&path);
    }

    // ── corner_netlist_staging_path ─────────────────────────────────
    //
    // The helper must produce a sibling of `output_dir` (same parent),
    // carry the exact hidden prefix + exactly 32 lowercase hex chars,
    // and never embed any user-supplied fragment (so a user-chosen
    // `output_dir` basename can never collide with the staging leaf).

    #[test]
    fn corner_netlist_staging_path_is_sibling_of_output_with_controlled_prefix_and_hex_suffix() {
        let output = std::path::Path::new("/tmp/some/operator_supplied/dir");
        let staging = corner_netlist_staging_path(output);

        // Sibling: same parent as output.
        assert_eq!(
            staging.parent(),
            output.parent(),
            "staging must be a sibling of output, sharing its parent"
        );

        let leaf = staging
            .file_name()
            .and_then(|n| n.to_str())
            .expect("staging leaf must be valid UTF-8");

        // Exact hidden prefix.
        assert!(
            leaf.starts_with(CORNER_NETLIST_STAGING_PREFIX),
            "staging leaf '{leaf}' must start with the hidden prefix '{CORNER_NETLIST_STAGING_PREFIX}'"
        );

        // Suffix is exactly 32 lowercase hex chars (UUIDv4 simple).
        let suffix = &leaf[CORNER_NETLIST_STAGING_PREFIX.len()..];
        assert_eq!(
            suffix.len(),
            32,
            "suffix must be exactly 32 chars; got {suffix:?}"
        );
        assert!(
            suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "suffix must be lowercase hex only; got {suffix:?}"
        );

        // No fragment of the user-supplied output basename may appear
        // anywhere in the staging leaf. The user could pass any string
        // as the `output_dir` basename; the staging leaf must NOT be
        // derivable from it (so a malicious or accidental user value
        // can never shadow or collide with staging).
        let user_basename = output
            .file_name()
            .and_then(|n| n.to_str())
            .expect("output basename must be valid UTF-8");
        assert!(
            !leaf.contains(user_basename),
            "staging leaf '{leaf}' must NOT contain the user basename \
             '{user_basename}'"
        );
        // Defensive: even when the user basename is also a hex string,
        // the staging leaf has the hidden prefix so the basename alone
        // could not match. This holds here because the prefix is fixed.
        assert!(
            !leaf.ends_with(user_basename),
            "staging leaf '{leaf}' must NOT end with the user basename"
        );
    }

    #[test]
    fn corner_netlist_staging_path_is_unique_across_many_calls() {
        // Same as the remote path uniqueness contract: collisions have
        // probability ~2^-122 across UUIDv4 calls. 256 calls is plenty
        // to detect any regression that re-uses a suffix.
        let output = std::path::Path::new("/tmp/dir");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let p = corner_netlist_staging_path(output);
            assert!(
                seen.insert(p.clone()),
                "duplicate staging path '{}' across calls",
                p.display()
            );
        }
        assert_eq!(seen.len(), 256);
    }
}

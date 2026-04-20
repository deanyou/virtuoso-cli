use crate::client::bridge::VirtuosoClient;
use crate::commands::schematic::parse_skill_json;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn open(lib: &str, cell: &str, view: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.open_session(lib, cell, view);
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to open maestro session: {}",
            r.output
        )));
    }
    Ok(json!({
        "status": "success",
        "session": r.output.trim_matches('"'),
        "lib": lib,
        "cell": cell,
        "view": view,
    }))
}

pub fn close(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.close_session(session);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
        "output": r.output,
    }))
}

pub fn list_sessions() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_sessions();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list sessions: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn set_var(name: &str, value: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.set_var(name, value);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "variable": name,
        "value": value,
        "output": r.output,
    }))
}

pub fn get_var(name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_var(name);
    let r = client.execute_skill(&skill, None)?;
    let value = if r.skill_ok() {
        r.output.trim_matches('"').to_string()
    } else {
        r.output.clone()
    };
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "variable": name,
        "value": value,
    }))
}

pub fn list_vars() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_vars();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list variables: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn get_analyses(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let version = client.version()?;
    let skill = client.maestro.get_analyses(session, version);
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get analyses for session '{}': {}",
            session, r.output
        )));
    }
    Ok(json!({
        "session": session,
        "analyses": r.output.trim_matches('"'),
    }))
}

pub fn set_analysis(
    session: &str,
    analysis_type: &str,
    options: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let version = client.version()?;

    // Validate --options JSON and convert to SKILL alist; warn if IC23 path ignores it.
    let options_alist: Option<String> = match options {
        None => None,
        Some(opts) => {
            let alist = crate::client::maestro_ops::json_to_skill_alist(opts)
                .map_err(|e| VirtuosoError::Execution(format!("--options: {e}")))?;
            if !version.is_ic25() {
                eprintln!(
                    "warning: --options is only supported on IC25; ignoring on IC23 path"
                );
                None
            } else {
                Some(alist)
            }
        }
    };

    let skill = client.maestro.set_analysis(session, analysis_type, options_alist.as_deref(), version);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
        "analysis": analysis_type,
    }))
}

pub fn run(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.run_simulation(session);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "launched" } else { "error" },
        "session": session,
        "output": r.output.trim_matches('"'),
    }))
}

pub fn add_output(output_name: &str, test_name: &str, expr: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.add_output(output_name, test_name, expr);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output_name": output_name,
        "test_name": test_name,
        "expression": expr,
        "output": r.output,
    }))
}

pub fn save(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.save_setup(session);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
    }))
}

pub fn export(session: &str, path: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.export_results(session, path);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
        "path": path,
        "output": r.output,
    }))
}

/// Inspect the focused ADE window and return session metadata.
///
/// Always makes one SKILL call (focused window title + session list).
/// Makes a second call only when `session` is provided (to get run_dir).
pub fn session_info(session: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    let skill = client.maestro.focused_window_skill();
    let r = client.execute_skill(&skill, None)?;

    // SKILL output: ("ADE Assembler Editing: LIB CELL VIEW*" ("t1" ...) ("sess1" ...))
    let focused = extract_first_skill_string(&r.output);
    let parsed = focused.as_deref().and_then(parse_ade_title);

    let run_dir = if let Some(s) = session {
        let skill2 = client.maestro.run_dir_skill(s);
        let r2 = client.execute_skill(&skill2, None)?;
        if r2.skill_ok() {
            Some(r2.output.trim_matches('"').to_string())
        } else {
            None
        }
    } else {
        None
    };

    Ok(json!({
        "status": "success",
        "focused_window": focused,
        "session": session,
        "application": parsed.as_ref().map(|p| p.application.as_str()),
        "lib": parsed.as_ref().map(|p| p.lib.as_str()),
        "cell": parsed.as_ref().map(|p| p.cell.as_str()),
        "view": parsed.as_ref().map(|p| p.view.as_str()),
        "editable": parsed.as_ref().map(|p| p.editable),
        "unsaved_changes": parsed.as_ref().map(|p| p.unsaved_changes),
        "run_dir": run_dir,
    }))
}

/// Extract the first quoted string from a SKILL list like `("foo" ...)`.
fn extract_first_skill_string(s: &str) -> Option<String> {
    let s = s.trim().strip_prefix('(')?;
    let start = s.find('"')?;
    let inner = &s[start + 1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
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
    let rest = title.strip_prefix("ADE ")?;

    let (app, rest) = if let Some(r) = rest.strip_prefix("Assembler ") {
        ("assembler", r)
    } else if let Some(r) = rest.strip_prefix("Explorer ") {
        ("explorer", r)
    } else {
        return None;
    };

    let (editable, rest) = if let Some(r) = rest.strip_prefix("Editing: ") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix("Reading: ") {
        (false, r)
    } else {
        return None;
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
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "history": history,
        "output": r.output,
    }))
}

/// Close the currently open results.
pub fn close_results() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.close_results();
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
    }))
}

/// List all test names that have results in the current history.
pub fn get_result_tests() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_result_tests();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get result tests: {}",
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
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get result outputs: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

/// Get the value of a specific output for a specific test and corner.
pub fn get_output_value(name: &str, test_name: &str, corner: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_output_value(name, test_name, corner);
    let r = client.execute_skill(&skill, None)?;
    let value = if r.skill_ok() {
        r.output.trim_matches('"').to_string()
    } else {
        r.output.clone()
    };
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output_name": name,
        "test_name": test_name,
        "corner": corner,
        "value": value,
    }))
}

/// Get the spec pass/fail status for an output.
pub fn get_spec_status(name: &str, test_name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_spec_status(name, test_name);
    let r = client.execute_skill(&skill, None)?;
    let status = if r.skill_ok() {
        r.output.trim_matches('"').to_string()
    } else {
        r.output.clone()
    };
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output_name": name,
        "test_name": test_name,
        "spec_status": status,
    }))
}

/// Get simulation messages (errors/warnings) from the last run.
pub fn get_sim_messages(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_sim_messages(session);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
        "messages": r.output,
    }))
}

/// List available history runs for the current Maestro session.
pub fn get_history_list() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_history_list();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get history list: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

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
    Ok(parse_skill_json(&r.output))
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
    Ok(parse_skill_json(&r.output))
}

pub fn get_analyses(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_analyses(session);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "session": session,
        "analyses": r.output.trim_matches('"'),
        "raw": r.output,
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
    Ok(parse_skill_json(&r.output))
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
    Ok(parse_skill_json(&r.output))
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
    Ok(parse_skill_json(&r.output))
}

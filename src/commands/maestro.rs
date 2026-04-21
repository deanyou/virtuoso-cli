use crate::client::bridge::VirtuosoClient;
use crate::commands::schematic::parse_skill_json;
use crate::error::{Result, VirtuosoError};
use crate::models::VirtuosoResult;
use serde_json::{json, Value};

/// Extract a scalar string value from a SKILL result: strips quotes on success, returns raw on error.
fn skill_str(r: &VirtuosoResult) -> String {
    if r.skill_ok() {
        r.output.trim_matches('"').to_string()
    } else {
        r.output.clone()
    }
}

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
    if !r.ok() {
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
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "variable": name,
        "value": skill_str(&r),
    }))
}

pub fn list_vars() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_vars();
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list variables: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn get_analyses(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_analyses(session);
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get analyses for session '{}': {}",
            session, r.output
        )));
    }
    parse_skill_json(&r.output)
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

pub fn export(
    session: &str,
    path: &str,
    test_name: Option<&str>,
    history: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.export_results(session, path, test_name, history);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "session": session,
        "path": path,
        "test_name": test_name,
        "history": history,
        "export_path": skill_str(&r),
    }))
}

/// Inspect the focused ADE window and return session metadata.
///
/// Always makes one SKILL call. A second call is made only when `session` is explicitly
/// provided AND it differs from the davSession bound to the focused window.
pub fn session_info(session: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    let skill = client.maestro.focused_window_skill();
    let r = client.execute_skill(&skill, None)?;

    // SKILL output: (title davSession (all_titles...) (sessions...) run_dir_or_nil)
    let tokens = parse_skill_list_top_level(&r.output);
    let focused = tokens.first().and_then(|t| extract_skill_string_token(t));
    let dav_session = tokens.get(1).and_then(|t| extract_skill_string_token(t));
    let bundled_run_dir = tokens.get(4).and_then(|t| extract_skill_string_token(t));

    let parsed = focused.as_deref().and_then(parse_ade_title);

    // Resolve effective session: explicit arg → davSession from window → None
    let effective_session = session.map(str::to_owned).or_else(|| dav_session.clone());

    // Second RTT only when session explicitly differs from focused window's davSession
    let run_dir = if let Some(s) = session.filter(|s| Some(*s) != dav_session.as_deref()) {
        let skill2 = client.maestro.run_dir_skill(s);
        let r2 = client.execute_skill(&skill2, None)?;
        if r2.skill_ok() {
            Some(r2.output.trim_matches('"').to_string())
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
    if !r.ok() {
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
    if !r.ok() {
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
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output_name": name,
        "test_name": test_name,
        "corner": corner,
        "value": skill_str(&r),
    }))
}

/// Get the spec pass/fail status for an output.
pub fn get_spec_status(name: &str, test_name: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_spec_status(name, test_name);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output_name": name,
        "test_name": test_name,
        "spec_status": skill_str(&r),
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

/// List available history runs for a Maestro session.
pub fn get_history_list(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.get_history_list(session);
    let r = client.execute_skill(&skill, None)?;
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get history list: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
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

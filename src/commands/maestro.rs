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

/// Prefix every Maestro (ADE) session name carries.
const MAESTRO_SESSION_PREFIX: &str = "fnxSession";

/// What a `--session` value names, when the command is a Maestro subcommand.
///
/// Maestro subcommands carry their own `--session` flag for the *Maestro*
/// session name (`fnxSessionN`), while the global `--session` selects the
/// *Virtuoso bridge* session (`<host>-<user>-<port>`). Clap folds both flags
/// into one value, so the value has to be classified before it can be used —
/// writing a Maestro session name into `VB_SESSION` makes the client look for
/// a bridge session file that does not exist and fall back to `VB_PORT=0`
/// (ECONNREFUSED), and ignoring a bridge session id makes the client pick a
/// *different* Virtuoso instance than the one the user named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionArgKind {
    /// A bridge session id (`<host>-<user>-<port>`) — selects the Virtuoso
    /// instance and must be written to `VB_SESSION`.
    Bridge,
    /// A Maestro session name (`fnxSessionN`) — Maestro only, never
    /// `VB_SESSION`.
    Maestro,
    /// Undecidable — the caller must fail loudly instead of guessing.
    Unknown,
}

/// Classify a `--session` value by shape alone (no filesystem access).
///
/// The bridge session id is built by the SKILL bridge as
/// `sprintf(nil "%s-%d" RBSessionBase RBPort)` where `RBSessionBase` is
/// `<hostname>-<username>` (see `resources/ramic_bridge.il`), so a bridge id
/// always ends in a numeric port and carries at least three dash-separated
/// fields.
pub fn classify_session_arg(value: &str) -> SessionArgKind {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return SessionArgKind::Unknown;
    }
    if trimmed.starts_with(MAESTRO_SESSION_PREFIX) {
        return SessionArgKind::Maestro;
    }
    if looks_like_bridge_session_id(trimmed) {
        return SessionArgKind::Bridge;
    }
    SessionArgKind::Unknown
}

/// `<host>-<user>-<port>` → the trailing field is the port.
fn looks_like_bridge_session_id(value: &str) -> bool {
    let mut fields = value.rsplitn(2, '-');
    let port = match fields.next() {
        Some(p) => p,
        None => return false,
    };
    let head = match fields.next() {
        Some(h) => h,
        None => return false,
    };
    // `<host>-<user>-<port>` needs at least three dash-separated fields; a
    // bare `name-1234` is not enough to call something a bridge session id.
    if !head.contains('-') {
        return false;
    }
    match port.parse::<u16>() {
        Ok(p) => p > 0,
        Err(_) => false,
    }
}

/// Is this id registered as a live Virtuoso session (`vcli session list`)?
///
/// The registry is the authoritative answer to "does this id name a Virtuoso
/// instance"; the shape heuristic above is only a fallback for ids that are
/// not registered (yet).
fn is_registered_bridge_session(value: &str) -> bool {
    crate::models::SessionInfo::list()
        .map(|sessions| sessions.iter().any(|s| s.id == value))
        .unwrap_or(false)
}

/// Resolve the `VB_SESSION` value implied by a Maestro command's `--session`.
///
/// * `Ok(Some(id))` — the value names a Virtuoso bridge session: write it to
///   `VB_SESSION` so the client connects to *that* instance.
/// * `Ok(None)` — the value names a Maestro session (`fnxSessionN`): it must
///   never reach `VB_SESSION`.
/// * `Err(msg)` — the value cannot be classified. The caller must fail
///   instead of silently connecting to some other Virtuoso instance.
pub fn resolve_maestro_session_env(value: &str) -> std::result::Result<Option<String>, String> {
    match classify_session_arg(value) {
        SessionArgKind::Bridge => Ok(Some(value.trim().to_string())),
        SessionArgKind::Maestro => Ok(None),
        SessionArgKind::Unknown => {
            if is_registered_bridge_session(value.trim()) {
                Ok(Some(value.trim().to_string()))
            } else {
                Err(ambiguous_session_arg_message(value))
            }
        }
    }
}

/// Error text for a `--session` value that names neither a bridge session nor a
/// Maestro session.
///
/// The message must stay unambiguous: it points at `VB_SESSION` for the
/// Virtuoso instance and at the *subcommand's* `--session` for the Maestro
/// session, so it never tells the user to "use --session" for the wrong thing.
pub fn ambiguous_session_arg_message(value: &str) -> String {
    format!(
        "cannot tell what '--session {value}' refers to: it is neither a Virtuoso bridge \
         session id (<host>-<user>-<port>) nor a Maestro session name ({MAESTRO_SESSION_PREFIX}N).\n\
         Select the Virtuoso instance with VB_SESSION=<bridge-session-id> \
         (e.g. 'VB_SESSION=host-user-1234 vcli maestro ...'),\n\
         and select the Maestro session with the subcommand's own --session \
         (e.g. 'vcli maestro run --session {MAESTRO_SESSION_PREFIX}0')."
    )
}

/// Rejects anything but `"r"` / `"a"` before it reaches SKILL.
///
/// `maeOpenSetup` ignores an unrecognised `?mode` and falls back to its own
/// default, which is `"a"` — the locking one. So a typo like `"ro"` would
/// silently do the opposite of what was asked and leave a human locked out of
/// the cell; that is worth an error rather than a guess.
pub(crate) fn check_session_mode(mode: &str) -> Result<()> {
    if mode != "r" && mode != "a" {
        return Err(VirtuosoError::Execution(format!(
            "mode must be \"r\" (read, no edit lock) or \"a\" (append, takes the edit lock), \
             got {mode:?}"
        )));
    }
    Ok(())
}

pub fn open(lib: &str, cell: &str, view: &str, mode: &str) -> Result<Value> {
    check_session_mode(mode)?;
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.open_session(lib, cell, view, mode);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("open session")?;
    Ok(json!({
        "status": "success",
        "session": r.output_unquoted(),
        "lib": lib,
        "cell": cell,
        "view": view,
        "mode": mode,
    }))
}

/// Takes or hands back the edit lock on an already-open session.
///
/// This is the escape hatch that makes the read-only default usable: open
/// `"r"`, switch to `"a"` for the writes, switch back. The lock file follows
/// each call exactly.
pub fn set_mode(session: &str, mode: &str) -> Result<Value> {
    check_session_mode(mode)?;
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.set_session_mode(session, mode == "a");
    client
        .execute_skill(&skill, None)?
        .ok_or_exec("set session mode")?;
    Ok(json!({
        "status": "success",
        "session": session,
        "mode": mode,
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

/// The corner names a `create_corner_netlist` call will accept.
///
/// `ok_or_exec` rather than `decode_json` alone: the SKILL raises `error()` for
/// an unknown session, and that text arrives in `errors` with `output` empty —
/// decoding first would report a real diagnostic as a JSON parse failure.
pub fn list_corners(session: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.list_corners(session);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("list Maestro corners")?;
    decode_json(&r, "list Maestro corners")
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

pub fn get_analyses(session: &str, test: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let version = client.version()?;
    let skill = client.maestro.get_analyses(session, test, version);
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

pub fn set_analysis(
    session: &str,
    analysis_type: &str,
    options: Option<&str>,
    test: Option<&str>,
) -> Result<Value> {
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

    let skill = client.maestro.set_analysis(
        session,
        analysis_type,
        options_alist.as_deref(),
        test,
        version,
    );
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
                .set_analysis(session, at, alist.as_deref(), None, version);
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
            // Unchecked — SKILL is generated internally, and the capability
            // check already ran at RPC dispatch. `maestro.snapshot` is the
            // forensics tool: gating it makes it unavailable to non-Admin
            // callers exactly when something has gone wrong and they need it.
            let r = client.execute_skill_unchecked(&skill, None)?;
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
    // Unchecked — called from `snapshot`; see the note there.
    let r = client.execute_skill_unchecked(&skill, None)?;

    let tokens = parse_skill_list_top_level(&r.output);
    let dav_session = tokens.get(1).and_then(|t| extract_skill_string_token(t));

    let effective = session.map(String::from).or(dav_session.clone());

    let run_dir = if let Some(ref s) = effective {
        if Some(s.as_str()) != dav_session.as_deref() {
            let skill2 = client.maestro.run_dir_skill(s);
            // Unchecked — called from `snapshot`; see the note there.
            let r2 = client.execute_skill_unchecked(&skill2, None)?;
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

/// How many trailing lines of each netlister log to bring back on failure.
/// Enough for the SFE/error block at the end, short enough not to bury it.
pub(crate) const CORNER_NETLIST_LOG_TAIL_LINES: usize = 40;

/// How many log files to read back. `si.foregnd.log` is the one that matters;
/// the cap is there so a corner sweep that dropped fifty logs cannot flood the
/// error message.
pub(crate) const CORNER_NETLIST_LOG_MAX_FILES: usize = 3;

/// Build the shell command that reads back whatever the netlister logged.
///
/// `maeCreateNetlistForCorner` collapses *every* failure into a bare `nil` —
/// a missing model file, an unknown corner name and a bad design variable all
/// look identical from SKILL. The actual reason is written to `si.foregnd.log`
/// under the export dir, which the failure path already preserves. Reading it
/// back is the difference between "failed: nil" and a diagnosis.
///
/// Best-effort by construction: `2>/dev/null` and `-r` mean a missing dir or
/// zero logs produce empty output rather than a second error on top of the
/// first.
pub(crate) fn corner_netlist_log_command(remote_dir: &str) -> String {
    format!(
        "find {} -maxdepth 3 -type f -name '*.log' -print 2>/dev/null | sort | head -{} | xargs -r tail -v -n {}",
        shell_quote(remote_dir),
        CORNER_NETLIST_LOG_MAX_FILES,
        CORNER_NETLIST_LOG_TAIL_LINES,
    )
}

/// Append a diagnostic line to a failure while keeping its variant.
///
/// Two reasons not to just `format!("{err}\n{extra}")`: the variant decides the
/// exit code and classification (`with_remote_dir_context` above guards the same
/// property), and `Display` already prints the `"execution failed: "` prefix, so
/// re-wrapping would print it twice.
///
/// Variants that carry no message pass through — the diagnostics below only ever
/// see `Execution`, since they run on the `ok_or_exec` path.
fn append_diagnostic(err: VirtuosoError, extra: &str) -> VirtuosoError {
    match err {
        VirtuosoError::Connection(m) => VirtuosoError::Connection(format!("{m}\n{extra}")),
        VirtuosoError::Ssh(m) => VirtuosoError::Ssh(format!("{m}\n{extra}")),
        VirtuosoError::Execution(m) => VirtuosoError::Execution(format!("{m}\n{extra}")),
        other => other,
    }
}

/// Append the netlister's own log tail to a failure message.
///
/// Best-effort: if the SSH call fails or there is nothing to read, the original
/// error is returned untouched. A diagnostic that can itself fail must never
/// replace the failure it is diagnosing.
fn with_netlister_log(
    err: VirtuosoError,
    ssh: &dyn RemoteTransport,
    remote_dir: &str,
) -> VirtuosoError {
    let cmd = corner_netlist_log_command(remote_dir);
    let Ok(r) = ssh.run_command(&CommandRequest::untimed(&cmd)) else {
        return err;
    };
    let log = r.stdout.trim();
    if log.is_empty() {
        return err;
    }
    append_diagnostic(err, &format!("--- netlister log ({remote_dir}) ---\n{log}"))
}

/// The hint appended when the netlister rejects the corner name.
///
/// `corners_json` is the raw `maestro.list_corners` payload. Split out from the
/// RTT so the wording is testable without a bridge.
///
/// Worth spelling out because the wrong answer is the intuitive one: an ADE
/// corner is a *setup label* (`C0`, `Nominal`), while `tt`/`ss`/`ff` are PDK
/// model sections. They read alike, and `maeCreateNetlistForCorner` reports the
/// difference as a bare `nil`.
pub(crate) fn corner_hint(corner: &str, corners_json: &str) -> String {
    let listed = corners_json.trim();
    if listed.is_empty() || listed == "[]" {
        format!(
            "corner {corner:?}: this Maestro setup defines no corners at all, \
             so no corner name can succeed"
        )
    } else {
        format!(
            "corner {corner:?}: this Maestro setup defines {listed} — corner names are \
             ADE setup labels, not PDK model sections like tt/ss/ff (maestro.list_corners)"
        )
    }
}

/// Name the corners the setup actually has, on the failure path only.
///
/// Best-effort, same contract as [`with_netlister_log`]: a diagnostic that can
/// itself fail must never replace the failure it is diagnosing.
fn with_available_corners(
    err: VirtuosoError,
    client: &VirtuosoClient,
    session: &str,
    corner: &str,
) -> VirtuosoError {
    let skill = client.maestro.list_corners(session);
    let Ok(r) = client.execute_skill_unchecked(&skill, None) else {
        return err;
    };
    let Ok(r) = r.ok_or_exec("list corners") else {
        return err;
    };
    append_diagnostic(err, &corner_hint(corner, r.output_unquoted()))
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
        // renameat2 is a glibc 2.28+ symbol; CentOS 7 (glibc 2.17)
        // links fail because the symbol is absent. Delegate to
        // sys::linux_rename which resolves via dlsym / raw syscall /
        // stat+renameat fallback transparently.
        let res = unsafe {
            crate::sys::linux_rename::rename_noreplace(
                libc::AT_FDCWD,
                src_c.as_ptr(),
                libc::AT_FDCWD,
                dst_c.as_ptr(),
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
///   1. Validate that VB_REMOTE_HOST is configured (env or config.toml).
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
             Set it in the environment (VB_REMOTE_HOST=eda-server) or in \
             config.toml (run `vcli init` to create one)."
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
        .execute_skill_unchecked(&skill, None)
        .map_err(|e| with_remote_dir_context(e, &remote_dir))?;
    if let Err(e) = exec_result.ok_or_exec("create netlist for corner") {
        // The SKILL side said `nil` and nothing else. Whatever the netlister
        // actually complained about is in the log it just wrote — read it back
        // before reporting, otherwise the operator gets a path and a shrug.
        // And when the netlister wrote no log at all (the usual case for a name
        // it rejected outright), name the corners that would have worked.
        let e = with_netlister_log(e, ssh.as_ref(), &remote_dir);
        let e = with_available_corners(e, &client, session, corner);
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
        // user can decide what to do. Same reasoning as step 7: SKILL
        // returned non-nil yet produced nothing, so the "why" is only in
        // the netlister's log.
        return Err(with_netlister_log(
            VirtuosoError::Execution(format!(
                "remote netlist dir {remote_dir} contains no non-empty regular files \
                 (no files produced for corner '{corner}'); preserved for forensics"
            )),
            ssh.as_ref(),
            &remote_dir,
        ));
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

// ============================================================================
// Persistent Maestro Jobs  (analogous to PR #173)
//
// Safety semantics:
// - Every submit writes a record. `maeRunSimulation` forks the Spectre
//   child, so a submit that cannot be confirmed still leaves a record
//   behind (status=unknown) — there is no orphan simulation.
// - The run id is read from the submit itself (`maeRunSimulation
//   (?returnRunId t)`), never guessed in advance: `maeGetSetup` has no
//   `?runId` keyword, so there is no way to ask for the id of a run that has
//   not happened yet.
// - The writability gate fails closed: only an explicit "writable" from the
//   probe lets a submit through.
// - The client-id gate compares `resolve_client_id()` — not the raw
//   `VB_PROFILE`, which is not an identity on its own.
// - Status polling never contacts CIW. It reads the persisted JSON and asks
//   the run host about the run directory, over the same transport
//   `job_logs` uses.
// - Terminal states are sticky and are persisted the first time they are
//   observed, so the `--status` filter and the displayed status agree.
// ============================================================================

use crate::client::bridge::resolve_client_id;
use crate::maestro_jobs::{MaestroJob, MaestroJobStatus, MaestroJobStore};
use crate::transport::backend;
use chrono::Utc;
use std::collections::HashMap;

/// Submit a Maestro simulation job (non-blocking).
///
/// The flow is: probe the session, check the client identity, submit once,
/// then record what happened.
///
/// - The **writability probe** must answer `"writable"`. `"readonly"` and
///   `"unknown"` both refuse the submit — and `"unknown"` is also what a
///   session that does not exist answers, because `axlIsSessionReadOnly`
///   returns `nil` for a writable session *and* for a missing one. The gate
///   fails closed: only an explicit `"writable"` lets a submit through.
/// - The **client-identity gate** compares `resolve_client_id()`
///   (`VB_CLIENT_ID` → `VB_PROFILE` → hostname → `"default"`) against the
///   client id the session carries and against the client ids of the jobs
///   already recorded for that session. The store directory is shared by
///   every profile, so those records are the only record of who owns the
///   session.
/// - **The submit happens exactly once.** `maeRunSimulation` is never
///   retried, and the run id comes from the submit itself.
/// - **A record always exists afterwards.** Once `maeRunSimulation` has been
///   issued the Spectre child may exist, so the job is written to the store
///   even when the run id turns out to belong to another job: a duplicate
///   becomes a visible, cancellable record instead of an orphan simulation.
pub fn job_submit(session: &str, test: Option<&str>, name: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let store = MaestroJobStore::new()?;

    // 1. Writability probe. The probe establishes that the session exists
    //    before "not read-only" can be read as "writable", and reports one
    //    of three outcomes.
    let probe_skill = client.maestro.session_writability_skill(session);
    let probe = client.execute_skill(&probe_skill, None).map_err(|e| {
        VirtuosoError::Execution(format!(
            "maestro job submit: the writability probe failed for session {session:?}: {e}; \
             refusing to submit"
        ))
    })?;
    if !probe.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "maestro job submit: the writability probe for session {session:?} returned no \
             result; refusing to submit"
        )));
    }
    check_writable(session, probe.output_unquoted())?;

    // 2. Client identity. `resolve_client_id()` is the authoritative
    //    resolution — the raw `VB_PROFILE` is only one of its inputs and is
    //    never compared on its own.
    let current_client_id = resolve_client_id();

    // 2a. The client id the session itself carries, when it has one.
    let session_client_id = read_session_client_id(&client, session);

    // 2b. The client ids of the jobs already recorded for this session. The
    //     store directory is shared by every profile, so those records are
    //     the only record of who owns the session.
    let recorded_client_ids: Vec<String> = store
        .find_active_by_session(session)?
        .into_iter()
        .filter_map(|j| j.client_id)
        .collect();
    check_client_ids(
        &current_client_id,
        session_client_id.as_deref(),
        &recorded_client_ids,
    )?;

    // 3. Submit. This is the only side-effecting call and it is never
    //    retried. The run id is read from the submit itself.
    let submit_skill = client.maestro.submit_skill(session);
    let (run_id, pid) = match client.execute_skill(&submit_skill, None) {
        Ok(r) => {
            let (run_id, pid) = parse_submit_output(r.output_unquoted());
            match run_id {
                Some(rid) => (Some(rid), pid),
                None => {
                    // No run id came back, so no run was started under an id
                    // this command could record. The attempt is still
                    // recorded: the operator has to be able to see it.
                    let job = record_unknown_submission(
                        &store,
                        session,
                        test,
                        name,
                        "maeRunSimulation returned no run id",
                    );
                    return Err(VirtuosoError::Execution(format!(
                        "maestro job submit: maeRunSimulation returned no run id for session \
                         {session:?}; job_id={} recorded with status=unknown",
                        job.job_id
                    )));
                }
            }
        }
        Err(e) => {
            // The transport broke: the submit may or may not have reached
            // Maestro. The attempt is recorded either way, so a simulation
            // that did start is never invisible.
            let job = record_unknown_submission(
                &store,
                session,
                test,
                name,
                &format!("the submit outcome is unknown: {e}"),
            );
            return Err(VirtuosoError::Execution(format!(
                "maestro job submit: the submit outcome is unknown ({e}); job_id={} recorded \
                 with status=unknown — poll it with maestro job status",
                job.job_id
            )));
        }
    };

    // 4. The run directory, needed for status polling.
    let run_dir = read_run_dir(&client, session);

    // 5. Record the job. The Spectre child may already exist, so a record
    //    MUST exist for this run — including when the run id belongs to a
    //    job another submitter already owns.
    let mut job = MaestroJob::new(
        session.to_string(),
        test.map(String::from),
        name.map(String::from),
    );
    job.run_id = run_id.clone();
    job.pid = pid;
    job.run_dir = run_dir;
    job.status = MaestroJobStatus::Running;
    job.updated_at = Utc::now();

    let (job, duplicate) = match record_submitted_job(&store, job.clone()) {
        Ok(v) => v,
        Err(e) => {
            // Persistence failed after acknowledgement → Unknown, caller can
            // poll. The job id is reported so the operator can find it.
            job.status = MaestroJobStatus::Unknown;
            job.error_message = Some(format!("persistence failed after acknowledgement: {e}"));
            job.updated_at = Utc::now();
            let _ = store.save(&job); // best-effort
            return Err(VirtuosoError::Execution(format!(
                "maestro job submit: maeRunSimulation succeeded but persistence failed after \
                 acknowledgement; job_id={} recorded with status=unknown — poll it with maestro \
                 job status",
                job.job_id
            )));
        }
    };

    Ok(json!({
        "job_id": job.job_id,
        "status": job.status,
        "run_id": job.run_id,
        "run_dir": job.run_dir,
        "pid": job.pid,
        "session": session,
        "test": test,
        "duplicate": duplicate.is_some(),
        "duplicate_of": duplicate.as_ref().map(|j| j.job_id.clone()),
    }))
}

/// Write the record for a submit that has already been issued.
///
/// A record ALWAYS exists once `maeRunSimulation` has been issued: the
/// Spectre child may exist, so a submit that cannot be recorded must still
/// leave a trace. When the run id turns out to belong to another job, the
/// record is written anyway — status `unknown`, with a note that says why —
/// so the duplicate is a visible, cancellable record instead of an orphan
/// simulation.
///
/// Returns the stored record and the job that already owns the run id, if
/// any.
fn record_submitted_job(
    store: &MaestroJobStore,
    mut job: MaestroJob,
) -> std::io::Result<(MaestroJob, Option<MaestroJob>)> {
    let duplicate = match job.run_id.as_ref() {
        Some(rid) => store.find_by_run_id(rid)?,
        None => None,
    };
    if let Some(existing) = &duplicate {
        // Another submitter already owns this run id. The simulation this
        // call started is not a separate run, so the record says exactly
        // that — but the record exists, so nothing is orphaned.
        job.status = MaestroJobStatus::Unknown;
        let rid = job.run_id.clone().unwrap_or_default();
        job.error_message = Some(format!(
            "duplicate run id {rid:?}: job {} already owns this run id. This record exists so \
             the simulation started by this call is not an orphan; it is not a separate run.",
            existing.job_id
        ));
    }
    store.save(&job)?;
    Ok((job, duplicate))
}

/// Read the run id and the pid out of the submit's answer.
///
/// The submit answers `(run_id pid)`. The run id is the id Maestro assigned
/// to THIS run, so the submit is the only place it can be read from — there
/// is no pre-submit probe for the id of a run that has not happened yet.
/// `"nil"` means the submit produced no run.
fn parse_submit_output(output: &str) -> (Option<String>, Option<u32>) {
    let tokens = parse_skill_list_top_level(output);
    let run_id = tokens
        .first()
        .and_then(|t| extract_skill_string_token(t))
        .filter(|s| s != "nil");
    let pid = tokens.get(1).and_then(|t| t.trim().parse::<u32>().ok());
    (run_id, pid)
}

/// Turn the writability probe's answer into a submit decision.
///
/// Only `"writable"` passes. `"readonly"` is refused with the fix in the
/// message. Anything else — including the `"unknown"` a missing session
/// produces, and the `"nil"` an older probe returned for both a writable and
/// a missing session — is refused as well: the gate fails closed, because
/// "I could not prove the session is writable" is not "the session is
/// writable".
fn check_writable(session: &str, probe: &str) -> Result<()> {
    match probe.trim().trim_matches('"') {
        "writable" => Ok(()),
        "readonly" => Err(VirtuosoError::Execution(format!(
            "maestro job submit: session {session:?} is read-only; open it with mode=a \
             (vcli maestro set-mode --session {session} --mode a) before submitting"
        ))),
        other => Err(VirtuosoError::Execution(format!(
            "maestro job submit: cannot show that session {session:?} is writable \
             (the probe said {other:?}); refusing to submit"
        ))),
    }
}

/// Check the client ids involved in a submit against this client's identity.
///
/// `current` is this client's resolved id (`resolve_client_id()`:
/// `VB_CLIENT_ID` → `VB_PROFILE` → hostname → `"default"`). The raw
/// `VB_PROFILE` is only one input to that resolution and is never compared
/// on its own — with `VB_CLIENT_ID=prod-abc` and `VB_PROFILE=prod` the
/// identity is `prod-abc`, and comparing `prod` against the session would
/// reject every submit.
///
/// `session_client_id` is the id the session carries, if it has one.
/// `recorded` are the client ids of the jobs already recorded for the
/// session — the store directory is shared by every profile, so those
/// records are the only record of who owns the session.
///
/// A mismatch is refused. The check is never skipped: the recorded ids are
/// always compared, and the session's own id is compared whenever the
/// session carries one.
fn check_client_ids(
    current: &str,
    session_client_id: Option<&str>,
    recorded: &[String],
) -> Result<()> {
    if let Some(session_id) = session_client_id {
        if session_id != current {
            return Err(VirtuosoError::Execution(format!(
                "maestro job submit: client id mismatch — the session was opened under client \
                 id {session_id:?} but this client is {current:?}"
            )));
        }
    }
    for recorded_id in recorded {
        if recorded_id != current {
            return Err(VirtuosoError::Execution(format!(
                "maestro job submit: client id mismatch — a job on this session was submitted \
                 under client id {recorded_id:?} but this client is {current:?}"
            )));
        }
    }
    Ok(())
}

/// Record a submit whose outcome could not be determined.
///
/// The Spectre child may exist even when the submit could not be confirmed,
/// so the attempt is written to the store: an invisible simulation is worse
/// than a record that says "unknown".
fn record_unknown_submission(
    store: &MaestroJobStore,
    session: &str,
    test: Option<&str>,
    name: Option<&str>,
    reason: &str,
) -> MaestroJob {
    let mut job = MaestroJob::new(
        session.to_string(),
        test.map(String::from),
        name.map(String::from),
    );
    job.status = MaestroJobStatus::Unknown;
    job.error_message = Some(reason.to_string());
    job.updated_at = Utc::now();
    let _ = store.save(&job);
    job
}

/// The run directory of the session, if the bridge reports one.
fn read_run_dir(client: &VirtuosoClient, session: &str) -> Option<String> {
    let skill = client.maestro.job_run_dir_skill(session);
    let r = client.execute_skill_unchecked(&skill, None).ok()?;
    let out = r.output_unquoted().trim();
    if out.is_empty() || out == "nil" {
        return None;
    }
    Some(out.to_string())
}

/// The client id the session carries, if it has one.
fn read_session_client_id(client: &VirtuosoClient, session: &str) -> Option<String> {
    let skill = client.maestro.session_client_id_skill(session);
    let r = client.execute_skill_unchecked(&skill, None).ok()?;
    let out = r.output_unquoted().trim();
    if out.is_empty() || out == "nil" {
        return None;
    }
    Some(out.to_string())
}

/// Open the transport used to ask the run host about a run directory.
///
/// Returns `None` when no transport can be opened (no remote configured, or
/// the backend refused). Status derivation then falls back to the local
/// filesystem and the persisted status.
fn open_run_host_transport() -> Option<Arc<dyn RemoteTransport>> {
    let cfg = crate::config::Config::from_env().ok()?;
    backend::open_transport(&cfg).ok()
}

/// Whether any of these jobs needs the run host to be asked.
///
/// A run directory that exists on this machine is inspected locally, so a
/// purely local setup never opens a transport.
fn needs_run_host(jobs: &[MaestroJob]) -> bool {
    jobs.iter().any(|j| {
        !MaestroJobStore::is_terminal(j.status)
            && j.run_dir.as_ref().is_some_and(|d| !Path::new(d).exists())
    })
}

/// The shell command that reports the state of run directories.
///
/// Emits one line per directory: `<run_dir> <status>`. The status is
/// `completed` when the canonical end-of-run artefact exists, `running`
/// while the psf directory is up but the log is not, `failed` when the run
/// died before the psf directory came up, and `unknown` when nothing is
/// reachable. The paths are single-quoted so a path with spaces, `;` or
/// `$()` cannot inject a second command.
fn run_dir_status_command(run_dirs: &[String]) -> String {
    let quoted: Vec<String> = run_dirs
        .iter()
        .map(|d| format!("'{}'", d.replace('\'', "'\\''")))
        .collect();
    format!(
        "for d in {}; do if [ -f \"$d/psf/spectre.out\" ]; then echo \"$d completed\"; \
         elif [ -d \"$d/psf\" ]; then echo \"$d running\"; \
         elif [ -f \"$d/spectre.out\" ]; then echo \"$d failed\"; \
         else echo \"$d unknown\"; fi; done",
        quoted.join(" ")
    )
}

/// Parse the output of [`run_dir_status_command`].
fn parse_run_dir_status(output: &str) -> HashMap<String, MaestroJobStatus> {
    let mut out = HashMap::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // A run directory may contain spaces; the status token never does,
        // so the split is at the LAST space.
        let Some((dir, token)) = line.rsplit_once(' ') else {
            continue;
        };
        out.insert(
            dir.to_string(),
            MaestroJobStore::status_from_token(token.trim()),
        );
    }
    out
}

/// Derive the effective status of every job.
///
/// A terminal status is sticky: it is returned unchanged, so a cancelled job
/// never comes back as completed.
///
/// A run directory that exists on this machine is inspected locally. A run
/// directory that does not exist here lives on the run host, so the host is
/// asked — the same transport `job_logs` uses. CIW is never contacted.
fn derive_statuses(
    jobs: &[MaestroJob],
    run_host: Option<&dyn RemoteTransport>,
) -> HashMap<String, MaestroJobStatus> {
    let mut effective: HashMap<String, MaestroJobStatus> = HashMap::new();
    let mut remote_dirs: Vec<String> = Vec::new();

    for job in jobs {
        let status = if MaestroJobStore::is_terminal(job.status) {
            job.status
        } else {
            match job.run_dir.as_ref() {
                None => job.status,
                Some(dir) => {
                    if Path::new(dir).exists() || run_host.is_none() {
                        MaestroJobStore::derive_status_from_run_dir(job)
                    } else {
                        remote_dirs.push(dir.clone());
                        job.status
                    }
                }
            }
        };
        effective.insert(job.job_id.clone(), status);
    }

    if !remote_dirs.is_empty() {
        if let Some(ssh) = run_host {
            let cmd = run_dir_status_command(&remote_dirs);
            if let Ok(result) = ssh.run_command(&CommandRequest::untimed(&cmd)) {
                let parsed = parse_run_dir_status(&result.stdout);
                for job in jobs {
                    let Some(dir) = job.run_dir.as_ref() else {
                        continue;
                    };
                    if let Some(status) = parsed.get(dir) {
                        effective.insert(job.job_id.clone(), *status);
                    }
                }
            }
        }
    }
    effective
}

/// Get the current status of a Maestro job.
///
/// The status is derived from the run directory: locally when the run
/// directory is on this machine, and by asking the run host when it is not.
/// CIW is never contacted. A terminal status is sticky and is written back
/// to the store the first time it is observed, so the `--status` filter in
/// `job_list` sees it too.
pub fn job_status(job_id: &str) -> Result<Value> {
    let store = MaestroJobStore::new()?;
    let job = store
        .load(job_id)?
        .ok_or_else(|| VirtuosoError::NotFound(format!("job not found: {job_id}")))?;
    let run_host = if needs_run_host(std::slice::from_ref(&job)) {
        open_run_host_transport()
    } else {
        None
    };
    job_status_in(&store, job_id, run_host.as_deref())
}

/// The body of [`job_status`], with the store and the run host injected.
fn job_status_in(
    store: &MaestroJobStore,
    job_id: &str,
    run_host: Option<&dyn RemoteTransport>,
) -> Result<Value> {
    let mut job = store
        .load(job_id)?
        .ok_or_else(|| VirtuosoError::NotFound(format!("job not found: {job_id}")))?;

    let effective = derive_statuses(std::slice::from_ref(&job), run_host);
    let status = effective.get(&job.job_id).copied().unwrap_or(job.status);

    // Persist a terminal status the first time it is observed.
    if MaestroJobStore::is_terminal(status) && !MaestroJobStore::is_terminal(job.status) {
        job.status = status;
        job.updated_at = Utc::now();
        let _ = store.save(&job);
    }

    Ok(json!({
        "job_id": job.job_id,
        "status": status,
        "run_id": job.run_id,
        "run_dir": job.run_dir,
        "pid": job.pid,
        "session": job.session,
        "test": job.test,
        "error_message": job.error_message,
        "created_at": job.created_at.to_rfc3339(),
        "updated_at": job.updated_at.to_rfc3339(),
    }))
}

/// List Maestro jobs, newest first.
///
/// The status of every row is the **derived** status — what the run host
/// reports about the run directory — and the `--status` filter runs on that
/// same status, so a filter and the rows it returns always agree. A
/// terminal status is persisted the first time it is observed.
///
/// Library/cell/view filtering was removed in round 2 — the conservative
/// stub used `run_dir` substring matching which produced false positives;
/// round 3 should parse `run_dir/maestro.sdb` (or equivalent) to get
/// reliable lib/cell/view for each row.
pub fn job_list(status_filter: Option<&str>) -> Result<Value> {
    let store = MaestroJobStore::new()?;
    let jobs = store.list()?;
    let run_host = if needs_run_host(&jobs) {
        open_run_host_transport()
    } else {
        None
    };
    job_list_in(&store, status_filter, run_host.as_deref())
}

/// The body of [`job_list`], with the store and the run host injected.
fn job_list_in(
    store: &MaestroJobStore,
    status_filter: Option<&str>,
    run_host: Option<&dyn RemoteTransport>,
) -> Result<Value> {
    let status = status_filter.and_then(|s| match s {
        "pending" => Some(MaestroJobStatus::Pending),
        "running" => Some(MaestroJobStatus::Running),
        "completed" => Some(MaestroJobStatus::Completed),
        "failed" => Some(MaestroJobStatus::Failed),
        "unknown" => Some(MaestroJobStatus::Unknown),
        "cancelled" => Some(MaestroJobStatus::Cancelled),
        _ => None,
    });
    let jobs = store.list()?;
    let effective = derive_statuses(&jobs, run_host);

    // Persist terminal states the first time they are observed, so the
    // stored record and the derived status agree from now on.
    for job in &jobs {
        let derived = effective.get(&job.job_id).copied().unwrap_or(job.status);
        if MaestroJobStore::is_terminal(derived) && !MaestroJobStore::is_terminal(job.status) {
            let mut updated = job.clone();
            updated.status = derived;
            updated.updated_at = Utc::now();
            let _ = store.save(&updated);
        }
    }

    // The filter runs on the same status the rows display.
    let rows = MaestroJobStore::filter_by_status(&jobs, &effective, status);
    let list: Vec<Value> = rows
        .iter()
        .map(|j| {
            let derived = effective.get(&j.job_id).copied().unwrap_or(j.status);
            json!({
                "job_id": j.job_id,
                "status": derived,
                "run_id": j.run_id,
                "run_dir": j.run_dir,
                "session": j.session,
                "test": j.test,
                "name": j.name,
                "created_at": j.created_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(json!({"jobs": list, "count": list.len()}))
}

/// Tail the simulation log for a Maestro job.
///
/// Reads `<run_dir>/psf/spectre.out` via RemoteTransport download_dir.
/// Falls back to local file if run_dir is local.
pub fn job_logs(job_id: &str, tail: Option<usize>) -> Result<Value> {
    let store = MaestroJobStore::new()?;
    let job = store
        .load(job_id)?
        .ok_or_else(|| VirtuosoError::NotFound(format!("job not found: {job_id}")))?;

    let run_dir = job
        .run_dir
        .as_ref()
        .ok_or_else(|| VirtuosoError::Execution("job has no run_dir set".into()))?;

    let n = tail.unwrap_or(100);

    // Remote paths are absolute paths on the compute host. We detect a local
    // path by whether it exists on the local filesystem; everything else is
    // treated as remote (subject to `RemoteTransport::run_command`).
    let local_path = std::path::Path::new(run_dir);
    if local_path.exists() {
        let log_path = local_path.join("psf").join("spectre.out");
        if !log_path.exists() {
            return Err(VirtuosoError::NotFound(format!("{}", log_path.display())));
        }
        let content = std::fs::read_to_string(&log_path)?;
        let lines: Vec<&str> = content.lines().rev().take(n).collect();
        let tail_lines: Vec<String> = lines.into_iter().rev().map(String::from).collect();
        return Ok(json!({
            "job_id": job_id,
            "run_dir": run_dir,
            "source": "local",
            "tail_lines": tail_lines,
            "count": tail_lines.len(),
        }));
    }

    // Remote run_dir — use RemoteTransport. The path comes from a persisted
    // job JSON written by `job_submit` after a Maestro-confirmed run, but
    // we still single-quote it for the shell so a malicious or unexpected
    // character (space, `;`, `&`, `$(...)`) cannot inject a second command.
    let cfg = crate::config::Config::from_env()
        .map_err(|e| VirtuosoError::Config(format!("Config::from_env: {e}")))?;
    let ssh: Arc<dyn RemoteTransport> = backend::open_transport(&cfg)
        .map_err(|e| VirtuosoError::Connection(format!("transport: {e}")))?;

    let remote_log = format!("{run_dir}/psf/spectre.out");
    // Single-quote the path and embed the integer `n` directly (already
    // validated upstream by clap as a usize; no need to quote).
    let tail_cmd = format!("tail -n {n} '{}'", remote_log.replace('\'', "'\\''"));
    let req = CommandRequest::untimed(&tail_cmd);
    let result = ssh
        .run_command(&req)
        .map_err(|e| VirtuosoError::Ssh(format!("remote tail: {e}")))?;

    let tail_lines: Vec<String> = result.stdout.lines().map(String::from).collect();

    Ok(json!({
        "job_id": job_id,
        "run_dir": run_dir,
        "source": "remote",
        "tail_lines": tail_lines,
        "count": tail_lines.len(),
    }))
}

/// Cancel a Maestro simulation job.
///
/// The cancel is **session-wide**: `maeCloseSession` closes the whole
/// Maestro session, so every other job in that session is terminated with
/// it and unsaved state in the session is lost. There is no per-run cancel
/// in this path, and the returned JSON says so.
///
/// A job that has already reached a terminal state is refused: there is
/// nothing left to cancel, and closing the session would terminate work
/// that has already finished.
///
/// Does NOT kill the Virtuoso process — Maestro kills its own Spectre
/// children.
pub fn job_cancel(job_id: &str) -> Result<Value> {
    let store = MaestroJobStore::new()?;
    let job = store
        .load(job_id)?
        .ok_or_else(|| VirtuosoError::NotFound(format!("job not found: {job_id}")))?;
    let run_host = if needs_run_host(std::slice::from_ref(&job)) {
        open_run_host_transport()
    } else {
        None
    };
    job_cancel_in(&store, job_id, run_host.as_deref())
}

/// The body of [`job_cancel`], with the store and the run host injected.
fn job_cancel_in(
    store: &MaestroJobStore,
    job_id: &str,
    run_host: Option<&dyn RemoteTransport>,
) -> Result<Value> {
    let mut job = store
        .load(job_id)?
        .ok_or_else(|| VirtuosoError::NotFound(format!("job not found: {job_id}")))?;

    // Derive the current status first: a job whose run has already finished
    // must not be "cancelled".
    let effective = derive_statuses(std::slice::from_ref(&job), run_host);
    let status = effective.get(&job.job_id).copied().unwrap_or(job.status);
    if MaestroJobStore::is_terminal(status) {
        return Err(VirtuosoError::Execution(format!(
            "maestro job cancel: job {job_id} already finished (status={status:?}); there is \
             nothing to cancel"
        )));
    }

    // Close the session. `maeCloseSession` is session-wide: every other job
    // in the session dies with it and unsaved session state is lost.
    let client = VirtuosoClient::from_env()?;
    let skill = client.maestro.close_session(&job.session);
    let r = client.execute_skill(&skill, None)?;
    // maeCloseSession may return nil even on success; check bridge-level ok()
    if !r.ok() {
        return Err(VirtuosoError::Execution(format!(
            "maestro job cancel: maeCloseSession failed for session {}",
            job.session
        )));
    }
    // `maeCloseSession` answers `t` when it closed the session and `nil` when
    // there was no such session open — in that case the job cannot be
    // running either, so it is still marked cancelled.
    let session_closed = r.skill_ok();

    // Mark as Cancelled
    job.status = MaestroJobStatus::Cancelled;
    job.updated_at = Utc::now();
    store.save(&job)?;

    Ok(json!({
        "job_id": job.job_id,
        "status": "cancelled",
        "session": job.session,
        "scope": "session",
        "session_closed": session_closed,
        "warning": "maeCloseSession closes the WHOLE Maestro session: every other job in the \
                    session was terminated with it and unsaved session state was lost.",
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

    // --- Maestro --session classification ---------------------------------
    //
    // A Maestro command's --session may name either namespace. The three
    // cases below are the whole contract:
    //   1. bridge session id  → selects the Virtuoso instance (VB_SESSION)
    //   2. fnxSessionN        → Maestro session only, never VB_SESSION
    //   3. anything else      → hard error, never a silent guess

    #[test]
    fn session_arg_bridge_shape_selects_virtuoso() {
        assert_eq!(
            classify_session_arg("xunipc-userone-38371"),
            SessionArgKind::Bridge
        );
        // A host containing a dash must still classify as a bridge id.
        assert_eq!(
            classify_session_arg("my-host-userone-38371"),
            SessionArgKind::Bridge
        );
        // The id must be routed to VB_SESSION so the client connects to
        // exactly that Virtuoso instance.
        assert_eq!(
            resolve_maestro_session_env("xunipc-userone-38371"),
            Ok(Some("xunipc-userone-38371".to_string()))
        );
    }

    #[test]
    fn session_arg_fnx_session_is_maestro_only() {
        assert_eq!(classify_session_arg("fnxSession0"), SessionArgKind::Maestro);
        assert_eq!(classify_session_arg("fnxSession4"), SessionArgKind::Maestro);
        // A Maestro session name must NEVER reach VB_SESSION: the bridge
        // client would look for a session file that does not exist and fall
        // back to VB_PORT=0 (ECONNREFUSED).
        assert_eq!(resolve_maestro_session_env("fnxSession0"), Ok(None));
        assert_eq!(resolve_maestro_session_env("fnxSession4"), Ok(None));
    }

    #[test]
    fn session_arg_unknown_is_rejected_with_actionable_message() {
        assert_eq!(classify_session_arg("mystery"), SessionArgKind::Unknown);
        let err = resolve_maestro_session_env("mystery").unwrap_err();
        // The message must point at VB_SESSION for the Virtuoso instance and
        // at the subcommand's own --session for the Maestro session.
        assert!(err.contains("VB_SESSION"), "{err}");
        assert!(err.contains("fnxSession"), "{err}");
        // ...and it must never tell the user to "use --session" for the
        // Virtuoso instance, which would be the contradictory advice the
        // multi-session error used to give.
        assert!(!err.contains("Use --session"), "{err}");
    }

    #[test]
    fn session_arg_empty_or_partial_is_unknown() {
        assert_eq!(classify_session_arg(""), SessionArgKind::Unknown);
        assert_eq!(classify_session_arg("   "), SessionArgKind::Unknown);
        // Two dash-separated fields is not enough: only <host>-<user>-<port>
        // is a bridge session id.
        assert_eq!(
            classify_session_arg("userone-38371"),
            SessionArgKind::Unknown
        );
        // A port of 0 is never a real bridge port.
        assert_eq!(
            classify_session_arg("xunipc-userone-0"),
            SessionArgKind::Unknown
        );
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

    // ── failure-path diagnostics for create_corner_netlist ───────────
    //
    // `maeCreateNetlistForCorner` collapses every failure into a bare `nil`.
    // Both helpers below add context on that path, and both must degrade to a
    // no-op rather than eat the original error when they cannot.

    #[test]
    fn netlister_log_is_appended_to_the_failure() {
        use crate::transport::contract::test_support::FakeTransport;
        let mut t = FakeTransport::ok();
        t.command_result.stdout =
            "==> /tmp/d/si.foregnd.log <==\nERROR (SFE-30): cannot find model file\n".into();
        let e = with_netlister_log(VirtuosoError::Execution("nil".into()), &t, "/tmp/d");
        let msg = e.to_string();
        assert!(msg.contains("nil"), "original failure survives: {msg}");
        assert!(msg.contains("SFE-30"), "log tail is carried back: {msg}");
        assert!(msg.contains("/tmp/d"), "says where it read from: {msg}");
        // And it must have asked for logs, not listed the whole dir.
        let cmds = t.commands.lock().unwrap();
        assert!(cmds[0].contains("*.log"), "{cmds:?}");
    }

    #[test]
    fn no_log_leaves_the_original_failure_untouched() {
        use crate::transport::contract::test_support::FakeTransport;
        let mut t = FakeTransport::ok();
        t.command_result.stdout = "   \n".into();
        let e = with_netlister_log(VirtuosoError::Execution("nil".into()), &t, "/tmp/d");
        assert_eq!(
            e.to_string(),
            VirtuosoError::Execution("nil".into()).to_string()
        );
    }

    /// The diagnostic runs against a machine that just failed something. If the
    /// readback itself fails, the caller must still see the real error.
    #[test]
    fn a_failing_readback_does_not_replace_the_failure_it_diagnoses() {
        use crate::transport::contract::test_support::FakeTransport;
        let mut t = FakeTransport::ok();
        t.deadline_expired = true;
        let e = with_netlister_log(VirtuosoError::Execution("nil".into()), &t, "/tmp/d");
        assert_eq!(
            e.to_string(),
            VirtuosoError::Execution("nil".into()).to_string()
        );
    }

    #[test]
    fn corner_hint_names_the_corners_that_would_have_worked() {
        let h = corner_hint("tt", r#"["C0","Nominal"]"#);
        assert!(h.contains("\"tt\""), "names the rejected corner: {h}");
        assert!(
            h.contains("C0") && h.contains("Nominal"),
            "lists the real ones: {h}"
        );
        assert!(
            h.contains("not PDK model sections"),
            "must explain why tt looked right: {h}"
        );
    }

    /// An empty list is a different diagnosis: no name would have worked, so
    /// telling the user to pick a better one would send them in a circle.
    #[test]
    fn corner_hint_says_so_when_the_setup_has_no_corners() {
        for empty in ["[]", "  ", ""] {
            let h = corner_hint("tt", empty);
            assert!(h.contains("no corners at all"), "{empty:?} -> {h}");
        }
    }

    /// `Display` already prints "execution failed: ", so a diagnostic that
    /// re-wraps with `{err}` prints it twice and flattens the variant with it.
    /// Live output caught exactly that before this helper existed.
    #[test]
    fn a_diagnostic_keeps_the_variant_and_does_not_double_the_prefix() {
        let e = append_diagnostic(VirtuosoError::Execution("nil".into()), "hint");
        assert!(
            matches!(e, VirtuosoError::Execution(_)),
            "variant preserved"
        );
        let msg = e.to_string();
        assert_eq!(
            msg.matches("execution failed").count(),
            1,
            "prefix must appear once: {msg}"
        );
        assert!(msg.ends_with("nil\nhint"), "{msg}");

        // Ssh keeps its own variant — the exit code depends on it.
        let e = append_diagnostic(VirtuosoError::Ssh("denied".into()), "hint");
        assert!(matches!(e, VirtuosoError::Ssh(_)), "{e}");
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
    #[allow(dead_code)] // some feature/test combinations use inline literals instead
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

    // ========================================================================
    // Persistent Maestro jobs
    // ========================================================================

    use crate::transport::contract::{
        CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest, TransportError,
        UploadFileRequest, UploadTextRequest,
    };

    /// A fake run host: answers the status probe with a canned reply and
    /// records the commands it was asked to run.
    struct FakeRunHost {
        stdout: String,
        commands: std::sync::Mutex<Vec<String>>,
    }

    impl FakeRunHost {
        fn new(stdout: &str) -> Self {
            Self {
                stdout: stdout.to_string(),
                commands: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn commands(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    impl RemoteTransport for FakeRunHost {
        fn test_connection(
            &self,
            _deadline: Deadline,
        ) -> std::result::Result<bool, TransportError> {
            Ok(true)
        }

        fn run_command(
            &self,
            req: &CommandRequest,
        ) -> std::result::Result<CommandResult, TransportError> {
            self.commands.lock().unwrap().push(req.command.clone());
            Ok(CommandResult {
                exit_status: 0,
                stdout: self.stdout.clone(),
                stderr: String::new(),
                success: true,
                duration: std::time::Duration::from_millis(1),
            })
        }

        fn upload_file(&self, _req: &UploadFileRequest) -> std::result::Result<(), TransportError> {
            Err(TransportError::UnsupportedOperation("upload_file".into()))
        }

        fn upload_text(&self, _req: &UploadTextRequest) -> std::result::Result<(), TransportError> {
            Err(TransportError::UnsupportedOperation("upload_text".into()))
        }

        fn download_file(
            &self,
            _req: &DownloadFileRequest,
        ) -> std::result::Result<(), TransportError> {
            Err(TransportError::UnsupportedOperation("download_file".into()))
        }

        fn download_dir(
            &self,
            _req: &DownloadDirRequest,
        ) -> std::result::Result<(), TransportError> {
            Err(TransportError::UnsupportedOperation("download_dir".into()))
        }
    }

    fn temp_store() -> (tempfile::TempDir, MaestroJobStore) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore::with_dir(dir).unwrap();
        (tmp, store)
    }

    // ---- B3: the read-only gate fails closed ------------------------

    #[test]
    fn writable_probe_answer_is_the_only_one_that_passes() {
        assert!(
            check_writable("fnxSession4", "\"writable\"").is_ok(),
            "an explicit writable must pass"
        );
        assert!(
            check_writable("fnxSession4", "writable").is_ok(),
            "the unquoted answer must pass too"
        );
    }

    #[test]
    fn readonly_probe_answer_is_refused() {
        let err = check_writable("fnxSession4", "\"readonly\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("read-only"), "{msg}");
    }

    #[test]
    fn unprovable_probe_answer_is_refused() {
        // "unknown" is what a missing session answers, and "nil" is what the
        // old probe answered for BOTH a writable and a missing session. Neither
        // may let a submit through.
        for answer in ["\"unknown\"", "\"nil\"", "nil", "", "   "] {
            assert!(
                check_writable("fnxSession4", answer).is_err(),
                "answer {answer:?} must not let a submit through"
            );
        }
    }

    // ---- B4: the run id comes from the submit, and a submit always
    //      leaves a record behind ---------------------------------------

    #[test]
    fn submit_output_carries_the_run_id_and_the_pid() {
        let (run_id, pid) = parse_submit_output("(\"1234\" 4321)");
        assert_eq!(run_id.as_deref(), Some("1234"));
        assert_eq!(pid, Some(4321));
    }

    #[test]
    fn submit_without_a_run_id_yields_none() {
        let (run_id, _pid) = parse_submit_output("(\"nil\" 4321)");
        assert!(run_id.is_none(), "a nil run id must not be recorded");
    }

    #[test]
    fn a_duplicate_run_id_still_leaves_a_record() {
        let (_tmp, store) = temp_store();

        // The first submitter owns the run id.
        let mut owner = MaestroJob::new("fnxSession4".into(), None, None);
        owner.run_id = Some("run-1".into());
        owner.status = MaestroJobStatus::Running;
        store.save(&owner).unwrap();

        // The second submitter is handed the same run id. Its simulation may
        // exist, so a record MUST exist for it — otherwise it is an orphan
        // that no status, list or cancel can see.
        let mut second = MaestroJob::new("fnxSession4".into(), None, None);
        second.run_id = Some("run-1".into());
        second.status = MaestroJobStatus::Running;
        let (recorded, duplicate) = record_submitted_job(&store, second).unwrap();

        let stored = store.list().unwrap();
        assert_eq!(
            stored.len(),
            2,
            "the duplicate submit must leave a record of its own"
        );
        assert_eq!(duplicate.map(|j| j.job_id), Some(owner.job_id.clone()));
        assert!(
            !MaestroJobStore::is_terminal(recorded.status),
            "the duplicate record must stay cancellable, got {:?}",
            recorded.status
        );
        assert!(
            recorded
                .error_message
                .as_deref()
                .is_some_and(|m| m.contains("duplicate run id")),
            "the duplicate record must say why it exists: {:?}",
            recorded.error_message
        );
        // And the record is really on disk.
        assert!(store.load(&recorded.job_id).unwrap().is_some());
    }

    // ---- B5: the client-id gate --------------------------------------

    #[test]
    fn client_id_mismatch_against_the_session_is_refused() {
        let err = check_client_ids("prod-abc", Some("prod"), &[]).unwrap_err();
        assert!(err.to_string().contains("client id mismatch"), "{err}");
    }

    #[test]
    fn client_id_mismatch_against_a_recorded_job_is_refused() {
        let recorded = vec!["prod-abc".to_string()];
        let err = check_client_ids("prod", None, &recorded).unwrap_err();
        assert!(err.to_string().contains("client id mismatch"), "{err}");
    }

    #[test]
    fn matching_client_ids_pass() {
        assert!(check_client_ids("prod-abc", Some("prod-abc"), &[]).is_ok());
        let recorded = vec!["prod-abc".to_string()];
        assert!(check_client_ids("prod-abc", None, &recorded).is_ok());
    }

    #[test]
    fn client_id_gate_is_not_skipped_when_the_session_has_no_id() {
        // A session that carries no id must not switch the gate off: the ids
        // recorded for the session are still compared.
        let recorded = vec!["someone-else".to_string()];
        assert!(check_client_ids("prod", None, &recorded).is_err());
    }

    // ---- M1: the status comes from the run host -----------------------

    #[test]
    fn remote_status_is_derived_from_the_run_host() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Running;
        job.run_dir = Some("/remote/run/dir".into());
        store.save(&job).unwrap();

        let host = FakeRunHost::new("/remote/run/dir completed\n");
        let effective = derive_statuses(&store.list().unwrap(), Some(&host));
        assert_eq!(
            effective.get(&job.job_id),
            Some(&MaestroJobStatus::Completed),
            "a remote run directory must be resolved by asking the run host"
        );
        let commands = host.commands();
        assert_eq!(commands.len(), 1, "the run host must be asked once");
        assert!(
            commands[0].contains("/remote/run/dir"),
            "the probe must name the run directory: {}",
            commands[0]
        );
    }

    #[test]
    fn a_terminal_status_is_sticky_even_when_the_host_says_otherwise() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Cancelled;
        job.run_dir = Some("/remote/run/dir".into());
        store.save(&job).unwrap();

        let host = FakeRunHost::new("/remote/run/dir completed\n");
        let effective = derive_statuses(&store.list().unwrap(), Some(&host));
        assert_eq!(
            effective.get(&job.job_id),
            Some(&MaestroJobStatus::Cancelled),
            "a cancelled job must not come back as completed"
        );
        assert!(host.commands().is_empty(), "a terminal job needs no probe");
    }

    #[test]
    fn the_run_host_probe_is_single_quoted() {
        let cmd = run_dir_status_command(&["/remote/a dir;rm -rf x".to_string()]);
        assert!(
            cmd.contains("'/remote/a dir;rm -rf x'"),
            "the path must be quoted: {cmd}"
        );
        // The injected `;` must be inert. It is inert because it sits
        // inside the single quotes — so the property to check is that
        // nothing of the payload survives OUTSIDE the quoted literal, not
        // that the payload is absent (the quoted form ends with the very
        // bytes the payload is made of).
        let outside = cmd.replace("'/remote/a dir;rm -rf x'", "");
        assert!(
            !outside.contains("rm -rf"),
            "the payload must not escape the quotes: {cmd}"
        );
        assert!(
            !outside.contains("/remote/a dir"),
            "the path must not appear unquoted: {cmd}"
        );
    }

    #[test]
    fn run_dir_status_output_is_parsed_per_directory() {
        let parsed =
            parse_run_dir_status("/a/dir completed\n/a dir with spaces running\n/b/dir unknown\n");
        assert_eq!(parsed.get("/a/dir"), Some(&MaestroJobStatus::Completed));
        assert_eq!(
            parsed.get("/a dir with spaces"),
            Some(&MaestroJobStatus::Running)
        );
        assert_eq!(parsed.get("/b/dir"), Some(&MaestroJobStatus::Unknown));
    }

    // ---- M2/M3: the filter and the displayed status agree -------------

    #[test]
    fn list_filter_matches_the_displayed_status() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Running;
        job.run_dir = Some("/remote/run/dir".into());
        store.save(&job).unwrap();

        let host = FakeRunHost::new("/remote/run/dir completed\n");
        let listed = job_list_in(&store, Some("completed"), Some(&host)).unwrap();
        assert_eq!(listed["count"], 1, "{listed}");
        assert_eq!(listed["jobs"][0]["status"], "completed", "{listed}");
    }

    #[test]
    fn list_persists_a_terminal_status_the_first_time_it_is_seen() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Running;
        job.run_dir = Some("/remote/run/dir".into());
        store.save(&job).unwrap();

        let host = FakeRunHost::new("/remote/run/dir completed\n");
        job_list_in(&store, None, Some(&host)).unwrap();
        let reloaded = store.load(&job.job_id).unwrap().unwrap();
        assert_eq!(
            reloaded.status,
            MaestroJobStatus::Completed,
            "the terminal status must be written back to the store"
        );
    }

    #[test]
    fn status_reports_the_derived_status_and_persists_it() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Running;
        job.run_dir = Some("/remote/run/dir".into());
        store.save(&job).unwrap();

        let host = FakeRunHost::new("/remote/run/dir failed\n");
        let status = job_status_in(&store, &job.job_id, Some(&host)).unwrap();
        assert_eq!(status["status"], "failed", "{status}");
        assert_eq!(
            store.load(&job.job_id).unwrap().unwrap().status,
            MaestroJobStatus::Failed
        );
    }

    // ---- M4: cancelling a finished job is refused ----------------------

    #[test]
    fn cancel_refuses_a_job_that_already_finished() {
        let (_tmp, store) = temp_store();
        let mut job = MaestroJob::new("fnxSession4".into(), None, None);
        job.status = MaestroJobStatus::Completed;
        store.save(&job).unwrap();

        let err = job_cancel_in(&store, &job.job_id, None).unwrap_err();
        assert!(err.to_string().contains("already finished"), "{err}");
        // The record is untouched: the job did not become "cancelled".
        assert_eq!(
            store.load(&job.job_id).unwrap().unwrap().status,
            MaestroJobStatus::Completed
        );
    }
}

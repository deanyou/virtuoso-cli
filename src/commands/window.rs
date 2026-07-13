use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use regex::Regex;
use serde_json::{json, Value};

/// Window-kind tag — mirrors virtuoso-bridge-lite's snapshot classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowKind {
    Maestro,
    Schematic,
    Layout,
    Waveform,
    Hierarchy,
    Ciw,
    Unknown,
}

impl WindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Maestro => "maestro",
            Self::Schematic => "schematic",
            Self::Layout => "layout",
            Self::Waveform => "waveform",
            Self::Hierarchy => "hierarchy",
            Self::Ciw => "ciw",
            Self::Unknown => "unknown",
        }
    }
}

/// Classify a Virtuoso window title into a kind tag.
///
/// This is a pure regex classifier — no I/O, no state.  It is exposed so that
/// `virtuoso windows` CLI output can be colorized by kind and so that future
/// snapshot aggregators can dispatch on kind without re-parsing.
///
/// Classification order matters: the first matching pattern wins.
/// ADE windows are the most specific (full title structure), then editors,
/// then generic tool windows.
///
/// # Examples
///
/// ```
/// use virtuoso_cli::commands::window::classify_window;
///
/// assert_eq!(classify_window("ADE Assembler Editing: LIB CELL maestro").as_str(), "maestro");
/// assert_eq!(classify_window("Virtuoso Schematic Editor").as_str(), "schematic");
/// assert_eq!(classify_window("Visualization & Analysis").as_str(), "waveform");
/// assert_eq!(classify_window("").as_str(), "unknown");
/// ```
pub fn classify_window(title: &str) -> WindowKind {
    if title.is_empty() {
        return WindowKind::Unknown;
    }

    for (regex, kind) in PATTERNS.iter() {
        if regex.is_match(title) {
            return *kind;
        }
    }

    WindowKind::Unknown
}

// Minimal lazy-compiled regex wrapper — avoids recomputing the regex on every call.
lazy_static::lazy_static! {
    static ref PATTERNS: Vec<(Regex, WindowKind)> = vec![
        // ADE Assembler/Explorer Editing/Reading — distinguish maestro vs schematic
        // by the trailing VIEW token (maestro ends with "maestro", schematic with "schematic")
        (
            Regex::new(
                r"ADE\s+(?:Assembler|Explorer)\s+(?:Editing|Reading):\s+\S+\s+\S+\s+maestro\b",
            )
            .expect("valid regex"),
            WindowKind::Maestro,
        ),
        (
            Regex::new(
                r"ADE\s+(?:Assembler|Explorer)\s+(?:Editing|Reading):\s+\S+\s+\S+\s+schematic\b",
            )
            .expect("valid regex"),
            WindowKind::Schematic,
        ),
        // Generic schematic editor (no ADE prefix)
        (Regex::new(r"Schematic Editor").expect("valid regex"), WindowKind::Schematic),
        // Layout editor
        (Regex::new(r"Layout Suite").expect("valid regex"), WindowKind::Layout),
        // Waveform windows — two variants
        (
            Regex::new(r"Visualization\s*&?\s*Analysis").expect("valid regex"),
            WindowKind::Waveform,
        ),
        (
            Regex::new(r"Waveform Window").expect("valid regex"),
            WindowKind::Waveform,
        ),
        // Hierarchy browser
        (
            Regex::new(r"Cadence Hierarchy Editor").expect("valid regex"),
            WindowKind::Hierarchy,
        ),
        // CIW / Log window — Virtuoso® 23.1.0 - Log: or Virtuoso - Log:
        (
            Regex::new(r"Virtuoso®?\s+[\d.\-a-z]+\s*-\s*Log:").expect("valid regex"),
            WindowKind::Ciw,
        ),
    ];
}

/// List all open Virtuoso windows with their names.
///
/// Window names reveal the current mode, e.g.:
///   "ADE Explorer Editing: LIB/CELL/maestro"
///   "ADE Explorer Reading: ..."
///   "Virtuoso Schematic Editor"
pub fn list() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let r = client.execute_skill(&client.window.list_windows(), None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "failed to list windows: {}",
            r.errors.join("; ")
        )));
    }
    let windows = parse_window_json(&r.output);
    // Annotate each window with a derived mode field
    let windows = annotate_modes(windows);
    Ok(json!({ "windows": windows }))
}

/// Dismiss the currently active blocking dialog.
///
/// With --dry-run, reports the dialog name without clicking anything.
/// action "cancel" (default): clicks Cancel / closes dialog.
/// action "ok": attempts hiSendOK — may not be supported by all dialog types.
///
/// When `use_x11` is true, the call goes through the X11 SSH bypass
/// (`transport::x11::dismiss`) instead of SKILL. This is the only path
/// that works when a modal has deadlocked the SKILL channel itself.
pub fn dismiss_dialog(action: &str, dry_run: bool) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    if dry_run {
        let r = client.execute_skill(&client.window.get_dialog_info(), None)?;
        let raw = r.output.trim_matches('"');
        let active = r.skill_ok() && raw != "no-dialog";
        return Ok(json!({
            "dialog": if active { raw } else { "none" },
            "active": active,
            "dry_run": true,
        }));
    }
    let r = client.execute_skill(&client.window.dismiss_dialog(action), None)?;
    let dismissed = r.skill_ok() && r.output.trim_matches('"') != "no-dialog";
    Ok(json!({
        "status": if dismissed { "dismissed" } else { "no-dialog" },
        "action": action,
    }))
}

/// List blocking dialog(s) via the X11 SSH bypass (no keypress sent).
///
/// Useful as a "dry-run" alternative when SKILL is deadlocked and you want
/// to see what dialogs are present before deciding which action to send.
pub fn list_dialogs_x11(explicit_display: Option<&str>) -> Result<Value> {
    use crate::config::Config;
    use crate::transport::x11;

    let config = Config::from_env()?;
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is not set; X11 bypass requires a remote SSH target".into(),
        ));
    }
    let runner = x11::runner_for_config(&config)?;
    let user = config.remote_user.as_deref();
    let (env, dialogs) = x11::list_dialogs(
        &runner,
        &x11::client_id_for(&config),
        user,
        explicit_display,
    )?;
    Ok(json!({
        "display": env.display,
        "xauthority": env.xauthority,
        "dialogs": dialogs,
        "count": dialogs.len(),
    }))
}

/// Dismiss blocking dialog(s) via the X11 SSH bypass.
///
/// This is the deadlock-resistant alternative to `dismiss_dialog`: it
/// SSHes into the same host Virtuoso is running on, finds modal dialogs
/// with `xwininfo`, and sends keypresses (`enter`/`escape`/`alt-y`/`alt-n`)
/// via `XTest`. The SKILL channel cannot be used when a modal has stalled
/// the CIW, so this path is independent of the SKILL bridge.
///
/// Adopted from <https://github.com/Arcadia-1/virtuoso-bridge-lite>
/// (MIT, 2026-05; helper vendored in resources/x11_dismiss_dialog.py).
pub fn dismiss_dialog_x11(
    action: &str,
    dry_run: bool,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::config::Config;
    use crate::transport::x11;

    if !["enter", "escape", "alt-y", "alt-n"].contains(&action) {
        return Err(VirtuosoError::Config(format!(
            "invalid --action '{}': must be one of enter|escape|alt-y|alt-n",
            action
        )));
    }
    let config = Config::from_env()?;
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is not set; X11 bypass requires a remote SSH target (or use the SKILL path without --x11)".into(),
        ));
    }
    let runner = x11::runner_for_config(&config)?;
    let user = config.remote_user.as_deref();
    let result = x11::dismiss(
        &runner,
        &x11::client_id_for(&config),
        user,
        explicit_display,
        action,
        dry_run,
    )?;
    let mut out = serde_json::to_value(&result)
        .map_err(|e| VirtuosoError::Execution(format!("failed to serialize X11 result: {e}")))?;
    // Ensure the top-level "status" field is set for parity with the SKILL path.
    let obj = out.as_object_mut().unwrap();
    let n_found = result.found.len();
    let n_dismissed = result.dismissed.len();
    obj.insert(
        "status".into(),
        json!(if n_dismissed > 0 {
            "dismissed"
        } else if n_found > 0 {
            "found"
        } else {
            "no-dialog"
        }),
    );
    obj.insert("action".into(), json!(action));
    obj.insert("dry_run".into(), json!(dry_run));
    Ok(out)
}

/// List every Virtuoso-related X11 window (no keypress sent).
///
/// Unlike `list_dialogs_x11` (which only returns dialogs matching the
/// geometric modal test), this enumerates every mapped Virtuoso window
/// along with its WM frame and dismiss id. Use this when you want to see
/// what's on screen before picking a target.
pub fn list_windows_x11(explicit_display: Option<&str>) -> Result<Value> {
    use crate::config::Config;
    use crate::transport::x11;

    let config = Config::from_env()?;
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is not set; X11 bypass requires a remote SSH target".into(),
        ));
    }
    let runner = x11::runner_for_config(&config)?;
    let user = config.remote_user.as_deref();
    let (env, windows) = x11::list_windows(
        &runner,
        &x11::client_id_for(&config),
        user,
        explicit_display,
    )?;
    Ok(json!({
        "display": env.display,
        "xauthority": env.xauthority,
        "windows": windows,
        "count": windows.len(),
    }))
}

/// Dismiss a SPECIFIC X11 window by id. Use `list_windows_x11` to find the id first.
///
/// The window id is the `dismiss_id` field returned by `--list-windows`
/// (typically an X resource id like `0x2e01f16`).
pub fn dismiss_window_x11(
    window_id: &str,
    action: &str,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::config::Config;
    use crate::transport::x11;

    let config = Config::from_env()?;
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is not set; X11 bypass requires a remote SSH target".into(),
        ));
    }
    let runner = x11::runner_for_config(&config)?;
    let user = config.remote_user.as_deref();
    let result = x11::dismiss_window(
        &runner,
        &x11::client_id_for(&config),
        user,
        explicit_display,
        window_id,
        action,
    )?;
    let mut out = serde_json::to_value(&result)
        .map_err(|e| VirtuosoError::Execution(format!("failed to serialize X11 result: {e}")))?;
    let obj = out.as_object_mut().unwrap();
    let n_dismissed = result.dismissed.len();
    obj.insert(
        "status".into(),
        json!(if n_dismissed > 0 {
            "dismissed"
        } else {
            "not-found"
        }),
    );
    obj.insert("action".into(), json!(action));
    obj.insert("window_id".into(), json!(window_id));
    Ok(out)
}

/// Capture a screenshot of the current (or pattern-matched) Virtuoso window.
///
/// Saves to --path as PNG. Requires IC23.1+ (hiGetWindowScreenDump).
pub fn screenshot(path: &str, window_pattern: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = match window_pattern {
        Some(pat) => client.window.screenshot_by_pattern(path, pat),
        None => client.window.screenshot(path),
    };
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        let detail = if r.output.is_empty() {
            r.errors.join("; ")
        } else {
            r.output.clone()
        };
        return Err(VirtuosoError::Execution(format!(
            "screenshot failed: {}",
            detail
        )));
    }
    if r.output.trim_matches('"') == "no-match" {
        return Err(VirtuosoError::NotFound(format!(
            "no window matching pattern '{}'",
            window_pattern.unwrap_or("")
        )));
    }
    Ok(json!({
        "status": "saved",
        "path": path,
    }))
}

/// Derive a mode string from a Virtuoso window name.
fn window_mode(name: &str) -> &'static str {
    if name.contains("ADE Explorer Editing") || name.contains("ADE Assembler Editing") {
        "ade-editing"
    } else if name.contains("ADE Explorer Reading") {
        "ade-reading"
    } else if name.contains("ADE") {
        "ade-other"
    } else if name.contains("Schematic Editor") {
        "schematic"
    } else if name.contains("Layout Editor") {
        "layout"
    } else {
        "other"
    }
}

/// Parse the JSON string returned by list_windows().
///
/// SKILL encodes non-ASCII chars as octal escapes (e.g. `\256` = ®).
/// Standard JSON parsers reject these, so we decode them first.
fn parse_window_json(output: &str) -> Value {
    // Strip surrounding SKILL string quotes
    let s = output.trim_matches('"');
    // Decode SKILL octal escapes (\NNN) → UTF-8, then un-escape \" and \\
    let decoded = decode_skill_octal(s);
    let unescaped = decoded.replace("\\\"", "\"").replace("\\\\", "\\");
    serde_json::from_str(&unescaped).unwrap_or_else(|_| json!([]))
}

/// Convert SKILL's `\NNN` octal escapes to their UTF-8 codepoints.
/// Leaves other backslash sequences untouched (they are handled later).
fn decode_skill_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            // Collect up to 3 octal digits
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && end < start + 3 && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(octal_str) = std::str::from_utf8(&bytes[start..end]) {
                if let Ok(n) = u32::from_str_radix(octal_str, 8) {
                    if let Some(c) = char::from_u32(n) {
                        out.push(c);
                        i = end;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn annotate_modes(v: Value) -> Value {
    match v {
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|mut item| {
                    if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                        let mode = window_mode(name).to_string();
                        let kind = classify_window(name);
                        if let Some(o) = item.as_object_mut() {
                            o.insert("mode".into(), json!(mode));
                            o.insert("kind".into(), json!(kind.as_str()));
                        }
                    }
                    item
                })
                .collect(),
        ),
        other => other,
    }
}

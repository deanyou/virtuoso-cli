//! X11 dialog dismissal via SSH bypass.
//!
//! When a modal dialog blocks the Virtuoso CIW, the SKILL channel is itself
//! stuck. `vcli window dismiss-dialog` (the SKILL path) can hang for the full
//! `VB_TIMEOUT` waiting for a SKILL reply that will never come. The X11
//! bypass SSHes into the same host the Virtuoso is running on, finds the
//! blocking modal via `xwininfo`, and sends a keypress to dismiss it.
//! The SKILL channel recovers once the modal closes.
//!
//! Adopted from
//! <https://github.com/Arcadia-1/virtuoso-bridge-lite/blob/main/src/virtuoso_bridge/resources/x11_dismiss_dialog.py>
//! (MIT, 2026-05).

use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::RemoteTaskResult;
use crate::transport::ssh::SSHRunner;
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

static RESOURCES: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources");

/// Display + xauthority detected from a running virtuoso process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X11Env {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xauthority: Option<String>,
}

/// One dialog (or non-dialog window) reported by the helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogInfo {
    pub window_id: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// One window from `--list-windows`. Includes both the WM frame and the
/// virt-class child that would receive the keypress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub frame_id: String,
    pub window_id: String,
    pub dismiss_id: String,
    pub title: String,
    #[serde(default)]
    pub class: Vec<String>,
    pub geometry: Geometry,
}

/// Window geometry (x, y, width, height in pixels).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Final result of a dismiss operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissResult {
    pub display: String,
    pub found: Vec<DialogInfo>,
    pub dismissed: Vec<DialogInfo>,
    pub errors: Vec<String>,
    /// Raw assistant stdout for debug (truncated to 8 KiB on the client).
    pub raw_log: String,
}

/// Remote dir leaf where we drop the helper script and any per-call scratch.
pub const X11_HELPER_NAME: &str = "x11_dismiss_dialog.py";
pub const X11_HELPER_SUBDIR: &str = "x11";

/// Build a stable, profile-isolated remote subdir for X11 helper artifacts.
pub fn x11_remote_dir(client_id: &str) -> String {
    format!(
        "/tmp/virtuoso_bridge/{}/{X11_HELPER_SUBDIR}",
        escape_remote_path(client_id)
    )
}

/// Derive a stable client_id from a Config. Mirrors the tunnel's
/// profile-isolated scratch dir so X11 artifacts and the SKILL daemon
/// land in sibling subdirs under `/tmp/virtuoso_bridge/`.
pub fn client_id_for(config: &Config) -> String {
    // setup_dir_for_profile returns "/tmp/<profiled_bridge_leaf>"; we want the leaf only.
    let dir = crate::transport::tunnel::setup_dir_for_profile(config.profile.as_deref());
    dir.trim_start_matches("/tmp/").to_string()
}

fn escape_remote_path(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Local path of the vendored Python helper. We always read from the
/// `resources/` tree embedded at build time so the binary is self-contained.
fn read_helper_source() -> Result<String> {
    let file = RESOURCES.get_file(X11_HELPER_NAME).ok_or_else(|| {
        VirtuosoError::Config(format!(
            "vendored {X11_HELPER_NAME} not found in resources/"
        ))
    })?;
    String::from_utf8(file.contents().to_vec())
        .map_err(|e| VirtuosoError::Config(format!("vendored {X11_HELPER_NAME} not utf8: {e}")))
}

fn hash_helper(source: &str) -> String {
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    let digest = h.finalize();
    // First 12 hex chars is enough for cache invalidation; we don't need cryptographic strength.
    let mut out = String::with_capacity(12);
    for b in digest.iter().take(6) {
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// Upload (or refresh) the helper. The remote path embeds a short hash of the
/// source so concurrent vcli versions don't overwrite each other.
pub fn ensure_helper_uploaded(runner: &SSHRunner, client_id: &str) -> Result<String> {
    let source = read_helper_source()?;
    let digest = hash_helper(&source);
    let remote_dir = x11_remote_dir(client_id);
    let remote_path = format!("{remote_dir}/x11_dismiss_dialog_{digest}.py");

    let mkdir = format!("mkdir -p {remote_dir}");
    let _ = runner.run_command(&mkdir, None)?;
    // Best-effort upload: if the file already exists with the same hash, the
    // hash-suffixed name avoids a write — but we still upload unconditionally
    // on the first call of a session to keep semantics simple. Idempotent.
    runner.upload_text(&source, &remote_path)?;
    Ok(remote_path)
}

/// Discover DISPLAY/XAUTHORITY from a running virtuoso process.
pub fn detect_env(runner: &SSHRunner, user: Option<&str>) -> Result<X11Env> {
    let user_filter = match user {
        Some(u) => format!("-u {u} "),
        None => "".to_string(),
    };
    // Use the pgrep pattern from the vendored helper so the contract matches.
    let cmd = format!(
        "pgrep {user_filter}-x virtuoso | head -1 | xargs -I{{}} sh -c 'tr \"\\0\" \"\\n\" </proc/{{}}/environ 2>/dev/null | grep -E \"^(DISPLAY|XAUTHORITY)=\"'"
    );
    let out = runner.run_command(&cmd, Some(10))?;
    let mut env = X11Env {
        display: None,
        xauthority: None,
    };
    for line in out.stdout.lines() {
        if let Some(v) = line.strip_prefix("DISPLAY=") {
            if !v.is_empty() {
                env.display = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("XAUTHORITY=") {
            if !v.is_empty() {
                env.xauthority = Some(v.to_string());
            }
        }
    }
    Ok(env)
}

/// Run the helper in detection-only mode (no dismiss).
pub fn list_dialogs(
    runner: &SSHRunner,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<(X11Env, Vec<DialogInfo>)> {
    let helper = ensure_helper_uploaded(runner, client_id)?;
    let env = match explicit_display {
        Some(d) => X11Env {
            display: Some(d.to_string()),
            xauthority: None,
        },
        None => detect_env(runner, user)?,
    };
    let display = env.display.clone().ok_or_else(|| {
        VirtuosoError::Config("cannot detect DISPLAY from virtuoso process".into())
    })?;
    let cmd = build_helper_cmd(&helper, &display, env.xauthority.as_deref(), false, "enter");
    let out = runner.run_command(&cmd, Some(30))?;
    // If the helper itself failed (e.g. xwininfo missing, libX11 not installed,
    // python not on PATH), surface the error so the user doesn't see an empty
    // list and assume "no dialogs". We attach a synthetic "no-dialog" entry so
    // the existing (env, Vec<DialogInfo>) signature stays unchanged.
    let mut dialogs = parse_helper_output(&out);
    let helper_errors = extract_helper_errors(&out);
    if dialogs.is_empty() && !helper_errors.is_empty() {
        for e in &helper_errors {
            dialogs.push(DialogInfo {
                window_id: "helper-error".into(),
                title: format!("x11 helper error: {e}"),
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            });
        }
    }
    Ok((env, dialogs))
}

/// Run the helper in dismiss mode.
pub fn dismiss(
    runner: &SSHRunner,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
    action: &str,
    dry_run: bool,
) -> Result<DismissResult> {
    let helper = ensure_helper_uploaded(runner, client_id)?;
    let env = match explicit_display {
        Some(d) => X11Env {
            display: Some(d.to_string()),
            xauthority: None,
        },
        None => detect_env(runner, user)?,
    };
    let display = env.display.clone().ok_or_else(|| {
        VirtuosoError::Config("cannot detect DISPLAY from virtuoso process".into())
    })?;
    let cmd = build_helper_cmd(
        &helper,
        &display,
        env.xauthority.as_deref(),
        !dry_run,
        action,
    );
    let out = runner.run_command(&cmd, Some(30))?;

    let mut found = Vec::new();
    let mut dismissed = Vec::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(d) = serde_json::from_str::<DialogInfo>(line) {
            // Dismiss records are emitted on stdout as {"dismissed":"...","action":"enter",...}
            // and look the same as a DialogInfo because they have window_id+x+y+w+h+title.
            // The helper places them AFTER the "found" list, and a dismiss record
            // has an extra "action" or "error" key. We split on that.
            if line.contains("\"action\"") || line.contains("\"error\"") {
                if let Some(rest) = line.split_once("\"action\"") {
                    // Re-parse as dismiss record by adding a dummy field.
                    if let Ok(rec) = serde_json::from_str::<DialogInfo>(&format!("{}}}", rest.0)) {
                        dismissed.push(rec);
                    }
                } else if let Ok(rec) = serde_json::from_str::<DialogInfo>(line) {
                    dismissed.push(rec);
                }
            } else {
                found.push(d);
            }
        }
        // Note: `{"error": "..."}` JSON lines are surfaced via extract_helper_errors
        // below; we don't double-count them here.
    }
    let errors = extract_helper_errors(&out);
    Ok(DismissResult {
        display,
        found,
        dismissed,
        errors,
        raw_log: truncate_log(&out),
    })
}

/// Enumerate Virtuoso-related X11 windows. No dismiss action.
pub fn list_windows(
    runner: &SSHRunner,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<(X11Env, Vec<WindowInfo>)> {
    let helper = ensure_helper_uploaded(runner, client_id)?;
    let (env, display) = resolve_env(runner, user, explicit_display)?;
    let cmd = build_helper_cmd_list_windows(&helper, &display, env.xauthority.as_deref());
    let out = runner.run_command(&cmd, Some(15))?;
    let windows: Vec<WindowInfo> = out
        .stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<WindowInfo>(l.trim()).ok())
        .collect();
    // If the helper died and produced no windows, surface the error so callers
    // can distinguish "no Virtuoso windows" from "x11 helper crashed".
    if windows.is_empty() {
        let errors = extract_helper_errors(&out);
        if !errors.is_empty() {
            return Err(VirtuosoError::Execution(format!(
                "x11 helper failed: {}",
                errors.join("; ")
            )));
        }
    }
    Ok((env, windows))
}

/// Dismiss a specific X11 window by id. Does NOT apply the dialog-size filter —
/// the caller is expected to have identified the target via `list_windows`
/// or by inspecting the X server directly.
pub fn dismiss_window(
    runner: &SSHRunner,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
    window_id: &str,
    action: &str,
) -> Result<DismissResult> {
    if !["enter", "escape", "alt-y", "alt-n"].contains(&action) {
        return Err(VirtuosoError::Config(format!(
            "invalid action '{action}': must be one of enter|escape|alt-y|alt-n"
        )));
    }
    if window_id.is_empty() {
        return Err(VirtuosoError::Config(
            "window_id is required for --dismiss-window".into(),
        ));
    }
    let helper = ensure_helper_uploaded(runner, client_id)?;
    let (env, display) = resolve_env(runner, user, explicit_display)?;
    let cmd = build_helper_cmd_dismiss_window(
        &helper,
        &display,
        env.xauthority.as_deref(),
        window_id,
        action,
    );
    let out = runner.run_command(&cmd, Some(15))?;

    // The helper emits exactly one JSON line for --dismiss-window: either
    // {"dismissed": "...", "action": "..."} or {"error": "..."}.
    let mut dismissed: Vec<DialogInfo> = Vec::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if val.get("error").is_some() {
                // Captured by extract_helper_errors below
                continue;
            }
            if val.get("dismissed").is_some() {
                dismissed.push(DialogInfo {
                    window_id: window_id.to_string(),
                    title: val
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                });
            }
        }
    }
    let errors = extract_helper_errors(&out);
    Ok(DismissResult {
        display,
        found: Vec::new(),
        dismissed,
        errors,
        raw_log: truncate_log(&out),
    })
}

/// Shared env-resolution: explicit display, else auto-detect from the running
/// virtuoso process. Returns the resolved env and the display string.
fn resolve_env(
    runner: &SSHRunner,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<(X11Env, String)> {
    let env = match explicit_display {
        Some(d) => X11Env {
            display: Some(d.to_string()),
            xauthority: None,
        },
        None => detect_env(runner, user)?,
    };
    let display = env.display.clone().ok_or_else(|| {
        VirtuosoError::Config("cannot detect DISPLAY from virtuoso process".into())
    })?;
    Ok((env, display))
}

fn build_helper_cmd(
    helper_remote_path: &str,
    display: &str,
    xauthority: Option<&str>,
    do_dismiss: bool,
    action: &str,
) -> String {
    // Quote the remote path with single quotes; the helper is ASCII so this is safe.
    let mut s = format!("python3 '{}'", helper_remote_path);
    s.push(' ');
    s.push_str(&shell_escape(display));
    if do_dismiss {
        s.push_str(" --dismiss");
        s.push_str(" --action ");
        s.push_str(action);
    }
    if let Some(xa) = xauthority {
        s.push_str(" XAUTHORITY=");
        s.push_str(xa);
    }
    s
}

/// Build `python3 <helper> <display> --list-windows` command.
fn build_helper_cmd_list_windows(
    helper_remote_path: &str,
    display: &str,
    xauthority: Option<&str>,
) -> String {
    let mut s = format!("python3 '{}'", helper_remote_path);
    s.push(' ');
    s.push_str(&shell_escape(display));
    s.push_str(" --list-windows");
    if let Some(xa) = xauthority {
        s.push_str(" XAUTHORITY=");
        s.push_str(xa);
    }
    s
}

/// Build `python3 <helper> <display> --dismiss-window <id> --action <a>` command.
fn build_helper_cmd_dismiss_window(
    helper_remote_path: &str,
    display: &str,
    xauthority: Option<&str>,
    window_id: &str,
    action: &str,
) -> String {
    let mut s = format!("python3 '{}'", helper_remote_path);
    s.push(' ');
    s.push_str(&shell_escape(display));
    s.push_str(" --dismiss-window ");
    s.push_str(&shell_escape(window_id));
    s.push_str(" --action ");
    s.push_str(action);
    if let Some(xa) = xauthority {
        s.push_str(" XAUTHORITY=");
        s.push_str(xa);
    }
    s
}

fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == ':' || c == '.' || c == '_' || c == '/' || c == '-'
    }) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn parse_helper_output(out: &RemoteTaskResult) -> Vec<DialogInfo> {
    let mut dialogs = Vec::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(d) = serde_json::from_str::<DialogInfo>(line) {
            dialogs.push(d);
        }
    }
    dialogs
}

/// Extract every failure signal from the helper's `RemoteTaskResult` so callers
/// can surface them in `DismissResult.errors` instead of silently seeing
/// "no dialogs" when the helper itself died.
///
/// Three independent sources of failure, in priority order:
/// 1. `{"error": "..."}` JSON lines on stdout (helper's structured error)
/// 2. Non-zero `returncode` (helper crashed, missing libX11, etc.)
/// 3. Non-empty `stderr` (helper printed to stderr without exit code)
fn extract_helper_errors(out: &RemoteTaskResult) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut errors: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let push = |s: String, errors: &mut Vec<String>, seen: &mut BTreeSet<String>| {
        if s.trim().is_empty() {
            return;
        }
        if seen.insert(s.clone()) {
            errors.push(s);
        }
    };

    // 1. Structured JSON errors on stdout.
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
                push(err.to_string(), &mut errors, &mut seen);
            }
        }
    }

    // 2. Non-zero returncode (no structured error AND nothing usable on stdout).
    if out.returncode != 0 && seen.is_empty() {
        let stderr_summary = out.stderr.lines().next().unwrap_or("").trim();
        let msg = if !stderr_summary.is_empty() {
            format!(
                "x11 helper exited with code {}: {}",
                out.returncode, stderr_summary
            )
        } else {
            format!("x11 helper exited with code {}", out.returncode)
        };
        push(msg, &mut errors, &mut seen);
    }

    // 3. Non-empty stderr with no other signal (some helpers print but don't exit non-zero).
    if seen.is_empty() {
        for line in out.stderr.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            push(line.to_string(), &mut errors, &mut seen);
        }
    }

    errors
}

fn truncate_log(out: &RemoteTaskResult) -> String {
    const LIMIT: usize = 8 * 1024;
    let mut log = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        out.stdout, out.stderr
    );
    if log.len() > LIMIT {
        log.truncate(LIMIT);
        log.push_str("\n[...truncated]");
    }
    log
}

/// Construct an SSHRunner for a given Config (mirrors `transport::tunnel`).
pub fn runner_for_config(config: &Config) -> Result<SSHRunner> {
    let runner = SSHRunner::from_config(config);
    if config.disable_control_master {
        if let Ok(mut guard) = runner.use_control_master.lock() {
            *guard = false;
        }
    }
    Ok(runner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_helper_is_stable_for_same_source() {
        let s = "print('hi')\n";
        assert_eq!(hash_helper(s), hash_helper(s));
    }

    #[test]
    fn hash_helper_differs_for_different_source() {
        assert_ne!(hash_helper("a"), hash_helper("b"));
    }

    #[test]
    fn hash_helper_is_12_hex_chars() {
        let h = hash_helper("anything");
        assert_eq!(h.len(), 12);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn remote_dir_is_profile_isolated() {
        let a = x11_remote_dir("default");
        let b = x11_remote_dir("proj/abc");
        assert!(a.contains("default"));
        assert!(b.contains("proj_abc"));
        assert!(a.starts_with("/tmp/virtuoso_bridge/"));
    }

    #[test]
    fn shell_escape_handles_punct() {
        assert_eq!(shell_escape("localhost:0.0"), "localhost:0.0");
        assert_eq!(shell_escape("a b"), "'a b'");
        assert_eq!(shell_escape("o'clock"), "'o'\\''clock'");
    }

    #[test]
    fn helper_source_is_embedded() {
        let s = read_helper_source().expect("vendored helper must be present");
        assert!(s.contains("X11 dialog finder and dismisser"));
        assert!(s.contains("def main"));
    }

    #[test]
    fn build_helper_cmd_quotes_path_and_keeps_action() {
        let cmd = build_helper_cmd(
            "/tmp/virtuoso_bridge/x/x11_dismiss_dialog_abc.py",
            ":0",
            None,
            true,
            "alt-n",
        );
        assert!(cmd.contains("'/tmp/virtuoso_bridge/x/x11_dismiss_dialog_abc.py'"));
        assert!(cmd.contains("--dismiss"));
        assert!(cmd.contains("--action alt-n"));
        assert!(!cmd.contains("XAUTHORITY="));
    }

    #[test]
    fn build_helper_cmd_propagates_xauthority_when_set() {
        let cmd = build_helper_cmd("/h.py", ":0", Some("/tmp/.X11-unix/X0"), false, "enter");
        assert!(cmd.contains("XAUTHORITY=/tmp/.X11-unix/X0"));
    }

    #[test]
    fn parse_helper_output_picks_json_dialogs_only() {
        let out = RemoteTaskResult {
            stdout: "noise\n{\"window_id\":\"0x1\",\"title\":\"a\",\"x\":0,\"y\":0,\"w\":1,\"h\":1}\nmore noise\n".to_string(),
            stderr: "".to_string(),
            success: true,
            returncode: 0,
            remote_dir: None,
            error: None,
            timings: Default::default(),
        };
        let dialogs = parse_helper_output(&out);
        assert_eq!(dialogs.len(), 1);
        assert_eq!(dialogs[0].window_id, "0x1");
    }

    fn mkresult(stdout: &str, stderr: &str, returncode: i32) -> RemoteTaskResult {
        RemoteTaskResult {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            success: returncode == 0,
            returncode,
            remote_dir: None,
            error: None,
            timings: Default::default(),
        }
    }

    #[test]
    fn extract_helper_errors_surfaces_json_error_lines() {
        let out = mkresult(
            "{\"error\": \"xwininfo not found\"}\n{\"error\": \"libX11 missing\"}\n",
            "",
            0,
        );
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 2);
        assert!(errs[0].contains("xwininfo not found"));
        assert!(errs[1].contains("libX11 missing"));
    }

    #[test]
    fn extract_helper_errors_summarizes_nonzero_returncode_with_stderr() {
        let out = mkresult("", "python3: command not found\n", 127);
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("127"));
        assert!(errs[0].contains("python3: command not found"));
    }

    #[test]
    fn extract_helper_errors_summarizes_nonzero_returncode_without_stderr() {
        let out = mkresult("", "", 1);
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("exited with code 1"));
    }

    #[test]
    fn extract_helper_errors_returns_stderr_when_no_other_signal() {
        // Helper printed warnings but exited cleanly — still surface them.
        let out = mkresult("", "warning: libXtst not found\n", 0);
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("libXtst not found"));
    }

    #[test]
    fn extract_helper_errors_dedupes_across_stderr_and_json() {
        // Same message from JSON line and stderr should appear only once.
        let out = mkresult(
            "{\"error\": \"xwininfo failed: not found\"}\n",
            "xwininfo failed: not found\n",
            0,
        );
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn extract_helper_errors_clean_run_returns_empty() {
        let out = mkresult(
            "{\"window_id\":\"0x1\",\"title\":\"a\",\"x\":0,\"y\":0,\"w\":1,\"h\":1}\n",
            "",
            0,
        );
        let errs = extract_helper_errors(&out);
        assert!(errs.is_empty());
    }

    #[test]
    fn truncate_log_caps_at_8k() {
        let huge = "x".repeat(20_000);
        let out = RemoteTaskResult {
            stdout: huge.clone(),
            stderr: "".into(),
            success: true,
            returncode: 0,
            remote_dir: None,
            error: None,
            timings: Default::default(),
        };
        let log = truncate_log(&out);
        assert!(log.len() <= 8 * 1024 + 32);
        assert!(log.ends_with("[...truncated]"));
    }

    #[test]
    fn build_helper_cmd_list_windows_includes_flag() {
        let cmd = build_helper_cmd_list_windows(
            "/tmp/virtuoso_bridge/x/x11_dismiss_dialog_abc.py",
            ":0",
            None,
        );
        assert!(cmd.contains("'/tmp/virtuoso_bridge/x/x11_dismiss_dialog_abc.py'"));
        assert!(cmd.contains("--list-windows"));
        assert!(!cmd.contains("--dismiss"));
        assert!(!cmd.contains("XAUTHORITY="));
    }

    #[test]
    fn build_helper_cmd_list_windows_propagates_xauthority() {
        let cmd = build_helper_cmd_list_windows("/h.py", ":0", Some("/tmp/.X11-unix/X0"));
        assert!(cmd.contains("XAUTHORITY=/tmp/.X11-unix/X0"));
    }

    #[test]
    fn build_helper_cmd_dismiss_window_includes_id_and_action() {
        let cmd = build_helper_cmd_dismiss_window("/h.py", ":0", None, "0x2e01f16", "escape");
        assert!(cmd.contains("--dismiss-window 0x2e01f16"));
        assert!(cmd.contains("--action escape"));
        assert!(!cmd.contains("--dismiss "));
    }

    #[test]
    fn build_helper_cmd_dismiss_window_quotes_window_id_with_spaces() {
        let cmd = build_helper_cmd_dismiss_window("/h.py", ":0", None, "0x a", "enter");
        assert!(cmd.contains("'0x a'"));
    }

    #[test]
    fn window_info_parses_helper_output_line() {
        let line = r#"{"frame_id":"0x400001","window_id":"0x400002","dismiss_id":"0x400002","title":"Save Changes","class":["virtuoso","VimClass"],"geometry":{"x":100,"y":200,"w":300,"h":100}}"#;
        let w: WindowInfo = serde_json::from_str(line).expect("parse");
        assert_eq!(w.frame_id, "0x400001");
        assert_eq!(w.window_id, "0x400002");
        assert_eq!(w.dismiss_id, "0x400002");
        assert_eq!(w.title, "Save Changes");
        assert_eq!(w.geometry.w, 300);
        assert_eq!(w.class, vec!["virtuoso", "VimClass"]);
    }
}

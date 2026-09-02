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
use crate::transport::contract::{
    CommandRequest, CommandResult, RemoteTransport, UploadTextRequest,
};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub still_mapped: Option<bool>,
}

/// One window from `--list-windows`. Includes both the WM frame and the
/// virt-class child that would receive the keypress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub frame_id: String,
    pub window_id: String,
    pub dismiss_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xauthority: Option<String>,
    pub title: String,
    #[serde(default)]
    pub class: Vec<String>,
    pub geometry: Geometry,
    /// _NET_WM_PID of the window; positive integers only. Zero is normalized
    /// to None (a window cannot have PID 0).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_pid_option"
    )]
    pub pid: Option<u32>,
    /// Whether the window is mapped/viewable (always true for listed windows).
    #[serde(default)]
    pub visible: bool,
}

/// Deserializer for `Option<u32>` that maps JSON integer 0 to `None`.
fn deserialize_pid_option<'de, D>(deserializer: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<u32>::deserialize(deserializer)?;
    Ok(opt.filter(|&p| p != 0))
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

/// Build a stable, user- and profile-isolated remote subdir for X11 helper artifacts.
///
/// Path format: `/tmp/virtuoso_bridge_<sanitized-user>/<sanitized-client>/x11`
///
/// Uses explicit user (from `remote_user` config or `-un` fallback) for user isolation,
/// and the client_id for client-level isolation. Both components are sanitized to
/// ascii-alphanumeric, underscore, and hyphen only.
#[allow(dead_code)]
pub fn x11_remote_dir(client_id: &str) -> String {
    // Default path structure without user isolation (kept for backward compat only)
    format!(
        "/tmp/virtuoso_bridge/{}/{X11_HELPER_SUBDIR}",
        escape_remote_path(client_id)
    )
}

/// Build a user-isolated scratch path for X11 helper artifacts.
///
/// `user` must be a non-empty sanitized username.
/// `client_id` is the profile/client identifier.
///
/// Path format: `/tmp/virtuoso_bridge_<sanitized-user>/<sanitized-client>/x11`
///
/// Both `user` and `client_id` are sanitized to prevent directory traversal.
/// Empty `client` falls back to "unnamed" to ensure the path remains valid.
/// Empty `user` is a programming error (caller must resolve via `id -un` first).
pub fn x11_remote_dir_with_user(user: &str, client_id: &str) -> String {
    let sanitized_user = sanitize_for_path(user);
    if sanitized_user.is_empty() {
        panic!("x11_remote_dir_with_user: empty user is not allowed — caller must resolve via `id -un`");
    }
    // Ensure we never produce an empty client component — fall back to "unnamed"
    let sanitized_client = sanitize_for_path(client_id);
    let sanitized_client = if sanitized_client.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized_client
    };

    format!(
        "/tmp/virtuoso_bridge_{}/{}/{X11_HELPER_SUBDIR}",
        sanitized_user, sanitized_client,
    )
}

/// Sanitize a string for use in a remote path component.
///
/// Allows only ascii-alphanumeric, underscore, and hyphen.
/// All other characters are replaced with underscore.
pub fn sanitize_for_path(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
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

/// Resolve the effective username via `id -un` on the remote host.
///
/// This is called when no explicit user is configured, to ensure the X11
/// scratch directory is isolated per real user rather than per-process.
pub fn resolve_effective_user(runner: &dyn RemoteTransport) -> Result<String> {
    let out = runner.run_command(&CommandRequest::with_exec_timeout(
        "id -un",
        Duration::from_secs(5),
    ))?;
    if !out.success {
        return Err(VirtuosoError::Execution(format!(
            "`id -un` failed with exit {}: {}",
            out.exit_status,
            out.stderr.trim()
        )));
    }
    let username = out.stdout.trim();
    if username.is_empty() {
        return Err(VirtuosoError::Execution(
            "`id -un` returned empty username".into(),
        ));
    }
    // Must be a single line (the username)
    if out.stdout.lines().count() != 1 {
        return Err(VirtuosoError::Execution(format!(
            "`id -un` returned unexpected multi-line output: {:?}",
            out.stdout
        )));
    }
    let sanitized = sanitize_for_path(username);
    if sanitized.is_empty() {
        return Err(VirtuosoError::Execution(format!(
            "`id -un` returned username that sanitizes to empty: {:?}",
            username
        )));
    }
    Ok(sanitized)
}

/// Derive a stable client_id from a Config. Mirrors the tunnel's
/// profile-isolated scratch dir so X11 artifacts and the SKILL daemon
/// land in sibling subdirs under `/tmp/virtuoso_bridge/`.
pub fn client_id_for(config: &Config) -> String {
    // setup_dir_for_profile returns "/tmp/<profiled_bridge_leaf>"; we want the leaf only.
    let dir = crate::transport::tunnel::setup_dir_for_profile(config.profile.as_deref());
    dir.trim_start_matches("/tmp/").to_string()
}

#[allow(dead_code)]
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
///
/// `explicit_user` is the configured remote user (from Config), or None.
/// When None, `id -un` is called on the remote host to resolve the effective user.
///
/// Directory is created with `install -d -m 700` for security.
pub fn ensure_helper_uploaded(
    runner: &dyn RemoteTransport,
    explicit_user: Option<&str>,
    client_id: &str,
) -> Result<String> {
    let source = read_helper_source()?;
    let digest = hash_helper(&source);

    // Resolve username: use explicit if non-empty, otherwise call `id -un`
    let effective_user = match explicit_user {
        Some(u) if !u.is_empty() => sanitize_for_path(u),
        _ => resolve_effective_user(runner)?,
    };
    if effective_user.is_empty() {
        return Err(VirtuosoError::Config(
            "effective X11 user is empty after sanitization".into(),
        ));
    }

    let remote_dir = x11_remote_dir_with_user(&effective_user, client_id);
    let remote_path = format!("{remote_dir}/x11_dismiss_dialog_{digest}.py");

    // Use `install -d -m 700` to create directory with strict permissions.
    // The path is already sanitized, so shell injection is not possible.
    // Propagate failures since mkdir errors indicate real problems (permissions, disk space, etc.).
    let install_cmd = format!("install -d -m 700 {}", shell_escape(&remote_dir));
    let result = runner.run_command(&CommandRequest::untimed(&install_cmd))?;
    if !result.success {
        return Err(VirtuosoError::Execution(format!(
            "failed to create X11 scratch dir '{}': exit {} — {}",
            remote_dir,
            result.exit_status,
            result.stderr.trim()
        )));
    }

    // Best-effort upload: if the file already exists with the same hash, the
    // hash-suffixed name avoids a write — but we still upload unconditionally
    // on the first call of a session to keep semantics simple. Idempotent.
    runner.upload_text(&UploadTextRequest::untimed(&source, &remote_path))?;
    Ok(remote_path)
}

/// Discover DISPLAY/XAUTHORITY from a running virtuoso process.
#[allow(dead_code)]
pub fn detect_env(runner: &dyn RemoteTransport, user: Option<&str>) -> Result<X11Env> {
    Ok(detect_envs(runner, user)?
        .into_iter()
        .next()
        .unwrap_or(X11Env {
            display: None,
            xauthority: None,
        }))
}

/// Discover all interactive Virtuoso DISPLAY/XAUTHORITY pairs.
pub fn detect_envs(runner: &dyn RemoteTransport, user: Option<&str>) -> Result<Vec<X11Env>> {
    let user_filter = match user {
        Some(u) => format!("-u {u} "),
        None => "".to_string(),
    };
    let cmd = format!(
        "pgrep {user_filter}-x virtuoso | while read pid; do if tr '\\0' ' ' </proc/$pid/cmdline 2>/dev/null | grep -q -- '-nograph'; then continue; fi; printf '__PID__=%s\\n' \"$pid\"; tr '\\0' '\\n' </proc/$pid/environ 2>/dev/null | grep -E '^(DISPLAY|XAUTHORITY)='; done"
    );
    let out = runner.run_command(&CommandRequest::with_exec_timeout(
        &cmd,
        Duration::from_secs(10),
    ))?;
    let mut envs = Vec::new();
    let mut current = X11Env {
        display: None,
        xauthority: None,
    };
    let mut seen = std::collections::BTreeSet::new();
    for line in out.stdout.lines() {
        if line.starts_with("__PID__=") {
            push_unique_env(&mut envs, &mut seen, &mut current);
        } else if let Some(v) = line.strip_prefix("DISPLAY=") {
            if !v.is_empty() {
                current.display = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("XAUTHORITY=") {
            if !v.is_empty() {
                current.xauthority = Some(v.to_string());
            }
        }
    }
    push_unique_env(&mut envs, &mut seen, &mut current);
    Ok(envs)
}

fn push_unique_env(
    envs: &mut Vec<X11Env>,
    seen: &mut std::collections::BTreeSet<(String, Option<String>)>,
    current: &mut X11Env,
) {
    let Some(display) = current.display.take() else {
        current.xauthority = None;
        return;
    };
    let xauthority = current.xauthority.take();
    if seen.insert((display.clone(), xauthority.clone())) {
        envs.push(X11Env {
            display: Some(display),
            xauthority,
        });
    }
}

/// The X11 helper exits with code 1 to signal "no dialogs found"
/// (`x11_dismiss_dialog.py`: `if not dialogs: sys.exit(1)`). In detection-only
/// mode an empty result is the healthy state, so it must not be surfaced as a
/// helper failure.
fn helper_exited_no_dialogs(out: &CommandResult, parsed_empty: bool) -> bool {
    out.exit_status == 1 && parsed_empty
}

/// Run the helper in detection-only mode (no dismiss).
pub fn list_dialogs(
    runner: &dyn RemoteTransport,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<(X11Env, Vec<DialogInfo>)> {
    let helper = ensure_helper_uploaded(runner, user, client_id)?;
    let envs = resolve_envs(runner, user, explicit_display)?;
    let primary_env = envs[0].clone();
    // If the helper itself failed (e.g. xwininfo missing, libX11 not installed,
    // python not on PATH), surface the error so the user doesn't see an empty
    // list and assume "no dialogs". We attach a synthetic "no-dialog" entry so
    // the existing (env, Vec<DialogInfo>) signature stays unchanged.
    let mut dialogs = Vec::new();
    for env in &envs {
        let display = env.display.as_deref().unwrap_or("");
        let cmd = build_helper_cmd(
            &helper,
            display,
            env.xauthority.as_deref(),
            false,
            "enter",
            None,
        );
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &cmd,
            Duration::from_secs(30),
        ))?;
        let mut these = parse_helper_output(&out);
        annotate_dialogs(&mut these, display);
        // exit code 1 + empty output means "no dialogs" (healthy), not failure.
        let helper_errors = if helper_exited_no_dialogs(&out, these.is_empty()) {
            Vec::new()
        } else {
            extract_helper_errors(&out)
        };
        if these.is_empty() && !helper_errors.is_empty() {
            for e in &helper_errors {
                these.push(DialogInfo {
                    window_id: "helper-error".into(),
                    requested_window_id: None,
                    resolved_window_id: None,
                    display: Some(display.to_string()),
                    title: format!("x11 helper error: {e}"),
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                    child: None,
                    action: None,
                    still_mapped: None,
                });
            }
        }
        dialogs.extend(these);
    }
    Ok((primary_env, dialogs))
}

/// Run the helper in dismiss mode.
pub fn dismiss(
    runner: &dyn RemoteTransport,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
    action: &str,
    dry_run: bool,
    window_id: Option<&str>,
) -> Result<DismissResult> {
    let helper = ensure_helper_uploaded(runner, user, client_id)?;
    let mut found = Vec::new();
    let mut dismissed = Vec::new();
    let mut errors = Vec::new();
    let mut logs = Vec::new();
    let envs = resolve_envs(runner, user, explicit_display)?;
    for env in &envs {
        let display = env.display.as_deref().unwrap_or("");
        let cmd = build_helper_cmd(
            &helper,
            display,
            env.xauthority.as_deref(),
            !dry_run,
            action,
            window_id,
        );
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &cmd,
            Duration::from_secs(30),
        ))?;
        for line in out.stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("error").is_some() {
                    continue;
                }
                if val.get("dismissed").is_some() {
                    dismissed.push(dialog_info_from_dismiss_value(&val, None, Some(display)));
                } else if let Ok(mut d) = serde_json::from_value::<DialogInfo>(val) {
                    d.display = Some(display.to_string());
                    found.push(d);
                }
            }
        }
        errors.extend(extract_helper_errors(&out));
        logs.push(truncate_log(&out));
    }
    append_still_mapped_errors(&mut errors, &dismissed);
    Ok(DismissResult {
        display: envs
            .iter()
            .filter_map(|e| e.display.as_deref())
            .collect::<Vec<_>>()
            .join(","),
        found,
        dismissed,
        errors,
        raw_log: logs.join("\n--- next display ---\n"),
    })
}

/// Parse the helper's `--list-windows` stdout into `WindowInfo`s.
///
/// The helper emits only per-window properties (frame_id/window_id/title/
/// class/pid/geometry). `display`/`xauthority` are properties of the DISPLAY
/// that was *queried*, not of the window, so they are backfilled here.
///
/// Backfilling is mandatory, not cosmetic: `resolve_unique_window` requires an
/// exact DISPLAY match and treats `None` as a mismatch, so a caller that skips
/// this step fails every resolution with `not_found`. Both `list_windows` and
/// `action_x11` go through this function so they cannot drift apart again.
fn parse_window_list(stdout: &str, display: &str, xauthority: Option<&String>) -> Vec<WindowInfo> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<WindowInfo>(l.trim()).ok())
        .map(|mut w| {
            w.display = Some(display.to_string());
            w.xauthority = xauthority.cloned();
            w
        })
        .collect()
}

/// Enumerate Virtuoso-related X11 windows. No dismiss action.
pub fn list_windows(
    runner: &dyn RemoteTransport,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<(X11Env, Vec<WindowInfo>)> {
    let helper = ensure_helper_uploaded(runner, user, client_id)?;
    let envs = resolve_envs(runner, user, explicit_display)?;
    let primary_env = envs[0].clone();
    let mut windows = Vec::new();
    let mut helper_errors = Vec::new();
    for env in &envs {
        let display = env.display.as_deref().unwrap_or("");
        let cmd = build_helper_cmd_list_windows(&helper, display, env.xauthority.as_deref());
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &cmd,
            Duration::from_secs(15),
        ))?;
        let these = parse_window_list(&out.stdout, display, env.xauthority.as_ref());
        // The helper exits with code 1 + empty output to signal "no Virtuoso
        // windows" (healthy), same semantics as list_dialogs. Don't surface
        // that as a helper failure, otherwise `list-windows-x11` errors out
        // whenever the display simply has no Virtuoso windows.
        let env_helper_errors = if helper_exited_no_dialogs(&out, these.is_empty()) {
            Vec::new()
        } else {
            extract_helper_errors(&out)
        };
        helper_errors.extend(env_helper_errors);
        windows.extend(these);
    }
    // If the helper died and produced no windows, surface the error so callers
    // can distinguish "no Virtuoso windows" from "x11 helper crashed".
    if windows.is_empty() && !helper_errors.is_empty() {
        return Err(VirtuosoError::Execution(format!(
            "x11 helper failed: {}",
            helper_errors.join("; ")
        )));
    }
    Ok((primary_env, windows))
}

/// Result of a single X11 action operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X11ActionResult {
    pub status: String,
    pub operation: String,
    pub window_id: String,
    pub pid: u32,
    pub display: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Sanitized details (e.g. "text_length: 5" for type, "keycode: 65" for key).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Artifact info for screenshot operations (None for other ops).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactInfo>,
}

/// Metadata describing a fetched screenshot artifact (name, size, sha256).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    /// Artifact name chosen by the caller (e.g. "baseline.png" or "0x1a2b.png").
    pub name: String,
    /// Local path where the artifact was written (evidence dir).
    pub local_path: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// SHA-256 hex digest of the file contents (for evidence integrity).
    pub sha256: String,
}

/// Parameters for an X11 action operation, already validated.
///
/// Reserved for callers that construct validated parameter bundles before
/// invoking `action_x11`; currently the CLI path validates inline.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ActionParams {
    pub window_id: String,
    pub pid: u32,
    pub display: String,
    pub operation: X11Operation,
    /// For key/type: the text or key chord
    pub text: Option<String>,
    /// For click-rel/drag-rel: x coordinate
    pub x: Option<i32>,
    /// For click-rel/drag-rel: y coordinate
    pub y: Option<i32>,
    /// For click-rel/drag-rel: mouse button (1=left, 2=middle, 3=right).
    /// When None, xdotool/click defaults to button 1.
    pub button: Option<u8>,
    /// For screenshot: output directory
    pub output_dir: Option<String>,
    /// For wait: condition to poll
    pub condition: Option<String>,
    /// Window geometry for bounds checking
    pub geometry: Option<Geometry>,
}

/// Allowlisted X11 action operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Operation {
    Activate,
    Key,
    Type,
    ClickRel,
    DragRel,
    Screenshot,
    Wait,
    Close,
}

impl X11Operation {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "activate" => Ok(Self::Activate),
            "key" => Ok(Self::Key),
            "type" => Ok(Self::Type),
            "click-rel" => Ok(Self::ClickRel),
            "drag-rel" => Ok(Self::DragRel),
            "screenshot" => Ok(Self::Screenshot),
            "wait" => Ok(Self::Wait),
            "close" => Ok(Self::Close),
            _ => Err(VirtuosoError::Config(format!(
                "unknown operation '{}': must be one of activate|key|type|click-rel|drag-rel|screenshot|wait|close",
                s
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Key => "key",
            Self::Type => "type",
            Self::ClickRel => "click-rel",
            Self::DragRel => "drag-rel",
            Self::Screenshot => "screenshot",
            Self::Wait => "wait",
            Self::Close => "close",
        }
    }
}

/// Validate action parameters.
///
/// Mirrors the positional flags of `vcli window action-x11`, hence the
/// argument count; kept positional intentionally for API parity with clap.
#[allow(clippy::too_many_arguments)]
pub fn validate_action_params(
    window_id: &str,
    pid: u32,
    display: &str,
    operation: &str,
    x: Option<i32>,
    y: Option<i32>,
    text: Option<&str>,
    button: Option<u8>,
    output_dir: Option<&str>,
) -> Result<String> {
    // Reject empty window_id
    if window_id.is_empty() {
        return Err(VirtuosoError::Config(
            "window_id is required and cannot be empty".into(),
        ));
    }

    // Reject non-positive PID
    if pid == 0 {
        return Err(VirtuosoError::Config(
            "positive PID required (session PID must be resolved before any GUI action)".into(),
        ));
    }

    // Reject empty display
    if display.is_empty() {
        return Err(VirtuosoError::Config(
            "DISPLAY is required and cannot be empty".into(),
        ));
    }

    let op = X11Operation::from_str(operation)?;

    // Operation-specific validation
    match op {
        X11Operation::Activate | X11Operation::Close => {
            // no extra params beyond the common ones
        }
        X11Operation::Screenshot => {
            // screenshot requires a caller-provided output directory, and the
            // directory must not contain path-traversal components
            let dir = output_dir.ok_or_else(|| {
                VirtuosoError::Config(
                    "operation 'screenshot' requires --output-dir parameter".into(),
                )
            })?;
            if dir.split('/').any(|c| c == "..") {
                return Err(VirtuosoError::Config(
                    "screenshot output_dir must not contain '..' (path traversal rejected)".into(),
                ));
            }
        }
        X11Operation::Key | X11Operation::Type => {
            // key and type require non-empty text
            let t = text.ok_or_else(|| {
                VirtuosoError::Config(format!(
                    "operation '{}' requires --text parameter",
                    op.as_str()
                ))
            })?;
            if t.is_empty() {
                return Err(VirtuosoError::Config(format!(
                    "operation '{}' requires non-empty --text",
                    op.as_str()
                )));
            }
        }
        X11Operation::ClickRel | X11Operation::DragRel => {
            // click-rel and drag-rel require x and y
            let _ = x.ok_or_else(|| {
                VirtuosoError::Config(format!(
                    "operation '{}' requires --x parameter",
                    op.as_str()
                ))
            })?;
            let _ = y.ok_or_else(|| {
                VirtuosoError::Config(format!(
                    "operation '{}' requires --y parameter",
                    op.as_str()
                ))
            })?;
            // button, if given, must be 1/2/3 (left/middle/right)
            if let Some(b) = button {
                if !(1..=3).contains(&b) {
                    return Err(VirtuosoError::Config(format!(
                        "button must be 1 (left), 2 (middle), or 3 (right); got {b}"
                    )));
                }
            }
        }
        X11Operation::Wait => {
            // wait condition (in --text) is evaluated by the caller's polling loop;
            // an empty pattern matches every title (substring match) so reject it
            // to avoid immediate false-positive matches.
            let t = text.ok_or_else(|| {
                VirtuosoError::Config("operation 'wait' requires --text parameter".into())
            })?;
            if t.is_empty() {
                return Err(VirtuosoError::Config(
                    "operation 'wait' requires non-empty --text (empty pattern matches every window)".into(),
                ));
            }
        }
    }

    // Build sanitized details string
    let details = match op {
        X11Operation::Activate => None,
        X11Operation::Key => Some(format!(
            "text_length: {}",
            text.map(|s| s.len()).unwrap_or(0)
        )),
        X11Operation::Type => Some(format!(
            "text_length: {}",
            text.map(|s| s.len()).unwrap_or(0)
        )),
        X11Operation::ClickRel => Some(format!(
            "x: {}, y: {}, button: {}",
            x.unwrap_or(0),
            y.unwrap_or(0),
            button.unwrap_or(1)
        )),
        X11Operation::DragRel => Some(format!(
            "x: {}, y: {}, button: {}",
            x.unwrap_or(0),
            y.unwrap_or(0),
            button.unwrap_or(1)
        )),
        X11Operation::Screenshot => {
            let dir = output_dir.unwrap_or("");
            Some(format!("output_dir: {}", dir))
        }
        X11Operation::Wait => {
            // wait uses a condition expression in --text; validated by the caller
            None
        }
        X11Operation::Close => None,
    };

    Ok(details.unwrap_or_default())
}

pub fn dismiss_window(
    runner: &dyn RemoteTransport,
    client_id: &str,
    user: Option<&str>,
    explicit_display: Option<&str>,
    window_id: &str,
    action: &str,
) -> Result<DismissResult> {
    if !["enter", "escape", "alt-y", "alt-n", "alt-o"].contains(&action) {
        return Err(VirtuosoError::Config(format!(
            "invalid action '{action}': must be one of enter|escape|alt-y|alt-n|alt-o"
        )));
    }
    if window_id.is_empty() {
        return Err(VirtuosoError::Config(
            "window_id is required for --dismiss-window".into(),
        ));
    }
    let helper = ensure_helper_uploaded(runner, user, client_id)?;
    let mut dismissed: Vec<DialogInfo> = Vec::new();
    let mut errors = Vec::new();
    let mut logs = Vec::new();
    let envs = resolve_envs(runner, user, explicit_display)?;
    for env in &envs {
        let display = env.display.as_deref().unwrap_or("");
        let cmd = build_helper_cmd_dismiss_window(
            &helper,
            display,
            env.xauthority.as_deref(),
            window_id,
            action,
        );
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &cmd,
            Duration::from_secs(15),
        ))?;
        for line in out.stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("error").is_some() {
                    continue;
                }
                if val.get("dismissed").is_some() {
                    dismissed.push(dialog_info_from_dismiss_value(
                        &val,
                        Some(window_id),
                        Some(display),
                    ));
                }
            }
        }
        errors.extend(extract_helper_errors(&out));
        logs.push(truncate_log(&out));
    }
    append_still_mapped_errors(&mut errors, &dismissed);
    Ok(DismissResult {
        display: envs
            .iter()
            .filter_map(|e| e.display.as_deref())
            .collect::<Vec<_>>()
            .join(","),
        found: Vec::new(),
        dismissed,
        errors,
        raw_log: logs.join("\n--- next display ---\n"),
    })
}

/// Execute a fixed-semantics X11 action on a specific window.
///
/// Bundles identity + action inputs into `ActionX11Inputs` to keep the
/// signature under the clippy argument ceiling. Internally it:
/// 1. uploads the remote helper, lists windows, and resolves the unique
///    window via `resolve_unique_window` (strict PID + DISPLAY binding);
/// 2. re-validated the resolved window matches the requested window_id;
/// 3. enforces geometry bounds for click/drag;
/// 4. builds and runs the fixed xdotool/import argv via the command runner.
pub fn action_x11(
    runner: &dyn RemoteTransport,
    client_id: &str,
    user: Option<&str>,
    inputs: &ActionX11Inputs<'_>,
) -> Result<X11ActionResult> {
    let ActionX11Inputs {
        window_id,
        pid,
        display,
        operation,
        x,
        y,
        button,
        text,
        output_dir,
        timeout_secs,
    } = inputs;
    let start = std::time::Instant::now();

    // Step 1: List windows and resolve unique window
    let helper = ensure_helper_uploaded(runner, user, client_id)?;
    let envs = resolve_envs(runner, user, Some(display))?;

    let env = envs
        .iter()
        .find(|e| e.display.as_deref() == Some(display))
        .ok_or_else(|| VirtuosoError::Config(format!("DISPLAY '{display}' not found")))?;

    let cmd = build_helper_cmd_list_windows(&helper, display, env.xauthority.as_deref());
    let out = runner.run_command(&CommandRequest::with_exec_timeout(
        &cmd,
        Duration::from_secs(*timeout_secs),
    ))?;

    let windows = parse_window_list(&out.stdout, display, env.xauthority.as_ref());

    // Build DISPLAY/XAUTHORITY prefix early so both the resolution fallback and
    // the action commands below can reuse it.
    let display_prefix = match env.xauthority {
        Some(ref xa) => format!(
            "env DISPLAY={} XAUTHORITY={} ",
            shell_escape(display),
            shell_escape(xa)
        ),
        None => format!("env DISPLAY={} ", shell_escape(display)),
    };

    // Step 2: Resolve the target window.
    // match by id + PID + DISPLAY directly — this disambiguates multiple windows
    // sharing one PID (common in Virtuoso: CIW + tool windows same process).
    // Without window_id, fall back to resolve_unique_window which errors on
    // multi-window ambiguity.
    let resolved = if !window_id.is_empty() {
        let wid = *window_id;
        let matched: Vec<&WindowInfo> = windows
            .iter()
            .filter(|w| {
                w.pid == Some(*pid)
                    && w.display.as_deref() == Some(display)
                    && (w.window_id == wid || w.dismiss_id == wid || w.frame_id == wid)
            })
            .collect();
        match matched.len() {
            0 => {
                // The window isn't in the visible list — it may be minimized,
                // unmapped, or on a different virtual desktop. xdotool can still
                // operate on minimized windows (e.g. `key`, `windowactivate`),
                // so fall back to a direct xdotool existence check before
                // giving up. This lets automation target windows that were
                // minimized between the list-windows snapshot and the action.
                let verify_cmd = format!(
                    "{}xdotool getwindowname {}",
                    display_prefix,
                    shell_escape(window_id)
                );
                let verify = runner.run_command(&CommandRequest::with_exec_timeout(
                    &verify_cmd,
                    Duration::from_secs(5),
                ))?;
                if verify.success && !verify.stdout.trim().is_empty() {
                    WindowInfo {
                        frame_id: window_id.to_string(),
                        window_id: window_id.to_string(),
                        dismiss_id: window_id.to_string(),
                        display: Some(display.to_string()),
                        xauthority: env.xauthority.clone(),
                        title: verify.stdout.trim().to_string(),
                        class: Vec::new(),
                        geometry: Geometry::default(),
                        pid: Some(*pid),
                        visible: false,
                    }
                } else {
                    return Err(VirtuosoError::NotFound(format!(
                        "no window with id '{window_id}' matching PID={pid} DISPLAY={display}"
                    )));
                }
            }
            1 => matched.into_iter().next().unwrap().clone(),
            _ => {
                return Err(VirtuosoError::Conflict(format!(
                    "multiple windows matching id '{window_id}' PID={pid} DISPLAY={display}",
                )))
            }
        }
    } else {
        resolve_unique_window(&windows, *pid, display, None)?
    };

    // Step 3 (legacy): verify the resolved window matches the requested id.
    // When window_id drove the match in Step 2 this is a no-op.
    if !window_id.is_empty()
        && resolved.window_id != *window_id
        && resolved.dismiss_id != *window_id
        && resolved.frame_id != *window_id
    {
        return Err(VirtuosoError::NotFound(format!(
            "no window with id '{}' matching PID={pid} DISPLAY={display} (resolved to '{}')",
            window_id, resolved.window_id
        )));
    }

    // Step 4: Check geometry bounds for absolute click coordinates. Only
    // ClickRel passes window-relative absolute coordinates, so only it is
    // bounds-checked. DragRel's x/y are a relative displacement (mousemove
    // delta) and may legitimately be negative (drag up/left) or exceed the
    // window size mid-drag. A zero-sized geometry means the window bounds are
    // unknown (unparsed or abnormal); don't blanket-reject — trust the
    // caller's coordinates instead.
    if *operation == X11Operation::ClickRel {
        if let (Some(op_x), Some(op_y)) = (*x, *y) {
            let geom = &resolved.geometry;
            if geom.w > 0
                && geom.h > 0
                && (op_x < 0 || op_y < 0 || op_x >= geom.w || op_y >= geom.h)
            {
                return Err(VirtuosoError::Config(format!(
                    "coordinates ({op_x}, {op_y}) out of bounds for window size {}x{}",
                    geom.w, geom.h
                )));
            }
        }
    }

    let operation = *operation;

    // Step 6: Build and execute the xdotool / import commands. Most
    // operations yield a single command; drag yields three (mousedown,
    // mousemove, mouseup).
    //
    // Drag uses a MouseButtonGuard so that if the intermediate mousemove
    // fails (SSH disconnect, timeout, NAK), a best-effort mouseup is still
    // issued to release the held button.
    // `wait` polls list-windows until the --text pattern matches a window title
    // or the timeout expires; success is driven by that outcome, not by the
    // individual helper runs.
    let mut wait_succeeded = false;
    let (results, artifact) = if operation == X11Operation::Screenshot {
        let dir = output_dir
            .ok_or_else(|| VirtuosoError::Config("screenshot requires --output-dir".into()))?;
        // Use a random temp name on the GUI host to avoid colliding with
        // prior runs or concurrent agents targeting the same DISPLAY.
        let token = Uuid::new_v4();
        let safe_name = format!("vcli_shot_{token}.png");
        let remote_path = format!("/tmp/{safe_name}");
        let remote_cmd = format!(
            "{}import -window {} {}",
            display_prefix,
            shell_escape(&resolved.window_id),
            shell_escape(&remote_path),
        );
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &remote_cmd,
            Duration::from_secs(*timeout_secs),
        ))?;
        if !out.success {
            return Err(VirtuosoError::Execution(format!(
                "import -window failed: {} (stderr: {})",
                out.exit_status,
                out.stderr.trim()
            )));
        }
        // Fetch the PNG back to the local evidence directory.
        match runner.fetch_file(&remote_path, dir, Duration::from_secs(*timeout_secs)) {
            Ok(()) => {}
            Err(e) => {
                // The import already wrote the PNG to remote_path on the GUI
                // host; since the fetch failed, clean it up there. Best-effort.
                let _ = runner.run_command(&CommandRequest::untimed(format!(
                    "rm -f {}",
                    shell_escape(&remote_path)
                )));
                return Err(VirtuosoError::Execution(format!(
                    "screenshot fetch failed: {e}"
                )));
            }
        }
        // Validate PNG magic bytes (89 50 4E 47 0D 0A 1A 0A).
        let local_path_buf = std::path::PathBuf::from(dir).join(&safe_name);
        match validate_png_artifact(&local_path_buf) {
            Ok((size, hash)) => {
                // Best-effort cleanup of the temp PNG on the GUI host (remote
                // or local — the runner abstracts the host).
                let _ = runner.run_command(&CommandRequest::untimed(format!(
                    "rm -f {}",
                    shell_escape(&remote_path)
                )));
                (
                    vec![out],
                    Some(ArtifactInfo {
                        name: safe_name,
                        local_path: local_path_buf.to_string_lossy().into(),
                        size_bytes: size,
                        sha256: hash,
                    }),
                )
            }
            Err(e) => {
                let _ = std::fs::remove_file(&local_path_buf); // cleanup invalid file
                                                               // Also remove the temp PNG left on the GUI host.
                let _ = runner.run_command(&CommandRequest::untimed(format!(
                    "rm -f {}",
                    shell_escape(&remote_path)
                )));
                return Err(e);
            }
        }
    } else if operation == X11Operation::Wait {
        // Wait for a window whose title contains the --text pattern to appear.
        // Polls a fresh list-windows snapshot until a match or the timeout
        // expires (default 30s). Returns success only when a match is observed.
        let pattern = text.ok_or_else(|| {
            VirtuosoError::Config("wait operation requires --text window-title pattern".into())
        })?;
        let (matched, outs) = wait_for_window_pattern(
            runner,
            &helper,
            display,
            env.xauthority.as_deref(),
            pattern,
            Duration::from_secs(*timeout_secs),
            &windows,
        )?;
        wait_succeeded = matched;
        (outs, None)
    } else if operation == X11Operation::DragRel {
        // Drag needs a MouseButtonGuard: mousedown -> mousemove -> mouseup.
        // If mousemove fails mid-drag (SSH drop, timeout), the guard's Drop
        // will still issue a best-effort mouseup so we don't leave the button
        // held on the CIW.
        let cmds = build_xdotool_actions(&resolved, operation, *x, *y, *button, *text)?;
        let mut outs = Vec::with_capacity(cmds.len());
        let mut guard = MouseButtonGuard::new(runner, &display_prefix, &resolved, *button);

        for (i, (_sub, argv)) in cmds.into_iter().enumerate() {
            let remote_cmd = format!(
                "{}xdotool {}",
                display_prefix,
                argv.iter()
                    .map(|a| shell_escape(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            match runner.run_command(&CommandRequest::with_exec_timeout(
                &remote_cmd,
                Duration::from_secs(*timeout_secs),
            )) {
                Ok(cmd_out) => {
                    if i == 0 {
                        guard.arm(); // mousedown succeeded: arm the release guard
                    } else if i == 2 {
                        guard.mark_released(); // explicit mouseup succeeded
                    }
                    outs.push(cmd_out);
                }
                Err(err) => {
                    // mousedown succeeded but a later step failed: guard's Drop
                    // will still run mouseup. mousedown failed (i==0): nothing
                    // to release because the button was never pressed.
                    return Err(VirtuosoError::Execution(format!(
                        "xdotool {} failed: {}",
                        _sub, err
                    )));
                }
            }
        }
        (outs, None)
    } else {
        let cmds = build_xdotool_actions(&resolved, operation, *x, *y, *button, *text)?;
        let mut outs = Vec::with_capacity(cmds.len());
        for (_sub, argv) in cmds {
            let remote_cmd = format!(
                "{}xdotool {}",
                display_prefix,
                argv.iter()
                    .map(|a| shell_escape(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let out = runner.run_command(&CommandRequest::with_exec_timeout(
                &remote_cmd,
                Duration::from_secs(*timeout_secs),
            ))?;
            outs.push(out);
        }
        (outs, None)
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    // P2-1: xdotool key exits 0 even for invalid key names (it only prints
    // "No such key name '...'. Ignoring it." to stderr). Detect that explicitly
    // so the caller gets a real error instead of a false success.
    if operation == X11Operation::Key {
        for out in &results {
            if out.stderr.contains("No such key name") {
                let detail = out
                    .stderr
                    .lines()
                    .find(|l| l.contains("No such key name"))
                    .unwrap_or("")
                    .trim();
                return Err(VirtuosoError::Config(format!("invalid key name: {detail}")));
            }
        }
    }

    // wait's success is the match outcome (matched within timeout), not the
    // exit status of the last polling helper run.
    let success = if operation == X11Operation::Wait {
        wait_succeeded
    } else {
        results.last().map(|r| r.success).unwrap_or(true)
    };

    // Build sanitized details
    let details = match operation {
        X11Operation::Activate => None,
        X11Operation::Close => None,
        X11Operation::Key => Some(format!("key: {}", text.unwrap_or(""))),
        X11Operation::Type => Some(format!(
            "text_length: {}",
            text.map(|s| s.len()).unwrap_or(0)
        )),
        X11Operation::ClickRel | X11Operation::DragRel => Some(format!(
            "x: {}, y: {}, button: {}",
            x.unwrap_or(0),
            y.unwrap_or(0),
            button.unwrap_or(1)
        )),
        X11Operation::Screenshot => Some(format!("output_dir: {}", output_dir.unwrap_or(""))),
        X11Operation::Wait => Some(format!(
            "pattern: {:?}, matched: {}",
            text.unwrap_or(""),
            wait_succeeded
        )),
    };

    Ok(X11ActionResult {
        status: if success { "success" } else { "failure" }.to_string(),
        operation: operation.as_str().to_string(),
        window_id: window_id.to_string(),
        pid: *pid,
        display: display.to_string(),
        duration_ms,
        details,
        artifact,
    })
}

/// Bundles identity + action inputs for `action_x11`; mirrors the CLI flags.
#[derive(Debug, Clone)]
pub struct ActionX11Inputs<'a> {
    pub window_id: &'a str,
    pub pid: u32,
    pub display: &'a str,
    pub operation: X11Operation,
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// Mouse button for click/drag: 1=left, 2=middle, 3=right. None = default 1.
    pub button: Option<u8>,
    pub text: Option<&'a str>,
    pub output_dir: Option<&'a str>,
    pub timeout_secs: u64,
}

/// Build xdotool commands for an action operation.
///
/// Returns a list of (command, argv) pairs. Most operations yield a single
/// command; drag yields three sequential commands (mousedown, mousemove,
/// mouseup) so the motion is a true drag-and-drop.
///
/// Each command's argv starts with the xdotool subcommand, followed by
/// action-specific flags, followed by `--window <id>` to target the
/// specific window. All arguments are separate items — no shell
/// interpolation. Screenshot and Wait are handled in action_x11, not here.
fn build_xdotool_actions(
    window: &WindowInfo,
    operation: X11Operation,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<u8>,
    text: Option<&str>,
) -> Result<Vec<(String, Vec<String>)>> {
    let wid = window.window_id.clone();
    let btn = button.unwrap_or(1).to_string();

    match operation {
        X11Operation::Activate => {
            // `xdotool windowraise [window_id]` — this command accepts NO
            // options: neither `--sync` nor `--window` exist for it. Both are
            // rejected by every xdotool release (3.20200624.1 through current
            // upstream xdotool.pod), so the window id must be positional.
            Ok(vec![(
                "windowraise".into(),
                vec!["windowraise".into(), wid],
            )])
        }
        X11Operation::Key => {
            // xdotool key --window <id> <key>...
            // xdotool separates chord modifiers/keys with `+` (e.g. `ctrl+s`).
            // Older xdotool releases (e.g. 3.20200624.1) reject the hyphenated
            // form `ctrl-s`, and no X11 keysym name contains a hyphen, so
            // normalizing `-` to `+` is safe and makes `key --text "ctrl-s"`
            // work across xdotool versions.
            let t =
                text.ok_or_else(|| VirtuosoError::Config("key operation requires --text".into()))?;
            let mut argv = vec!["key".into(), "--window".into(), wid];
            for key in t.split_whitespace() {
                argv.push(key.replace('-', "+"));
            }
            Ok(vec![("key".into(), argv)])
        }
        X11Operation::Type => {
            // xdotool type --window <id> -- <text>
            let t =
                text.ok_or_else(|| VirtuosoError::Config("type operation requires --text".into()))?;
            // xdotool type drives XTestFakeKeyEvent and only supports ASCII
            // keysyms. Non-ASCII text (CJK, emoji, full-width) is silently
            // dropped by xdotool — detect it upfront and fail loudly instead
            // of reporting a false success.
            if !t.is_ascii() {
                return Err(VirtuosoError::Config(format!(
                    "type operation supports ASCII only; got non-ASCII text ({} bytes). \
                     Use clipboard paste (xclip + ctrl+v) for non-ASCII input",
                    t.len()
                )));
            }
            Ok(vec![(
                "type".into(),
                vec!["type".into(), "--window".into(), wid, "--".into(), t.into()],
            )])
        }
        X11Operation::ClickRel => {
            // Two-step click: first move cursor to the window-relative point,
            // then issue the click. `--` separator before coords protects
            // negative-valued x/y from being parsed as xdotool flags.
            let (rx, ry) = match (x, y) {
                (Some(xx), Some(yy)) => (xx, yy),
                _ => {
                    return Err(VirtuosoError::Config(
                        "click-rel requires --x and --y".into(),
                    ));
                }
            };
            Ok(vec![
                // Step 1: xdotool mousemove --window <id> -- <x> <y>
                (
                    "mousemove".into(),
                    vec![
                        "mousemove".into(),
                        "--window".into(),
                        wid.clone(),
                        "--".into(),
                        rx.to_string(),
                        ry.to_string(),
                    ],
                ),
                // Step 2: xdotool click --window <id> <button>
                // `click [options] button` — the button is a POSITIONAL
                // argument; there is no `--button` option on click/mousedown/
                // mouseup in any xdotool release.
                (
                    "click".into(),
                    vec!["click".into(), "--window".into(), wid, btn],
                ),
            ])
        }
        X11Operation::DragRel => {
            // True drag: mousedown -> mousemove_relative -> mouseup.
            // Only press/release target the window; the relative move in
            // between is global pointer motion and carries no window id.
            let (rx, yv) = match (x, y) {
                (Some(xx), Some(yy)) => (xx, yy),
                _ => {
                    return Err(VirtuosoError::Config(
                        "drag-rel requires --x and --y".to_string(),
                    ));
                }
            };
            Ok(vec![
                // press — button is positional, there is no `--button` option
                (
                    "mousedown".into(),
                    vec![
                        "mousedown".into(),
                        "--window".into(),
                        wid.clone(),
                        btn.clone(),
                    ],
                ),
                // move relatively
                // Relative pointer motion is its OWN command: `--relative` is
                // not a mousemove option (it belongs to the window command
                // `windowmove`), and `mousemove_relative` takes no `--window`
                // because a delta needs no window-relative origin. `--` keeps
                // negative deltas from being parsed as flags.
                (
                    "mousemove_relative".into(),
                    vec![
                        "mousemove_relative".into(),
                        "--".into(),
                        rx.to_string(),
                        yv.to_string(),
                    ],
                ),
                // release
                (
                    "mouseup".into(),
                    vec!["mouseup".into(), "--window".into(), wid, btn],
                ),
            ])
        }
        X11Operation::Close => {
            // Close the window via Alt+F4 (standard WM close shortcut). xdotool
            // cannot send WM_DELETE directly, so Alt+F4 is the portable path.
            Ok(vec![(
                "key".into(),
                vec!["key".into(), "--window".into(), wid, "alt+F4".into()],
            )])
        }
        X11Operation::Screenshot | X11Operation::Wait => {
            // Handled specially in action_x11, not via xdotool
            Ok(vec![])
        }
    }
}

/// RAII guard that guarantees a mouse-button release after a successful
/// `mousedown`. Used by `action_x11` for drag sequences: if the intermediate
/// `mousemove_relative` fails (SSH drop, X11 timeout) the drag function
/// returns an error before reaching the explicit `mouseup`. Without this
/// guard the button would remain logically held on the CIW.
///
/// Call `arm()` immediately after the mousedown command succeeds. Call
/// `mark_released()` after the explicit mouseup succeeds. On Drop, if
/// still armed (mousedown succeeded but mouseup never ran), a best-effort
/// mouseup is issued. Failures in Drop are logged but never panic.
struct MouseButtonGuard<'a> {
    runner: &'a dyn RemoteTransport,
    display_prefix: &'a str,
    window_id: String,
    button: u8,
    armed: bool,
}

impl<'a> MouseButtonGuard<'a> {
    fn new(
        runner: &'a dyn RemoteTransport,
        display_prefix: &'a str,
        window: &WindowInfo,
        button: Option<u8>,
    ) -> Self {
        Self {
            runner,
            display_prefix,
            window_id: window.window_id.clone(),
            button: button.unwrap_or(1),
            armed: false,
        }
    }

    /// Mark the guard as armed — call after a successful mousedown.
    fn arm(&mut self) {
        self.armed = true;
    }

    /// Mark the guard as released — call after the explicit mouseup succeeds.
    fn mark_released(&mut self) {
        self.armed = false;
    }
}

impl Drop for MouseButtonGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let remote_cmd = format!(
            "{}xdotool mouseup --window {} {}",
            self.display_prefix, self.window_id, self.button,
        );
        let req = CommandRequest::with_exec_timeout(
            &remote_cmd,
            Duration::from_secs(5), // bounded best-effort
        );
        if let Err(e) = self.runner.run_command(&req) {
            // Best-effort: report but do not override the primary error.
            eprintln!(
                "vcli::x11 MouseButtonGuard: best-effort mouseup failed for \
                 window {}: {}",
                self.window_id, e
            );
        }
    }
}

/// PNG file signature: 89 50 4E 47 0D 0A 1A 0A.
const PNG_MAGIC: &[u8; 8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Validate that `path` is a real PNG, then return (size_bytes, sha256_hex).
/// Rejects empty files, tiny files (< 64 bytes), wrong magic, and symlinks.
fn validate_png_artifact(
    path: &std::path::Path,
) -> std::result::Result<(u64, String), VirtuosoError> {
    use std::io::{Read, Seek};

    // Use symlink_metadata so the check does not follow the link — a plain
    // `metadata` would resolve the target and never report a symlink.
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        VirtuosoError::Execution(format!("cannot stat screenshot path {:?}: {e}", path))
    })?;

    // Reject symlinks — they could point outside the evidence directory.
    if metadata.file_type().is_symlink() {
        return Err(VirtuosoError::Execution(format!(
            "screenshot path {:?} is a symlink — rejecting",
            path
        )));
    }

    let size = metadata.len();
    if size < 64 {
        return Err(VirtuosoError::Execution(format!(
            "screenshot file is suspiciously small ({size} bytes) — rejecting"
        )));
    }

    // Open the file. On Unix, refuse to follow a symlink (O_NOFOLLOW) so a
    // race between the symlink_metadata check above and this open cannot swap
    // in a symlink pointing outside the evidence directory.
    let mut file = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .map_err(|e| {
                    VirtuosoError::Execution(format!("cannot open screenshot path {:?}: {e}", path))
                })?
        }
        #[cfg(not(unix))]
        {
            std::fs::File::open(path).map_err(|e| {
                VirtuosoError::Execution(format!("cannot open screenshot path {:?}: {e}", path))
            })?
        }
    };
    // Check magic bytes.
    let mut header = [0u8; 8];
    file.read_exact(&mut header).map_err(|e| {
        VirtuosoError::Execution(format!(
            "cannot read screenshot header from {:?}: {e}",
            path
        ))
    })?;
    if &header != PNG_MAGIC {
        return Err(VirtuosoError::Execution(format!(
            "screenshot file is not a valid PNG (header: {header:02X?})"
        )));
    }

    // Compute SHA-256 of the full file.
    file.rewind()
        .map_err(|e| VirtuosoError::Execution(format!("cannot rewind screenshot file: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            VirtuosoError::Execution(format!("cannot read screenshot for hashing: {e}"))
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let hash = hex::encode(digest);

    Ok((size, hash))
}

fn dialog_info_from_dismiss_value(
    val: &serde_json::Value,
    requested_window_id: Option<&str>,
    display: Option<&str>,
) -> DialogInfo {
    DialogInfo {
        window_id: requested_window_id
            .or_else(|| val.get("dismissed").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string(),
        requested_window_id: val
            .get("requested_window_id")
            .and_then(|v| v.as_str())
            .or(requested_window_id)
            .map(|s| s.to_string()),
        resolved_window_id: val
            .get("resolved_window_id")
            .and_then(|v| v.as_str())
            .or_else(|| val.get("child").and_then(|v| v.as_str()))
            .map(|s| s.to_string()),
        display: display.map(|s| s.to_string()),
        title: val
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        x: val.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        y: val.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        w: val.get("w").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        h: val.get("h").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        child: val
            .get("child")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        action: val
            .get("action")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        still_mapped: val.get("still_mapped").and_then(|v| v.as_bool()),
    }
}

fn annotate_dialogs(dialogs: &mut [DialogInfo], display: &str) {
    for d in dialogs {
        d.display = Some(display.to_string());
    }
}

fn append_still_mapped_errors(errors: &mut Vec<String>, dismissed: &[DialogInfo]) {
    for d in dismissed {
        if d.still_mapped == Some(true) {
            errors.push(format!(
                "window {} still mapped after action {}",
                d.window_id,
                d.action.as_deref().unwrap_or("unknown")
            ));
        }
    }
}

/// Shared env-resolution: explicit display, else auto-detect from the running
/// virtuoso process. Returns the resolved env and the display string.
fn resolve_envs(
    runner: &dyn RemoteTransport,
    user: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<Vec<X11Env>> {
    let envs = match explicit_display {
        Some(d) => vec![X11Env {
            display: Some(d.to_string()),
            xauthority: None,
        }],
        None => detect_envs(runner, user)?,
    };
    if envs.is_empty() {
        return Err(VirtuosoError::Config(
            "cannot detect DISPLAY from virtuoso process".into(),
        ));
    }
    Ok(envs)
}

fn build_helper_cmd(
    helper_remote_path: &str,
    display: &str,
    xauthority: Option<&str>,
    do_dismiss: bool,
    action: &str,
    window_id: Option<&str>,
) -> String {
    // Quote the remote path with single quotes; the helper is ASCII so this is safe.
    let mut s = format!("python3 '{}'", helper_remote_path);
    s.push(' ');
    s.push_str(&shell_escape(display));
    if let Some(wid) = window_id {
        // Explicit target: bypass the dialog-size filter and dismiss this window
        // directly (frame/app/child id all accepted by the helper).
        s.push_str(" --dismiss-window ");
        s.push_str(&shell_escape(wid));
        s.push_str(" --action ");
        s.push_str(action);
    } else if do_dismiss {
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

/// Poll `--list-windows` until a window whose title contains `pattern` appears,
/// or until `timeout` elapses (default 30s via the caller).
///
/// `initial` is the snapshot already fetched by the caller (polled once before
/// the first refresh). Returns `(matched, helper_outputs)` — `matched` drives
/// the action's success status, since a helper exit 0 alone does not mean the
/// window appeared.
fn wait_for_window_pattern(
    runner: &dyn RemoteTransport,
    helper: &str,
    display: &str,
    xauthority: Option<&str>,
    pattern: &str,
    timeout: Duration,
    initial: &[WindowInfo],
) -> Result<(bool, Vec<CommandResult>)> {
    let deadline = std::time::Instant::now() + timeout;
    let mut matched = initial.iter().any(|w| w.title.contains(pattern));
    let mut outs = Vec::new();
    let poll_interval = Duration::from_millis(500);
    while !matched {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Sleep only up to the remaining deadline so we never overshoot timeout
        // by a full poll interval + helper execution time.
        std::thread::sleep(remaining.min(poll_interval));
        if std::time::Instant::now() >= deadline {
            break;
        }
        let cmd = build_helper_cmd_list_windows(helper, display, xauthority);
        let out = runner.run_command(&CommandRequest::with_exec_timeout(
            &cmd,
            Duration::from_secs(30),
        ))?;
        let fresh = parse_window_list(&out.stdout, display, xauthority.map(String::from).as_ref());
        outs.push(out);
        matched = fresh.iter().any(|w| w.title.contains(pattern));
    }
    Ok((matched, outs))
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

/// Resolve a unique window by strict PID + DISPLAY match, with optional title narrowing.
///
/// - `expected_pid` must be positive (zero/None windows do not match).
/// - `expected_display` must match exactly.
/// - `optional_title`, if provided, only narrows the candidate set (substring match).
///
/// Returns `Ok(WindowInfo)` if exactly one window matches.
/// Returns `Err(NotFound)` if zero windows match.
/// Returns `Err(Conflict)` if more than one window matches.
#[allow(dead_code)]
pub fn resolve_unique_window(
    windows: &[WindowInfo],
    expected_pid: u32,
    expected_display: &str,
    optional_title: Option<&str>,
) -> Result<WindowInfo> {
    if expected_pid == 0 {
        return Err(VirtuosoError::NotFound(
            "resolve_unique_window requires a positive PID".into(),
        ));
    }

    // Filter by PID (must be positive) and DISPLAY (exact match)
    let candidates: Vec<&WindowInfo> = windows
        .iter()
        .filter(|w| {
            // Require positive PID — zero/None windows are excluded
            let pid_ok = w.pid.map(|p| p == expected_pid).unwrap_or(false);
            // Exact DISPLAY match
            let display_ok = w
                .display
                .as_deref()
                .map(|d| d == expected_display)
                .unwrap_or(false);
            pid_ok && display_ok
        })
        .collect();

    // Apply optional title narrowing (substring match, not required)
    let candidates: Vec<&WindowInfo> = match optional_title {
        Some(title) => candidates
            .into_iter()
            .filter(|w| w.title.contains(title))
            .collect(),
        None => candidates,
    };

    match candidates.len() {
        0 => Err(VirtuosoError::NotFound(format!(
            "no window matching PID={expected_pid} DISPLAY={expected_display} title={:?}",
            optional_title
        ))),
        1 => Ok(candidates.into_iter().next().unwrap().clone()),
        _ => Err(VirtuosoError::Conflict(format!(
            "multiple windows ({}) matching PID={expected_pid} DISPLAY={expected_display} title={:?}: {}",
            candidates.len(),
            optional_title,
            candidates
                .iter()
                .map(|w| format!("{} (title={:?})", w.window_id, w.title))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
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

fn parse_helper_output(out: &CommandResult) -> Vec<DialogInfo> {
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
fn extract_helper_errors(out: &CommandResult) -> Vec<String> {
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
    if out.exit_status != 0 && seen.is_empty() {
        let stderr_summary = out.stderr.lines().next().unwrap_or("").trim();
        let msg = if !stderr_summary.is_empty() {
            format!(
                "x11 helper exited with code {}: {}",
                out.exit_status, stderr_summary
            )
        } else {
            format!("x11 helper exited with code {}", out.exit_status)
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

fn truncate_log(out: &CommandResult) -> String {
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

/// Construct the configured remote transport.
///
/// When `config.remote_host` is absent or empty, returns a local
/// [`LocalTransport`] that runs commands directly on the workstation.
/// When a remote host is set, delegates to [`crate::transport::backend::open_transport`]
/// so SSH backend selection and `VB_DISABLE_CONTROL_MASTER` are honoured.
pub fn transport_for_config(config: &Config) -> Result<Arc<dyn RemoteTransport>> {
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        Ok(Arc::new(LocalTransport::new()))
    } else {
        Ok(crate::transport::backend::open_transport(config)?)
    }
}

/// A [`RemoteTransport`] that runs commands directly on the local workstation.
/// Used when `VB_REMOTE_HOST` is not set so the X11 helper can still operate locally.
struct LocalTransport;

impl LocalTransport {
    fn new() -> Self {
        Self
    }
}

impl RemoteTransport for LocalTransport {
    fn test_connection(
        &self,
        deadline: crate::transport::contract::Deadline,
    ) -> std::result::Result<bool, crate::transport::contract::TransportError> {
        if deadline.is_expired() {
            return Err(crate::transport::contract::TransportError::QueueTimeout {
                request: crate::transport::contract::RequestId::new(),
                after_secs: 0,
            });
        }
        Ok(true)
    }

    fn run_command(
        &self,
        req: &CommandRequest,
    ) -> std::result::Result<CommandResult, crate::transport::contract::TransportError> {
        use std::io::Read;
        use std::process::Stdio;
        use std::time::Instant;

        if req.deadline.is_expired() {
            return Err(crate::transport::contract::TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }

        let start = Instant::now();

        // Compute the maximum wait time from req.timeout (if set) and remaining deadline.
        let max_wait = req.timeout.map(|t| {
            let remaining = req.deadline.0.saturating_duration_since(start);
            t.min(remaining)
        });

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&req.command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            crate::transport::contract::TransportError::LocalIo(format!("local spawn failed: {e}"))
        })?;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout_bytes = Vec::new();
                    if let Some(ref mut s) = child.stdout {
                        let _ = s.read_to_end(&mut stdout_bytes);
                    }
                    let mut stderr_bytes = Vec::new();
                    if let Some(ref mut s) = child.stderr {
                        let _ = s.read_to_end(&mut stderr_bytes);
                    }
                    let exit_status = status.code().unwrap_or(-1);
                    let success = status.success();
                    return Ok(CommandResult {
                        exit_status,
                        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
                        success,
                        duration: start.elapsed(),
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(crate::transport::contract::TransportError::LocalIo(
                        format!("local try_wait failed: {e}"),
                    ));
                }
            }

            let elapsed = start.elapsed();
            if elapsed >= req.deadline.0.saturating_duration_since(start) {
                // Deadline has passed.
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    crate::transport::contract::TransportError::ExecutionTimeout {
                        request: req.id.clone(),
                        after_secs: start.elapsed().as_secs(),
                        remote_terminated: false,
                    },
                );
            }

            // Wait up to 10ms before next poll.
            let remaining = req.deadline.0.saturating_duration_since(Instant::now());
            let wait_for = max_wait
                .map(|t| t.saturating_sub(elapsed))
                .unwrap_or(remaining)
                .min(Duration::from_millis(10));
            if wait_for.is_zero() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    crate::transport::contract::TransportError::ExecutionTimeout {
                        request: req.id.clone(),
                        after_secs: start.elapsed().as_secs(),
                        remote_terminated: false,
                    },
                );
            }
            std::thread::sleep(wait_for);
        }
    }

    fn upload_file(
        &self,
        req: &crate::transport::contract::UploadFileRequest,
    ) -> std::result::Result<(), crate::transport::contract::TransportError> {
        if req.deadline.is_expired() {
            return Err(crate::transport::contract::TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        std::fs::copy(&req.local, &req.remote).map_err(|e| {
            crate::transport::contract::TransportError::LocalIo(format!(
                "local file copy failed: {e}"
            ))
        })?;
        Ok(())
    }

    fn upload_text(
        &self,
        req: &UploadTextRequest,
    ) -> std::result::Result<(), crate::transport::contract::TransportError> {
        if req.deadline.is_expired() {
            return Err(crate::transport::contract::TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        std::fs::write(&req.remote, &req.text).map_err(|e| {
            crate::transport::contract::TransportError::LocalIo(format!("local write failed: {e}"))
        })?;
        Ok(())
    }

    fn download_file(
        &self,
        req: &crate::transport::contract::DownloadFileRequest,
    ) -> std::result::Result<(), crate::transport::contract::TransportError> {
        if req.deadline.is_expired() {
            return Err(crate::transport::contract::TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        // LocalTransport runs on the same host as vcli, so the "remote" and
        // "local" paths live on the same filesystem — a plain copy is correct.
        std::fs::copy(&req.remote, &req.local).map_err(|e| {
            crate::transport::contract::TransportError::LocalIo(format!(
                "local file copy failed: {e}"
            ))
        })?;
        Ok(())
    }

    fn download_dir(
        &self,
        _req: &crate::transport::contract::DownloadDirRequest,
    ) -> std::result::Result<(), crate::transport::contract::TransportError> {
        Err(
            crate::transport::contract::TransportError::UnsupportedOperation(
                "download_dir not supported on LocalTransport".into(),
            ),
        )
    }
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
            "alt-o",
            None,
        );
        assert!(cmd.contains("'/tmp/virtuoso_bridge/x/x11_dismiss_dialog_abc.py'"));
        assert!(cmd.contains("--dismiss"));
        assert!(cmd.contains("--action alt-o"));
        assert!(!cmd.contains("XAUTHORITY="));
    }

    #[test]
    fn build_helper_cmd_propagates_xauthority_when_set() {
        let cmd = build_helper_cmd(
            "/h.py",
            ":0",
            Some("/tmp/.X11-unix/X0"),
            false,
            "enter",
            None,
        );
        assert!(cmd.contains("XAUTHORITY=/tmp/.X11-unix/X0"));
    }

    #[test]
    fn build_helper_cmd_explicit_window_id_uses_dismiss_window_flag() {
        let cmd = build_helper_cmd("/h.py", ":0", None, true, "escape", Some("0x2603394"));
        assert!(cmd.contains("--dismiss-window 0x2603394"));
        assert!(cmd.contains("--action escape"));
        assert!(!cmd.contains("--dismiss "));
    }

    #[test]
    fn parse_helper_output_picks_json_dialogs_only() {
        let out = CommandResult {
            stdout: "noise\n{\"window_id\":\"0x1\",\"title\":\"a\",\"x\":0,\"y\":0,\"w\":1,\"h\":1}\nmore noise\n".to_string(),
            stderr: "".to_string(),
            success: true,
            exit_status: 0,
            duration: Duration::ZERO,
        };
        let dialogs = parse_helper_output(&out);
        assert_eq!(dialogs.len(), 1);
        assert_eq!(dialogs[0].window_id, "0x1");
        assert_eq!(dialogs[0].still_mapped, None);
    }

    fn mkresult(stdout: &str, stderr: &str, returncode: i32) -> CommandResult {
        CommandResult {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            success: returncode == 0,
            exit_status: returncode,
            duration: Duration::ZERO,
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
    fn extract_helper_errors_empty_inputs_returns_empty() {
        // No stdout, no stderr, returncode 0 → no errors.
        let out = mkresult("", "", 0);
        let errs = extract_helper_errors(&out);
        assert!(errs.is_empty(), "expected empty errors, got {errs:?}");
    }

    #[test]
    fn extract_helper_errors_json_with_extra_fields_still_extracted() {
        // Helper emitted structured error with extra context fields — the
        // `error` key alone is what we surface.
        let out = mkresult(
            r#"{"error": "xauth not set", "code": 2, "context": {"display": ":0"}}"#,
            "",
            0,
        );
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("xauth not set"));
    }

    #[test]
    fn extract_helper_errors_dedupes_repeated_json_lines() {
        // Same error repeated across multiple stdout lines should appear once.
        let out = mkresult(
            "{\"error\": \"duplicate\"}\n{\"error\": \"duplicate\"}\n",
            "",
            0,
        );
        let errs = extract_helper_errors(&out);
        assert_eq!(
            errs.len(),
            1,
            "duplicate JSON errors should dedup: {errs:?}"
        );
        assert_eq!(errs[0], "duplicate");
    }

    #[test]
    fn extract_helper_errors_mixed_json_and_returncode_prefers_json() {
        // When a structured JSON error is present, the generic returncode
        // summary is suppressed — the user gets the specific message, not
        // a fallback like "exited with code 1".
        let out = mkresult(
            "{\"error\": \"specific failure reason\"}\n",
            "python traceback: something failed\n",
            1,
        );
        let errs = extract_helper_errors(&out);
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].contains("specific failure reason"),
            "should surface JSON error, got {errs:?}"
        );
    }

    #[test]
    fn extract_helper_errors_preserves_json_order_then_appends_stderr() {
        // Distinct JSON errors are emitted in order; then if no JSON was
        // found, distinct stderr lines are appended (preserving order).
        let out = mkresult("{\"error\": \"first\"}\n{\"error\": \"second\"}\n", "", 0);
        let errs = extract_helper_errors(&out);
        assert_eq!(errs, vec!["first", "second"]);
    }

    #[test]
    fn truncate_log_caps_at_8k() {
        let huge = "x".repeat(20_000);
        let out = CommandResult {
            stdout: huge.clone(),
            stderr: "".into(),
            success: true,
            exit_status: 0,
            duration: Duration::ZERO,
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
    fn dialog_info_parses_dismiss_metadata() {
        let line = r#"{"window_id":"0x1","title":"a","x":0,"y":0,"w":1,"h":1,"child":"0x2","action":"alt-o","still_mapped":false}"#;
        let d: DialogInfo = serde_json::from_str(line).expect("parse");
        assert_eq!(d.child.as_deref(), Some("0x2"));
        assert_eq!(d.action.as_deref(), Some("alt-o"));
        assert_eq!(d.still_mapped, Some(false));
    }

    #[test]
    fn dismiss_value_uses_reported_window_and_metadata() {
        let val: serde_json::Value = serde_json::from_str(
            r#"{"dismissed":"0x1","child":"0x2","title":"ADE Assembler Message 1749","action":"alt-o","still_mapped":true}"#,
        )
        .expect("json");
        let d = dialog_info_from_dismiss_value(&val, None, Some(":1"));
        assert_eq!(d.window_id, "0x1");
        assert_eq!(d.child.as_deref(), Some("0x2"));
        assert_eq!(d.action.as_deref(), Some("alt-o"));
        assert_eq!(d.still_mapped, Some(true));
        assert_eq!(d.display.as_deref(), Some(":1"));
    }

    #[test]
    fn still_mapped_dismiss_records_become_errors() {
        let mut errors = Vec::new();
        append_still_mapped_errors(
            &mut errors,
            &[DialogInfo {
                window_id: "0x1".into(),
                requested_window_id: None,
                resolved_window_id: None,
                display: None,
                title: "".into(),
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                child: Some("0x2".into()),
                action: Some("alt-o".into()),
                still_mapped: Some(true),
            }],
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("0x1"));
        assert!(errors[0].contains("alt-o"));
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
        // Backward compatibility: pid and visible default to None/false when absent
        assert_eq!(w.pid, None);
        assert!(!w.visible);
    }

    #[test]
    fn window_info_parses_with_pid_and_visible() {
        let line = r#"{"frame_id":"0x400001","window_id":"0x400002","dismiss_id":"0x400002","title":"Save Changes","class":["virtuoso"],"geometry":{"x":100,"y":200,"w":300,"h":100},"pid":12345,"visible":true}"#;
        let w: WindowInfo = serde_json::from_str(line).expect("parse");
        assert_eq!(w.pid, Some(12345));
        assert!(w.visible);
        assert_eq!(w.geometry.w, 300);
    }

    #[test]
    fn window_info_backward_compat_old_json_without_pid_visible() {
        // Old helper output without pid/visible fields must still parse
        let old = r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"Dialog","class":["virtuoso"],"geometry":{"x":0,"y":0,"w":200,"h":100}}"#;
        let w: WindowInfo = serde_json::from_str(old).expect("parse");
        assert_eq!(w.frame_id, "0x1");
        assert_eq!(w.window_id, "0x2");
        assert_eq!(w.pid, None);
        assert!(!w.visible);
    }

    #[test]
    fn window_info_pid_is_optional_positive_integer() {
        // Positive PID is preserved
        let with_pid = r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"","class":[],"geometry":{"x":0,"y":0,"w":1,"h":1},"pid":99999}"#;
        let w: WindowInfo = serde_json::from_str(with_pid).expect("parse");
        assert_eq!(w.pid, Some(99999));

        // Zero PID is normalized to None (a window cannot have PID 0)
        let zero_pid = r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"","class":[],"geometry":{"x":0,"y":0,"w":1,"h":1},"pid":0}"#;
        let z: WindowInfo = serde_json::from_str(zero_pid).expect("parse");
        assert_eq!(z.pid, None);

        // Absent pid remains None
        let no_pid = r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"","class":[],"geometry":{"x":0,"y":0,"w":1,"h":1}}"#;
        let n: WindowInfo = serde_json::from_str(no_pid).expect("parse");
        assert_eq!(n.pid, None);
    }

    // =============================================================================
    // resolve_unique_window tests — strict window identity resolution (Task 1)
    // =============================================================================

    fn mk_window(window_id: &str, pid: Option<u32>, display: &str, title: &str) -> WindowInfo {
        WindowInfo {
            frame_id: window_id.to_string(),
            window_id: window_id.to_string(),
            dismiss_id: window_id.to_string(),
            display: Some(display.to_string()),
            xauthority: None,
            title: title.to_string(),
            class: vec![],
            geometry: Geometry {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            },
            pid,
            visible: true,
        }
    }

    #[test]
    fn resolve_unique_window_zero_matches_returns_not_found() {
        let windows = vec![mk_window("0x1", Some(99999), ":0", "ADE Explorer")];
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let err = result.expect_err("expected error");
        assert!(
            matches!(err, VirtuosoError::NotFound(_)),
            "expected NotFound, got {err}"
        );
    }

    #[test]
    fn resolve_unique_window_multiple_matches_returns_conflict() {
        let windows = vec![
            mk_window("0x1", Some(12345), ":0", "ADE Explorer"),
            mk_window("0x2", Some(12345), ":0", "Virtuoso Schematic"),
        ];
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let err = result.expect_err("expected error");
        assert!(
            matches!(err, VirtuosoError::Conflict(_)),
            "expected Conflict, got {err}"
        );
    }

    #[test]
    fn resolve_unique_window_zero_pid_rejected() {
        let windows = vec![mk_window("0x1", Some(0), ":0", "ADE Explorer")];
        // Positive PID required; zero-PID windows must not match
        let result = resolve_unique_window(&windows, 0, ":0", None);
        let err = result.expect_err("expected error");
        assert!(
            matches!(err, VirtuosoError::NotFound(_)),
            "expected NotFound for zero PID, got {err}"
        );
    }

    #[test]
    fn resolve_unique_window_none_pid_rejected() {
        let windows = vec![mk_window("0x1", None, ":0", "ADE Explorer")];
        // Positive PID required; None-PID windows must not match
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let err = result.expect_err("expected error");
        assert!(
            matches!(err, VirtuosoError::NotFound(_)),
            "expected NotFound for None PID, got {err}"
        );
    }

    #[test]
    fn resolve_unique_window_exact_match_returns_window() {
        let windows = vec![mk_window("0x1", Some(12345), ":0", "ADE Explorer")];
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let win = result.expect("expected Ok");
        assert_eq!(win.window_id, "0x1");
    }

    #[test]
    fn resolve_unique_window_display_mismatch_returns_not_found() {
        let windows = vec![mk_window("0x1", Some(12345), ":1", "ADE Explorer")];
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let err = result.expect_err("expected error");
        assert!(
            matches!(err, VirtuosoError::NotFound(_)),
            "expected NotFound for display mismatch, got {err}"
        );
    }

    #[test]
    fn resolve_unique_window_title_narrows_without_requiring() {
        let windows = vec![
            mk_window("0x1", Some(12345), ":0", "ADE Explorer"),
            mk_window("0x2", Some(12345), ":0", "Virtuoso Schematic"),
        ];
        // Without title: multiple matches → Conflict
        let result = resolve_unique_window(&windows, 12345, ":0", None);
        let err = result.expect_err("expected error");
        assert!(matches!(err, VirtuosoError::Conflict(_)));

        // With title that matches one: returns that window
        let result = resolve_unique_window(&windows, 12345, ":0", Some("ADE"));
        let win = result.expect("expected Ok");
        assert_eq!(win.window_id, "0x1");

        // With title matching none: NotFound
        let result = resolve_unique_window(&windows, 12345, ":0", Some("NoMatch"));
        let err = result.expect_err("expected error");
        assert!(matches!(err, VirtuosoError::NotFound(_)));
    }

    #[test]
    fn resolve_unique_window_partial_title_match_suffices() {
        // Title substring match should succeed
        let windows = vec![mk_window(
            "0x1",
            Some(12345),
            ":0",
            "ADE Assembler Editing: LIB CELL schematic",
        )];
        let result = resolve_unique_window(&windows, 12345, ":0", Some("ADE Assembler"));
        let win = result.expect("expected Ok");
        assert_eq!(win.window_id, "0x1");
    }

    #[test]
    fn parse_window_list_backfills_display_and_xauthority() {
        // Regression: the helper emits NO display/xauthority fields (see
        // resources/x11_dismiss_dialog.py::discover_windows). If a caller
        // forgets to backfill them, resolve_unique_window's exact DISPLAY
        // match compares against None and every action returns not_found.
        let line = r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"ADE Explorer","class":["virtuoso"],"geometry":{"x":0,"y":0,"w":800,"h":600},"pid":12345,"visible":true}"#;
        let windows = parse_window_list(line, ":99", Some(&"/tmp/.Xauthority".to_string()));

        assert_eq!(windows.len(), 1, "helper line must parse");
        assert_eq!(windows[0].display.as_deref(), Some(":99"));
        assert_eq!(windows[0].xauthority.as_deref(), Some("/tmp/.Xauthority"));

        // The backfilled window must survive the strict PID+DISPLAY check that
        // `action_x11` performs before running any xdotool command.
        let resolved = resolve_unique_window(&windows, 12345, ":99", None)
            .expect("backfilled display must satisfy resolve_unique_window");
        assert_eq!(resolved.window_id, "0x2");
    }

    #[test]
    fn parse_window_list_skips_unparsable_lines() {
        let stdout = "not json\n{}\n";
        let windows = parse_window_list(stdout, ":99", None);
        assert!(windows.is_empty(), "malformed lines must be dropped");
    }

    // =============================================================================
    // Scratch path tests — user-isolated X11 helper directory (Task 1)
    // =============================================================================

    #[test]
    fn x11_remote_dir_includes_user_isolation() {
        // New format: /tmp/virtuoso_bridge_<user>/<client>/x11
        let dir = x11_remote_dir_with_user("testuser", "my-client");
        let components: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        assert!(
            dir.starts_with("/tmp/virtuoso_bridge_"),
            "path must use new user-isolated format: {dir}"
        );
        // Must have at least: tmp, virtuoso_bridge_<user>, <client>, x11
        assert!(
            components.len() >= 4,
            "expected /tmp/virtuoso_bridge_<user>/<client>/x11 structure, got {dir}"
        );
        assert_eq!(components.last(), Some(&"x11"));
    }

    #[test]
    fn x11_remote_dir_rejects_dotdot_in_user() {
        let dir = x11_remote_dir_with_user("..", "client");
        assert!(!dir.contains(".."), "dotdot must be sanitized: {dir}");
        assert!(dir.starts_with("/tmp/virtuoso_bridge_"));
    }

    #[test]
    fn x11_remote_dir_rejects_dotdot_in_client() {
        let dir = x11_remote_dir_with_user("user", "../../etc/passwd");
        // .. must be sanitized (replaced with _), not present as ..
        assert!(!dir.contains(".."), "dotdot must be sanitized: {dir}");
        // Path must still be valid and under /tmp/
        assert!(dir.starts_with("/tmp/virtuoso_bridge_"));
    }

    #[test]
    #[should_panic(expected = "empty user is not allowed")]
    fn x11_remote_dir_rejects_empty_user() {
        // Empty user is not allowed — must be resolved via `id -un` first
        let _dir = x11_remote_dir_with_user("", "client");
    }

    #[test]
    fn x11_remote_dir_rejects_empty_client() {
        let dir = x11_remote_dir_with_user("user", "");
        let components: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        assert!(
            !dir.contains("//"),
            "must not have empty path components: {dir}"
        );
        assert!(
            components.iter().all(|s| !s.is_empty()),
            "all components must be nonempty: {components:?}"
        );
    }

    #[test]
    fn x11_remote_dir_sanitizes_special_chars_in_user() {
        let dir = x11_remote_dir_with_user("user!@#$%^&*()", "client");
        // Only alphanumerics, underscore, hyphen allowed
        let user_part = dir.split('/').find(|s| s.starts_with("virtuoso_bridge_"));
        if let Some(part) = user_part {
            let suffix = &part["virtuoso_bridge_".len()..];
            assert!(
                suffix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "user part must be sanitized: {suffix}"
            );
            assert!(!suffix.contains('!'));
            assert!(!suffix.contains('@'));
        }
    }

    #[test]
    fn x11_remote_dir_sanitizes_special_chars_in_client() {
        let dir = x11_remote_dir_with_user("user", "client!@#$");
        let client_part = dir.split('/').filter(|s| !s.is_empty()).nth(2);
        if let Some(part) = client_part {
            assert!(
                part.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "client part must be sanitized: {part}"
            );
        }
    }

    #[test]
    fn local_transport_runs_harmless_command_without_remote_host() {
        // Construct a minimal Config with no remote_host directly to avoid dotenv contamination.
        let config = minimal_config_without_remote_host();
        let transport = transport_for_config(&config).expect("local transport created");
        let req = crate::transport::contract::CommandRequest::untimed("echo hello");
        let result = transport.run_command(&req).expect("command should succeed");
        assert!(result.success);
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn local_transport_upload_text_creates_file() {
        let config = minimal_config_without_remote_host();
        let transport = transport_for_config(&config).expect("local transport created");
        let tmp = std::env::temp_dir().join(format!("x11_local_test_{}.txt", std::process::id()));
        let req = crate::transport::contract::UploadTextRequest::untimed(
            "content",
            tmp.to_str().unwrap(),
        );
        transport
            .upload_text(&req)
            .expect("upload_text should succeed");
        let content = std::fs::read_to_string(&tmp).expect("file should exist");
        assert_eq!(content, "content");
        std::fs::remove_file(tmp).ok();
    }

    #[test]
    fn local_transport_respects_expired_deadline() {
        let config = minimal_config_without_remote_host();
        let transport = transport_for_config(&config).expect("local transport created");
        // A deadline that is already expired should return QueueTimeout.
        let past =
            crate::transport::contract::Deadline::from_now(std::time::Duration::from_secs(0));
        // Override deadline to already-expired by using a negative duration (Deadline::from_now with 0 is already expired since it sets to Instant::now()).
        let req = crate::transport::contract::CommandRequest::new("echo hello", past);
        let err = transport
            .run_command(&req)
            .expect_err("should fail on expired deadline");
        match err {
            crate::transport::contract::TransportError::QueueTimeout { .. } => {}
            other => panic!("expected QueueTimeout, got {:?}", other),
        }
    }

    // =============================================================================
    // Recording-transport tests for ensure_helper_uploaded (Task 1)
    // =============================================================================

    /// A transport that records every command it receives for inspection.
    #[derive(Default)]
    struct RecordingTransport {
        commands: std::sync::Mutex<Vec<String>>,
        responses: std::sync::Mutex<Vec<CommandResult>>,
    }

    impl RecordingTransport {
        fn new() -> Self {
            Self::default()
        }
        /// Enqueue a response for the next command(s).
        fn enqueue_response(&self, r: CommandResult) {
            self.responses.lock().unwrap().push(r);
        }
    }

    impl RemoteTransport for RecordingTransport {
        fn test_connection(
            &self,
            _deadline: crate::transport::contract::Deadline,
        ) -> std::result::Result<bool, crate::transport::contract::TransportError> {
            Ok(true)
        }

        fn run_command(
            &self,
            req: &CommandRequest,
        ) -> std::result::Result<CommandResult, crate::transport::contract::TransportError>
        {
            self.commands.lock().unwrap().push(req.command.clone());
            let responses = &mut self.responses.lock().unwrap();
            if responses.is_empty() {
                return Ok(CommandResult {
                    exit_status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    success: true,
                    duration: Duration::ZERO,
                });
            }
            Ok(responses.remove(0))
        }

        fn upload_text(
            &self,
            _req: &UploadTextRequest,
        ) -> std::result::Result<(), crate::transport::contract::TransportError> {
            Ok(())
        }

        fn upload_file(
            &self,
            _req: &crate::transport::contract::UploadFileRequest,
        ) -> std::result::Result<(), crate::transport::contract::TransportError> {
            Ok(())
        }

        fn download_file(
            &self,
            _req: &crate::transport::contract::DownloadFileRequest,
        ) -> std::result::Result<(), crate::transport::contract::TransportError> {
            Err(
                crate::transport::contract::TransportError::UnsupportedOperation(
                    "download_file not supported".into(),
                ),
            )
        }

        fn download_dir(
            &self,
            _req: &crate::transport::contract::DownloadDirRequest,
        ) -> std::result::Result<(), crate::transport::contract::TransportError> {
            Err(
                crate::transport::contract::TransportError::UnsupportedOperation(
                    "download_dir not supported".into(),
                ),
            )
        }
    }

    fn mk_result(stdout: &str, stderr: &str, exit_status: i32) -> CommandResult {
        CommandResult {
            exit_status,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            success: exit_status == 0,
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn list_dialogs_exit1_empty_means_no_dialogs_not_error() {
        // The helper exits 1 with no output to signal "no dialogs". This is the
        // healthy state and must come back as an empty list, not a helper error.
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result("", "", 1)); // helper: no dialogs → exit 1
        let (_, dialogs) = list_dialogs(&transport, "client1", Some("user1"), Some(":99"))
            .expect("list_dialogs should succeed");
        assert!(dialogs.is_empty(), "expected no dialogs, got {dialogs:?}");
    }

    #[test]
    fn list_dialogs_with_dialog_exit0_returns_dialog() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result(
            "{\"window_id\":\"0x1\",\"title\":\"Save?\",\"x\":10,\"y\":10,\"w\":300,\"h\":120}\n",
            "",
            0,
        ));
        let (_, dialogs) = list_dialogs(&transport, "client1", Some("user1"), Some(":99"))
            .expect("list_dialogs should succeed");
        assert_eq!(dialogs.len(), 1);
        assert_eq!(dialogs[0].window_id, "0x1");
    }

    #[test]
    fn list_dialogs_real_failure_exit2_surfaces_helper_error() {
        // A genuine failure (exit 2, stderr) must still surface as helper-error
        // so callers can distinguish it from the healthy "no dialogs" state.
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result("", "xwininfo: unable to open display", 2));
        let (_, dialogs) = list_dialogs(&transport, "client1", Some("user1"), Some(":99"))
            .expect("list_dialogs should succeed");
        assert_eq!(dialogs.len(), 1);
        assert_eq!(dialogs[0].window_id, "helper-error");
        assert!(dialogs[0].title.contains("x11 helper error"));
    }

    #[test]
    fn list_windows_exit1_empty_means_no_windows_not_error() {
        // The helper exits 1 with no output to signal "no Virtuoso windows".
        // This is the healthy state and must come back as an empty list, not a
        // helper failure (same semantics as list_dialogs).
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result("", "", 1)); // helper: no windows → exit 1
        let (_, windows) = list_windows(&transport, "client1", Some("user1"), Some(":99"))
            .expect("list_windows should succeed");
        assert!(windows.is_empty(), "expected no windows, got {windows:?}");
    }

    #[test]
    fn list_windows_with_window_exit0_returns_window() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result(
            r#"{"frame_id":"0x1","window_id":"0x2","dismiss_id":"0x2","title":"Virtuoso","class":["virtuoso"],"geometry":{"x":0,"y":0,"w":800,"h":600},"pid":12345,"visible":true}"#,
            "",
            0,
        ));
        let (_, windows) = list_windows(&transport, "client1", Some("user1"), Some(":99"))
            .expect("list_windows should succeed");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_id, "0x2");
    }

    #[test]
    fn list_windows_real_failure_exit2_surfaces_helper_error() {
        // A genuine failure (exit 2, stderr) must still be surfaced as an
        // error so callers can distinguish "no Virtuoso windows" from "x11
        // helper crashed".
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // install -d
        transport.enqueue_response(mk_result("", "xwininfo: unable to open display", 2));
        let err = list_windows(&transport, "client1", Some("user1"), Some(":99"))
            .expect_err("expected an error for real helper failure");
        assert!(err.to_string().contains("x11 helper failed"));
    }

    #[test]
    fn resolve_effective_user_calls_id_un_and_returns_sanitized_username() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("testuser\n", "", 0));

        let user = resolve_effective_user(&transport).expect("expected Ok");
        assert_eq!(user, "testuser");

        let cmds = transport.commands.lock().unwrap();
        assert!(
            cmds.contains(&"id -un".to_string()),
            "id -un must be called"
        );
    }

    #[test]
    fn resolve_effective_user_rejects_empty_output() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0));

        let err = resolve_effective_user(&transport).expect_err("expected error");
        assert!(err.to_string().contains("empty username"));
    }

    #[test]
    fn resolve_effective_user_rejects_failure() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "no such user", 1));

        let err = resolve_effective_user(&transport).expect_err("expected error");
        assert!(err.to_string().contains("`id -un` failed"));
    }

    #[test]
    fn resolve_effective_user_rejects_multiline_output() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("user\nextra\n", "", 0));

        let err = resolve_effective_user(&transport).expect_err("expected error");
        assert!(err.to_string().contains("multi-line"));
    }

    #[test]
    fn ensure_helper_uploaded_with_explicit_user_skips_id_un() {
        let transport = RecordingTransport::new();
        // No responses needed — id -un won't be called

        let result = ensure_helper_uploaded(&transport, Some("alice"), "client1");
        assert!(result.is_ok(), "expected Ok, got {result:?}");

        let cmds = transport.commands.lock().unwrap();
        assert!(
            !cmds.iter().any(|c| c.contains("id -un")),
            "id -un must NOT be called when explicit user is provided"
        );
        // Must call install -d -m 700
        assert!(
            cmds.iter().any(|c| c.contains("install -d -m 700")),
            "install -d -m 700 must be called"
        );
    }

    #[test]
    fn ensure_helper_uploaded_without_explicit_user_calls_id_un() {
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("bob\n", "", 0));

        let result = ensure_helper_uploaded(&transport, None, "client1");
        assert!(result.is_ok(), "expected Ok, got {result:?}");

        let cmds = transport.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("id -un")),
            "id -un must be called when no explicit user"
        );
        assert!(
            cmds.iter().any(|c| c.contains("install -d -m 700")),
            "install -d -m 700 must be called after id -un"
        );
        // install -d -m 700 must come AFTER id -un
        let id_un_idx = cmds.iter().position(|c| c.contains("id -un")).unwrap();
        let install_idx = cmds
            .iter()
            .position(|c| c.contains("install -d -m 700"))
            .unwrap();
        assert!(
            install_idx > id_un_idx,
            "install -d -m 700 must follow id -un"
        );
    }

    #[test]
    fn ensure_helper_uploaded_rejects_empty_user_fallback() {
        // When explicit user is empty string, id -un is called and must succeed
        let transport = RecordingTransport::new();
        transport.enqueue_response(mk_result("", "", 0)); // empty from id -un

        let err =
            ensure_helper_uploaded(&transport, Some(""), "client1").expect_err("expected error");
        assert!(err.to_string().contains("empty username"));
    }

    // =============================================================================
    // Task 2: action_x11 tests — fixed-semantics X11 action command
    // =============================================================================

    /// Operation tokens allowed in action_x11.
    #[test]
    fn action_x11_operation_is_allowlisted() {
        let allowed = [
            "activate",
            "key",
            "type",
            "click-rel",
            "drag-rel",
            "screenshot",
            "wait",
        ];
        for op in allowed {
            assert!(
                matches!(
                    op,
                    "activate" | "key" | "type" | "click-rel" | "drag-rel" | "screenshot" | "wait"
                ),
                "operation '{}' must be in allowlist",
                op
            );
        }
    }

    #[test]
    fn action_x11_requires_window_id_not_empty() {
        // Empty window_id should be rejected by validate_action_params
        let result =
            validate_action_params("", 12345, ":0", "activate", None, None, None, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("window_id"));
    }

    #[test]
    fn action_x11_rejects_zero_pid() {
        // Zero PID should be rejected
        let result =
            validate_action_params("0x123", 0, ":0", "activate", None, None, None, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("positive PID"));
    }

    #[test]
    fn action_x11_rejects_empty_display() {
        // Empty display should be rejected
        let result =
            validate_action_params("0x123", 12345, "", "activate", None, None, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn action_x11_key_requires_text_parameter() {
        // key operation requires non-empty text — passing None must be rejected
        let result =
            validate_action_params("0x123", 12345, ":0", "key", None, None, None, None, None);
        assert!(result.is_err(), "key requires --text");
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "key",
            None,
            None,
            Some(""),
            None,
            None,
        );
        assert!(result.is_err(), "key requires non-empty --text");
    }

    #[test]
    fn action_x11_type_returns_text_length_not_text() {
        // type operation sanitizes output to length only
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "type",
            None,
            None,
            Some("secret"),
            None,
            None,
        );
        assert!(result.is_ok(), "type should accept non-empty text");
        // The sanitized details should contain length, not the text itself
        let details = result.unwrap();
        assert!(
            details.contains("text_length"),
            "details should contain text_length, not actual text"
        );
        assert!(
            !details.contains("secret"),
            "details must not contain actual text"
        );
    }

    #[test]
    fn action_x11_click_rel_requires_x_y() {
        // click-rel requires x and y
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "click-rel",
            Some(10),
            Some(20),
            None,
            None,
            None,
        );
        assert!(result.is_ok(), "click-rel with x,y should be ok");
    }

    #[test]
    fn action_x11_drag_rel_requires_x_y() {
        // drag-rel requires x and y
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "drag-rel",
            Some(10),
            Some(20),
            None,
            None,
            None,
        );
        assert!(result.is_ok(), "drag-rel with x,y should be ok");
    }

    #[test]
    fn action_x11_screenshot_requires_output_dir() {
        // screenshot requires output_dir — missing must be rejected
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "screenshot",
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            result.is_err(),
            "screenshot without output_dir should be rejected"
        );
        // provided output_dir is accepted
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "screenshot",
            None,
            None,
            None,
            None,
            Some("/tmp/output"),
        );
        assert!(result.is_ok(), "screenshot with output_dir should be ok");
    }

    #[test]
    fn action_x11_screenshot_rejects_path_traversal() {
        // screenshot path must be within output_dir (no ..)
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "screenshot",
            None,
            None,
            None,
            None,
            Some("/tmp/../../../etc"),
        );
        assert!(
            result.is_err(),
            "screenshot path with .. should be rejected"
        );
    }

    // ---------------------------------------------------------------------------
    // validate_png_artifact: rejection branches (review #1 / #5)
    // ---------------------------------------------------------------------------

    fn write_temp_png(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, bytes).expect("write temp png");
        p
    }

    #[test]
    fn validate_png_artifact_accepts_valid_png() {
        let mut data = Vec::new();
        data.extend_from_slice(super::PNG_MAGIC);
        data.resize(128, 0); // >= 64 bytes
        let p = write_temp_png(&data, "vcli_test_png_valid.png");
        let res = super::validate_png_artifact(&p);
        assert!(res.is_ok(), "valid PNG should pass: {res:?}");
        let (size, hash) = res.unwrap();
        assert_eq!(size, 128);
        assert_eq!(hash.len(), 64);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn validate_png_artifact_rejects_tiny_file() {
        let data = vec![0u8; 10];
        let p = write_temp_png(&data, "vcli_test_png_tiny.png");
        let res = super::validate_png_artifact(&p);
        assert!(res.is_err(), "file < 64 bytes must be rejected");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn validate_png_artifact_rejects_wrong_magic() {
        let mut data = vec![0u8; 128];
        data[0..4].copy_from_slice(b"NOPE");
        let p = write_temp_png(&data, "vcli_test_png_badmagic.png");
        let res = super::validate_png_artifact(&p);
        assert!(res.is_err(), "wrong magic must be rejected");
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn validate_png_artifact_rejects_symlink() {
        // Locks review #1: symlink_metadata (not metadata) must make the
        // is_symlink() branch reachable.
        let mut data = Vec::new();
        data.extend_from_slice(super::PNG_MAGIC);
        data.resize(128, 0);
        let target = write_temp_png(&data, "vcli_test_png_symlink_target.png");
        let link = std::env::temp_dir().join("vcli_test_png_symlink.png");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        let res = super::validate_png_artifact(&link);
        assert!(res.is_err(), "symlink must be rejected");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);
    }

    // ---------------------------------------------------------------------------
    // MouseButtonGuard: best-effort mouseup on drop (review #5)
    // ---------------------------------------------------------------------------

    #[test]
    fn mouse_button_guard_drops_best_effort_mouseup_when_armed() {
        let win = WindowInfo {
            frame_id: "0x1".into(),
            window_id: "0x2".into(),
            dismiss_id: "0x3".into(),
            display: Some(":0".into()),
            xauthority: None,
            title: "t".into(),
            class: vec![],
            geometry: Geometry {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            },
            pid: Some(12345),
            visible: true,
        };
        let t = RecordingTransport::new();
        {
            let mut g = super::MouseButtonGuard::new(&t, "env DISPLAY=:0 ", &win, Some(1));
            g.arm();
            // guard dropped at end of scope
        }
        let cmds = t.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("mouseup")),
            "armed MouseButtonGuard must issue best-effort mouseup on drop; got {cmds:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // LocalTransport::download_file (review #3)
    // ---------------------------------------------------------------------------

    #[test]
    fn local_transport_download_file_copies_local_file() {
        let src = std::env::temp_dir().join("vcli_test_dl_src.png");
        let dst = std::env::temp_dir().join("vcli_test_dl_dst.png");
        let _ = std::fs::remove_file(&dst);
        std::fs::write(&src, b"hello-world-png-contents").expect("write src");
        let req = crate::transport::contract::DownloadFileRequest::untimed(
            src.display().to_string(),
            dst.clone(),
        );
        let res = super::LocalTransport::new().download_file(&req);
        assert!(
            res.is_ok(),
            "LocalTransport::download_file should copy: {res:?}"
        );
        let copied = std::fs::read(&dst).expect("read dst");
        assert_eq!(copied, b"hello-world-png-contents");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }

    #[test]
    fn action_x11_wait_requires_condition() {
        // wait requires a condition expression
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "wait",
            None,
            None,
            Some("visible"),
            None,
            None,
        );
        assert!(result.is_ok(), "wait with condition should be ok");
    }

    fn wait_win_json(id: &str, title: &str) -> String {
        serde_json::json!({
            "frame_id": id,
            "window_id": id,
            "dismiss_id": id,
            "display": ":0",
            "title": title,
            "class": [],
            "geometry": {"x": 0, "y": 0, "w": 100, "h": 100},
            "pid": 123,
            "visible": true
        })
        .to_string()
    }

    #[test]
    fn wait_for_window_pattern_initial_snapshot_matches() {
        let transport = RecordingTransport::new();
        let initial = parse_window_list(
            &wait_win_json("0x100", "Schematic Editor bandgap"),
            ":0",
            None,
        );
        let (matched, outs) = wait_for_window_pattern(
            &transport,
            "/tmp/helper.py",
            ":0",
            None,
            "Schematic",
            Duration::from_secs(1),
            &initial,
        )
        .expect("wait should succeed");
        assert!(matched, "initial snapshot already matches");
        assert!(outs.is_empty(), "no polling needed when initial matches");
    }

    #[test]
    fn wait_for_window_pattern_matches_after_poll() {
        let transport = RecordingTransport::new();
        // First poll returns a window that does NOT match.
        transport.enqueue_response(mk_result(&wait_win_json("0x100", "CIW Log"), "", 0));
        // Second poll returns the matching window.
        transport.enqueue_response(mk_result(&wait_win_json("0x200", "Dialog: Save As"), "", 0));
        let initial = parse_window_list("", ":0", None);
        let (matched, outs) = wait_for_window_pattern(
            &transport,
            "/tmp/helper.py",
            ":0",
            None,
            "Save As",
            Duration::from_secs(5),
            &initial,
        )
        .expect("wait should succeed");
        assert!(matched, "poll should observe the matching window");
        assert_eq!(outs.len(), 2, "two polling rounds expected");
    }

    #[test]
    fn wait_for_window_pattern_timeout_returns_unmatched() {
        let transport = RecordingTransport::new();
        // Even with a helper success, no window matches -> timeout -> false.
        transport.enqueue_response(mk_result(&wait_win_json("0x100", "CIW Log"), "", 0));
        let initial = parse_window_list("", ":0", None);
        let (matched, _outs) = wait_for_window_pattern(
            &transport,
            "/tmp/helper.py",
            ":0",
            None,
            "NoSuchWindow",
            Duration::from_millis(50),
            &initial,
        )
        .expect("wait should return without error");
        assert!(!matched, "timeout with no match must report unmatched");
    }

    #[test]
    fn action_x11_unknown_operation_rejected() {
        // Unknown operation should be rejected
        let result = validate_action_params(
            "0x123",
            12345,
            ":0",
            "delete everything",
            None,
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("operation")
                || err.to_string().contains("activate")
        );
    }

    // =============================================================================
    // P1-6: argv-recording integration test — assert generated xdotool/import
    // command sequences match the fixed, position-checked shape. (Task 1-6)
    // =============================================================================

    fn mk_action_window(wid: &str) -> WindowInfo {
        WindowInfo {
            frame_id: wid.into(),
            window_id: wid.into(),
            dismiss_id: wid.into(),
            display: Some(":0".into()),
            xauthority: None,
            title: "test cellview".into(),
            class: vec![],
            geometry: Geometry {
                x: 100,
                y: 100,
                w: 1920,
                h: 1080,
            },
            pid: Some(12345),
            visible: true,
        }
    }

    #[test]
    fn build_xdotool_actions_activate_argv_shape() {
        let action = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::Activate,
            None,
            None,
            None,
            None,
        )
        .expect("activate ok");
        assert_eq!(action.len(), 1);
        let (sub, argv) = &action[0];
        assert_eq!(sub, "windowraise");
        // windowraise takes NO options — neither --sync nor --window exist for
        // it in any xdotool release, so the window id is purely positional.
        assert_eq!(
            argv,
            &["windowraise", "0x400002"],
            "activate argv must be `windowraise <id>`"
        );
        assert!(
            !argv.iter().any(|a| a == "--sync" || a == "--window"),
            "regression: windowraise accepts neither --sync nor --window"
        );
    }

    #[test]
    fn build_xdotool_actions_click_rel_argv_shape_with_button() {
        // P2-1 fix: click-rel now expands to TWO commands — mousemove then click.
        // Previously it used "--move-to-cursor" which is not a valid xdotool flag.
        let action = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::ClickRel,
            Some(10),
            Some(20),
            Some(1),
            None,
        )
        .expect("click-rel ok");
        assert_eq!(action.len(), 2, "click-rel must produce 2 commands");

        // Command 0: mousemove --window ID -- <x> <y>
        assert_eq!(action[0].0, "mousemove");
        assert_eq!(
            action[0].1,
            &["mousemove", "--window", "0x400002", "--", "10", "20"],
            "click-rel step 1 must move cursor to window-relative point"
        );

        // Command 1: click --window ID N (button is positional)
        assert_eq!(action[1].0, "click");
        assert_eq!(
            action[1].1,
            &["click", "--window", "0x400002", "1"],
            "click-rel step 2 must click at the current position"
        );
        assert!(
            !action[1].1.iter().any(|a| a == "--button"),
            "regression: click has no --button option"
        );
    }

    #[test]
    fn build_xdotool_actions_click_rel_button_three_argv() {
        // Right-click (button 3) — confirms button value passes through verbatim.
        let action = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::ClickRel,
            Some(5),
            Some(-5),
            Some(3),
            None,
        )
        .expect("click-rel ok");
        let (_, argv) = &action[1]; // second command (click)
        assert!(
            !argv.iter().any(|a| a == "--button"),
            "regression: click has no --button option"
        );
        assert_eq!(
            argv.last().expect("argv non-empty"),
            "3",
            "button=3 must be the trailing positional button"
        );
        // Negative y must reach the command unmodified (protected by `--` sep).
        let dash_i = action[0]
            .1
            .iter()
            .position(|a| a == "--")
            .expect("-- separator present");
        assert_eq!(
            action[0].1[dash_i + 2],
            "-5",
            "negative y must pass through"
        );
    }

    #[test]
    fn build_xdotool_actions_click_rel_requires_x_y() {
        // click-rel without --x or --y must be rejected (can't click "nowhere").
        let err = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::ClickRel,
            None,
            None,
            None,
            None,
        )
        .expect_err("click-rel without x,y must be rejected");
        assert!(
            err.to_string().contains("--x") && err.to_string().contains("--y"),
            "error should mention both --x and --y, got: {err}"
        );
    }

    #[test]
    fn build_xdotool_actions_key_argv_shape() {
        let action = build_xdotools_actions_key("0x400002", "ctrl-s");
        assert_eq!(action.len(), 1);
        let (sub, argv) = &action[0];
        assert_eq!(sub, "key");
        // Hyphenated chords (`ctrl-s`) are normalized to xdotool's `+` form.
        assert_eq!(argv, &["key", "--window", "0x400002", "ctrl+s"]);
        assert!(
            !argv.windows(2).any(|w| w[0] == "--window" && w[1] == "key"),
            "regression: --window must come AFTER subcommand"
        );
    }

    #[test]
    fn build_xdotool_actions_key_normalizes_hyphen_chords_across_tokens() {
        // `ctrl-s Return` and `ctrl-shift-x` (multi-token + hyphen chord) both
        // normalize `-` → `+`; keysyms with underscores (KP_Subtract) are
        // untouched since they contain no hyphen.
        let action = build_xdotools_actions_key("0x400002", "ctrl-shift-x Return KP_Subtract");
        let (sub, argv) = &action[0];
        assert_eq!(sub, "key");
        assert_eq!(
            argv,
            &[
                "key",
                "--window",
                "0x400002",
                "ctrl+shift+x",
                "Return",
                "KP_Subtract"
            ]
        );
    }

    fn build_xdotools_actions_key(wid: &str, keys: &str) -> Vec<(String, Vec<String>)> {
        build_xdotool_actions(
            &mk_action_window(wid),
            X11Operation::Key,
            None,
            None,
            None,
            Some(keys),
        )
        .expect("key ok")
    }

    #[test]
    fn build_xdotool_actions_type_argv_shape_with_separator() {
        let action = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::Type,
            None,
            None,
            None,
            Some("CTRL+S"),
        )
        .expect("type ok");
        assert_eq!(action.len(), 1);
        let (sub, argv) = &action[0];
        assert_eq!(sub, "type");
        // xdotool type swallows --foo keys; the `--` separator is mandatory.
        assert_eq!(
            argv,
            &["type", "--window", "0x400002", "--", "CTRL+S"],
            "type argv must contain `--` separator to protect literal text"
        );
    }

    #[test]
    fn build_xdotool_actions_drag_rel_expands_to_three_commands() {
        let action = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::DragRel,
            Some(50),
            Some(-10),
            Some(2),
            None,
        )
        .expect("drag-rel ok");
        // drag-rel must expand to three commands, NOT one malformed string.
        assert_eq!(action.len(), 3, "drag-rel must produce 3 commands");

        // mousedown --window ID N   (button is positional)
        assert_eq!(action[0].0, "mousedown");
        assert_eq!(action[0].1, vec!["mousedown", "--window", "0x400002", "2"]);

        // Relative pointer motion is the separate `mousemove_relative` command:
        // `--relative` is not a mousemove option (it belongs to the window
        // command `windowmove`), and mousemove_relative takes no --window
        // because a delta needs no window-relative origin. `--` protects the
        // negative delta from being parsed as a flag.
        assert_eq!(action[1].0, "mousemove_relative");
        assert_eq!(
            action[1].1,
            vec!["mousemove_relative", "--", "50", "-10"],
            "drag-rel motion must use mousemove_relative with x,y deltas"
        );

        // mouseup --window ID N
        assert_eq!(action[2].0, "mouseup");
        assert_eq!(action[2].1, vec!["mouseup", "--window", "0x400002", "2"]);

        // No command may emit the nonexistent --button option...
        for (sub, argv) in &action {
            assert!(
                !argv.iter().any(|a| a == "--button"),
                "{sub} must not use the nonexistent --button option"
            );
        }
        // ...and the press/release pair must target the same window. The
        // relative move in between is global pointer motion and by design
        // carries no window id.
        for (sub, argv) in action.iter().filter(|(s, _)| s != "mousemove_relative") {
            let wid_i = argv
                .iter()
                .position(|a| a == "--window")
                .unwrap_or_else(|| panic!("{sub} missing --window"));
            assert_eq!(argv[wid_i + 1], "0x400002", "window id at fixed position");
        }
    }

    #[test]
    fn build_xdotool_actions_drag_rel_rejects_missing_y() {
        let err = build_xdotool_actions(
            &mk_action_window("0x400002"),
            X11Operation::DragRel,
            Some(10),
            None,
            None,
            None,
        )
        .expect_err("y must be required");
        assert!(err.to_string().contains("--y"), "error should mention --y");
    }

    /// Construct a Config with no remote_host set, avoiding `from_env` / dotenv.
    fn minimal_config_without_remote_host() -> Config {
        Config {
            profile: None,
            remote_host: None,
            remote_user: None,
            port: 5555,
            jump_host: None,
            jump_user: None,
            ssh_port: None,
            ssh_key: None,
            ssh_config: None,
            ssh_backend: None,
            disable_control_master: false,
            timeout: 30,
            read_timeout: 120,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: Vec::new(),
            spectre_max_workers: 8,
            ssh_max_sessions: 10,
            ssh_max_bulk_sessions: 2,
            ssh_reconnect_max_attempts: 8,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 30,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 10,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: crate::config::RemoteRoles {
                gui_host: None,
                deploy_host: None,
                daemon_host: None,
                spectre_host: None,
                scratch_root: None,
            },
        }
    }
}

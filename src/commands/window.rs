use std::sync::Arc;

use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
use crate::transport::contract::RemoteTransport;
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
pub fn list(ctx: &crate::context::CommandContext) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    // Unchecked — see `symbol::inspect`. The SKILL is generated internally and
    // the capability check already ran at RPC dispatch; re-checking here locks
    // non-Admin users out of their own typed RPC (`window.list`).
    let r = client.execute_skill_unchecked(&client.window.list_windows(), None)?;
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
pub fn dismiss_dialog(
    ctx: &crate::context::CommandContext,
    action: &str,
    dry_run: bool,
) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    if dry_run {
        let r = client.execute_skill_unchecked(&client.window.get_dialog_info(), None)?;
        let raw = r.output.trim_matches('"');
        let active = r.skill_ok() && raw != "no-dialog";
        return Ok(json!({
            "dialog": if active { raw } else { "none" },
            "active": active,
            "dry_run": true,
        }));
    }
    let r = client.execute_skill_unchecked(&client.window.dismiss_dialog(action), None)?;
    let dismissed = r.skill_ok() && r.output.trim_matches('"') != "no-dialog";
    Ok(json!({
        "status": if dismissed { "dismissed" } else { "no-dialog" },
        "action": action,
    }))
}

/// List blocking dialog(s) via the X11 bypass (no keypress sent).
///
/// Useful as a "dry-run" alternative when SKILL is deadlocked and you want
/// to see what dialogs are present before deciding which action to send.
/// Works locally when VB_REMOTE_HOST is absent, otherwise via SSH.
pub fn list_dialogs_x11(
    ctx: &crate::context::CommandContext,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::transport::x11;

    let config = ctx.config();
    let runner = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let (env, dialogs) = x11::list_dialogs(
        runner.as_ref(),
        &x11::client_id_for(config),
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

/// Dismiss blocking dialog(s) via the X11 bypass.
///
/// This is the deadlock-resistant alternative to `dismiss_dialog`: it
/// finds modal dialogs with `xwininfo`, and sends keypresses
/// (`enter`/`escape`/`alt-y`/`alt-n`/`alt-o`) via `XTest`. The SKILL channel
/// cannot be used when a modal has stalled the CIW, so this path is independent
/// of the SKILL bridge. Works locally when VB_REMOTE_HOST is absent.
///
/// Adopted from <https://github.com/Arcadia-1/virtuoso-bridge-lite>
/// (MIT, 2026-05; helper vendored in resources/x11_dismiss_dialog.py).
pub fn dismiss_dialog_x11(
    ctx: &crate::context::CommandContext,
    action: &str,
    dry_run: bool,
    explicit_display: Option<&str>,
    window_id: Option<&str>,
) -> Result<Value> {
    use crate::transport::x11;

    if !["enter", "escape", "alt-y", "alt-n", "alt-o"].contains(&action) {
        return Err(VirtuosoError::Config(format!(
            "invalid --action '{}': must be one of enter|escape|alt-y|alt-n|alt-o",
            action
        )));
    }
    let config = ctx.config();
    let runner = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let result = x11::dismiss(
        runner.as_ref(),
        &x11::client_id_for(config),
        user,
        explicit_display,
        action,
        dry_run,
        window_id,
    )?;
    x11_dismiss_payload(
        &result,
        x11_dismiss_status(&result),
        action,
        &[("dry_run", json!(dry_run))],
    )
}

/// Serialize an X11 dismiss result into the command's JSON payload.
///
/// Shared by `dismiss_dialog_x11` and `dismiss_window_x11` so both report a
/// failure the same way. `status` is the command-specific status string;
/// `failed` always carries a non-zero `exit_code` so `main()` maps it to a
/// failing process exit status (see `exit_codes::exit_code_for_value`).
/// Every other status keeps the default exit code.
pub(crate) fn x11_dismiss_payload(
    result: &crate::transport::x11::DismissResult,
    status: &str,
    action: &str,
    extra: &[(&str, Value)],
) -> Result<Value> {
    let mut out = serde_json::to_value(result)
        .map_err(|e| VirtuosoError::Execution(format!("failed to serialize X11 result: {e}")))?;
    let obj = out.as_object_mut().ok_or_else(|| {
        VirtuosoError::Execution("X11 dismiss result is not a JSON object".into())
    })?;
    obj.insert("status".into(), json!(status));
    obj.insert("action".into(), json!(action));
    for (key, value) in extra {
        obj.insert((*key).to_string(), value.clone());
    }
    if status == "failed" {
        // A dismissal that could not be verified must not read as success.
        // Purely additive — the JSON shape dispatchers consume is unchanged.
        obj.insert("exit_code".into(), json!(crate::exit_codes::GENERAL_ERROR));
    }
    Ok(out)
}

/// Status string for an X11 dismiss result (the `dismiss-dialog` path).
///
/// `failed` means at least one dismissal could not be verified (or the helper
/// itself failed).
pub(crate) fn x11_dismiss_status(result: &crate::transport::x11::DismissResult) -> &'static str {
    if !result.errors.is_empty() {
        return "failed";
    }
    if !result.dismissed.is_empty() {
        return "dismissed";
    }
    if !result.found.is_empty() {
        return "found";
    }
    "no-dialog"
}

/// Status string for an X11 single-window dismiss result (the
/// `dismiss-window` path).
///
/// Same rule as `x11_dismiss_status`, but the target is explicit, so "no
/// window was dismissed" is `not-found` rather than `no-dialog`.
pub(crate) fn x11_dismiss_window_status(
    result: &crate::transport::x11::DismissResult,
) -> &'static str {
    if !result.errors.is_empty() {
        return "failed";
    }
    if !result.dismissed.is_empty() {
        return "dismissed";
    }
    "not-found"
}

/// List every Virtuoso-related X11 window (no keypress sent).
///
/// Unlike `list_dialogs_x11` (which only returns dialogs matching the
/// geometric modal test), this enumerates every mapped Virtuoso window
/// along with its WM frame and dismiss id. Use this when you want to see
/// what's on screen before picking a target. Works locally when VB_REMOTE_HOST
/// is absent.
pub fn list_windows_x11(
    ctx: &crate::context::CommandContext,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::transport::x11;

    let config = ctx.config();
    let runner = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let (env, windows) = x11::list_windows(
        runner.as_ref(),
        &x11::client_id_for(config),
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

/// Report the client-area geometry of one X11 window.
///
/// The geometry that `action-x11` validates `--x`/`--y` against, and the
/// space those coordinates are interpreted in. `list-windows-x11` reports the
/// window *frame* geometry instead, which includes the WM decorations — the
/// two differ by the title-bar and border widths, so a caller that mixes them
/// computes coordinates outside the client area.
pub fn geometry(
    ctx: &crate::context::CommandContext,
    window_id: &str,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::transport::x11;

    let config = ctx.config();
    let runner = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let geom = x11::client_geometry(runner.as_ref(), user, explicit_display, window_id)?;
    Ok(json!({
        "status": "success",
        "window_id": geom.window_id,
        "display": geom.display,
        "geometry": geom.geometry,
    }))
}

/// Dismiss a SPECIFIC X11 window by id. Use `list_windows_x11` to find the id first.
///
/// The window id is the `dismiss_id` field returned by `--list-windows`
/// (typically an X resource id like `0x2e01f16`). Works locally when VB_REMOTE_HOST
/// is absent.
pub fn dismiss_window_x11(
    ctx: &crate::context::CommandContext,
    window_id: Option<&str>,
    pid: Option<u32>,
    action: &str,
    explicit_display: Option<&str>,
) -> Result<Value> {
    use crate::transport::x11;

    let config = ctx.config();
    let runner = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();

    // Resolve the target window id. Precedence: explicit --window-id (or the
    // positional id) > --pid (resolve via list-windows-x11) > error.
    let id = match (window_id.filter(|s| !s.is_empty()), pid) {
        (Some(id), _) => id.to_string(),
        (None, Some(p)) => {
            // A supplied PID is only a verification elsewhere; here it is the
            // identity because no window id was given. Resolve it from the
            // live window list — the same list-windows scan the caller uses
            // to discover dismiss_ids.
            let (_env, windows) = x11::list_windows(
                runner.as_ref(),
                &x11::client_id_for(config),
                user,
                explicit_display,
            )?;
            let matches: Vec<&x11::WindowInfo> =
                windows.iter().filter(|w| w.pid == Some(p)).collect();
            match matches.len() {
                0 => {
                    return Err(VirtuosoError::NotFound(format!(
                        "no window with PID={p} (it may not expose _NET_WM_PID, or may not be a Virtuoso window)"
                    )))
                }
                1 => matches[0].dismiss_id.clone(),
                _ => {
                    return Err(VirtuosoError::Conflict(format!(
                        "multiple windows with PID={p}; pass --window-id to disambiguate"
                    )))
                }
            }
        }
        (None, None) => {
            return Err(VirtuosoError::Config(
                "window id required: pass it positionally, via --window-id, or via --pid".into(),
            ))
        }
    };

    let result = x11::dismiss_window(
        runner.as_ref(),
        &x11::client_id_for(config),
        user,
        explicit_display,
        &id,
        action,
    )?;
    let mut extra: Vec<(&str, Value)> = vec![("window_id", json!(id))];
    // Report which identity was used when resolution came from a PID.
    if pid.is_some() && window_id.filter(|s| !s.is_empty()).is_none() {
        extra.push(("resolved_by_pid", json!(pid)));
    }
    x11_dismiss_payload(&result, x11_dismiss_window_status(&result), action, &extra)
}

/// Capture a screenshot of the current (or pattern-matched) Virtuoso window.
///
/// Saves to --path as PNG. Requires IC23.1+ (hiGetWindowScreenDump).
///
/// Two capture routes, tried in order:
///
/// 1. The SKILL route grabs the X11 **root** window. It needs no window id,
///    so it works for a caller that has none — but it fails outright on X
///    servers that refuse `XGetImage` on the root window (observed on an
///    IC26.1 host: `xwd -root` fails with BadMatch, and ImageMagick reports
///    that as the misleading "missing an image filename"), and it cannot work
///    at all once the SKILL evaluation channel is hung.
/// 2. The X11 route grabs one named window by id. It works on those hosts,
///    and it does not go through the SKILL interpreter, so it still works on
///    a hung channel.
///
/// The second route is the fallback for *every* failure of the first — a
/// failed grab, a timeout, a hung channel. With `--window-id` it captures
/// that window; without one it finds a window itself through the X11 window
/// list (matching `--pattern` when one is given). Only when both routes fail
/// does the command report an error, and then it names both failures.
///
/// This is the CLI entry point: the SKILL route runs raw SKILL, so it needs
/// the Admin capability. The RPC dispatcher uses [`screenshot_unchecked`].
pub fn screenshot(
    ctx: &crate::context::CommandContext,
    path: &str,
    window_pattern: Option<&str>,
    window_id: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    screenshot_with(
        &client,
        ctx,
        path,
        window_pattern,
        window_id,
        explicit_display,
        ScreenshotRoute::Checked,
    )
}

/// The RPC entry point for the same capture.
///
/// The RPC dispatcher has already checked the method's capability
/// (`permits_method` → Window), so the SKILL must not demand the Admin
/// capability on top of that: `window.screenshot` — and with it the MCP
/// `window_screenshot` tool — failed for every default (non-Admin) client
/// with "raw SKILL exec is not permitted: use 'vcli rpc call' instead".
///
/// The body is shared with [`screenshot`]; only the SKILL route differs.
pub fn screenshot_unchecked(
    ctx: &crate::context::CommandContext,
    path: &str,
    window_pattern: Option<&str>,
    window_id: Option<&str>,
    explicit_display: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    screenshot_with(
        &client,
        ctx,
        path,
        window_pattern,
        window_id,
        explicit_display,
        ScreenshotRoute::Unchecked,
    )
}

/// How the SKILL half of a screenshot capture is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotRoute {
    /// `execute_skill` — raw SKILL exec, Admin capability required. This is
    /// the CLI path.
    Checked,
    /// `execute_skill_unchecked` — the caller checked the capability itself.
    /// This is the RPC dispatcher path, whose gate is the Window capability.
    Unchecked,
}

/// Execute the SKILL half of a screenshot capture on the given route.
///
/// Split out so the two routes cannot drift apart: the checked route demands
/// the Admin capability, the unchecked one leaves the permission check to the
/// caller that owns the capability decision.
pub(crate) fn execute_screenshot_skill(
    client: &VirtuosoClient,
    skill: &str,
    route: ScreenshotRoute,
) -> Result<crate::models::VirtuosoResult> {
    match route {
        ScreenshotRoute::Checked => client.execute_skill(skill, None),
        ScreenshotRoute::Unchecked => client.execute_skill_unchecked(skill, None),
    }
}

/// The shared body of [`screenshot`] and [`screenshot_unchecked`].
fn screenshot_with(
    client: &VirtuosoClient,
    ctx: &crate::context::CommandContext,
    path: &str,
    window_pattern: Option<&str>,
    window_id: Option<&str>,
    explicit_display: Option<&str>,
    route: ScreenshotRoute,
) -> Result<Value> {
    let skill = match window_pattern {
        Some(pat) => client.window.screenshot_by_pattern(path, pat),
        None => client.window.screenshot(path),
    };
    let mut backend = LiveScreenshotBackend::new(client, ctx, route, explicit_display);
    screenshot_via_backend(&mut backend, &skill, path, window_pattern, window_id)
}

/// One X11 window the fallback may capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct X11Candidate {
    /// The X11 window id — what `import -window <id>` takes.
    pub window_id: String,
    /// The window title, used to pick the interesting window.
    pub title: String,
}

/// What the SKILL half of a screenshot produced.
pub(crate) enum SkillCapture {
    /// The root-window grab worked.
    Captured,
    /// The pattern matched no Virtuoso window.
    NoMatch,
    /// The grab failed; the message says why (timeout, hung channel, ...).
    Failed(String),
    /// The SKILL route is not available at all — the caller lacks the
    /// capability, or the auth check refused. The X11 route must not paper
    /// over this one.
    Unavailable(VirtuosoError),
}

/// How a screenshot is captured.
///
/// Split out from the decision logic so the fallback can be exercised
/// offline: a fake backend stands in for the SKILL route and for X11.
pub(crate) trait ScreenshotBackend {
    /// Run the SKILL (root-window) capture.
    fn skill_capture(&mut self, skill: &str) -> SkillCapture;

    /// Capture one window by X11 id.
    fn capture_window(&mut self, window_id: &str, path: &str) -> Result<()>;

    /// The windows the X11 side can see.
    fn list_windows(&mut self) -> Result<Vec<X11Candidate>>;
}

/// Capture a screenshot, SKILL route first and X11 route second.
///
/// The SKILL route grabs the X11 **root** window. It is the route that needs
/// no window id, and the one that works when the caller has none — but it
/// fails outright on X servers that refuse `XGetImage` on the root window
/// (observed on an IC26.1 host: `xwd -root` fails with BadMatch), and it
/// cannot work at all once the SKILL channel is hung.
///
/// The X11 route grabs one named window. It works on those hosts, and it
/// does not go through the SKILL interpreter, so it still works when the
/// channel is dead.
///
/// So the two are complementary, and a failure of the first is not the end of
/// the story: the second route is tried, and only when *both* fail does the
/// command report an error — with the reason from each route.
fn screenshot_via_backend(
    backend: &mut dyn ScreenshotBackend,
    skill: &str,
    path: &str,
    window_pattern: Option<&str>,
    window_id: Option<&str>,
) -> Result<Value> {
    // ── Route 1: SKILL (root window) ─────────────────────────────────────
    let (skill_detail, no_match) = match backend.skill_capture(skill) {
        SkillCapture::Captured => {
            return Ok(json!({
                "status": "saved",
                "path": path,
                "capture": "skill",
            }));
        }
        SkillCapture::NoMatch => ("no-match".to_string(), true),
        SkillCapture::Failed(msg) => (msg, false),
        SkillCapture::Unavailable(e) => return Err(e),
    };

    // ── Route 2: X11 (one named window) ──────────────────────────────────
    // The caller told us which window to grab.
    if let Some(wid) = window_id.filter(|w| !w.is_empty()) {
        return match backend.capture_window(wid, path) {
            Ok(()) => Ok(json!({
                "status": "saved",
                "path": path,
                "capture": "x11",
                "window_id": wid,
                "skill_error": skill_detail,
            })),
            Err(e) => Err(VirtuosoError::Execution(format!(
                "screenshot failed: skill route: {skill_detail}; \
                 x11 route (window {wid}): {e}"
            ))),
        };
    }

    // No window id: find one through the X11 window list — the same scan
    // `vcli window list-windows-x11` uses.
    let candidates = match backend.list_windows() {
        Ok(c) => c,
        Err(e) => {
            return Err(VirtuosoError::Execution(format!(
                "screenshot failed: skill route: {skill_detail}; \
                 x11 route: cannot list windows: {e}"
            )))
        }
    };

    match select_x11_window(&candidates, window_pattern) {
        Some(w) => match backend.capture_window(&w.window_id, path) {
            Ok(()) => Ok(json!({
                "status": "saved",
                "path": path,
                "capture": "x11",
                "window_id": w.window_id,
                "window_title": w.title,
                "skill_error": skill_detail,
            })),
            Err(e) => Err(VirtuosoError::Execution(format!(
                "screenshot failed: skill route: {skill_detail}; \
                 x11 route (window {}): {e}",
                w.window_id
            ))),
        },
        None => {
            if no_match {
                Err(VirtuosoError::NotFound(format!(
                    "no window matching pattern '{}' (skill route: {skill_detail}; \
                     x11 route: no matching window)",
                    window_pattern.unwrap_or("")
                )))
            } else {
                Err(VirtuosoError::Execution(format!(
                    "screenshot failed: skill route: {skill_detail}; \
                     x11 route: no Virtuoso window found to capture"
                )))
            }
        }
    }
}

/// Pick the window the X11 fallback should capture.
///
/// With a pattern, the same pattern the SKILL route used is matched against
/// the window title. Without one, the window an engineer is most likely
/// looking at wins: a schematic editor, then a layout editor, then an ADE
/// window, then anything Virtuoso owns.
fn select_x11_window(candidates: &[X11Candidate], pattern: Option<&str>) -> Option<X11Candidate> {
    if candidates.is_empty() {
        return None;
    }

    if let Some(pat) = pattern.filter(|p| !p.is_empty()) {
        let re = Regex::new(pat).ok();
        return candidates
            .iter()
            .find(|c| match &re {
                Some(r) => r.is_match(&c.title),
                None => c.title.contains(pat),
            })
            .cloned();
    }

    const PREFERRED: [&str; 6] = [
        "Schematic Editor",
        "Layout Editor",
        "ADE Explorer",
        "ADE Assembler",
        "Virtuoso",
        "CIW",
    ];
    for key in PREFERRED {
        if let Some(c) = candidates.iter().find(|c| c.title.contains(key)) {
            return Some(c.clone());
        }
    }
    candidates.first().cloned()
}

/// The production screenshot backend: the SKILL route and the X11 route.
struct LiveScreenshotBackend<'a> {
    client: &'a VirtuosoClient,
    ctx: &'a crate::context::CommandContext,
    route: ScreenshotRoute,
    explicit_display: Option<&'a str>,
    runner: Option<Arc<dyn RemoteTransport>>,
}

impl<'a> LiveScreenshotBackend<'a> {
    fn new(
        client: &'a VirtuosoClient,
        ctx: &'a crate::context::CommandContext,
        route: ScreenshotRoute,
        explicit_display: Option<&'a str>,
    ) -> Self {
        Self {
            client,
            ctx,
            route,
            explicit_display,
            runner: None,
        }
    }

    /// The transport used for the X11 route, opened on first use.
    fn runner(&mut self) -> Result<Arc<dyn RemoteTransport>> {
        if self.runner.is_none() {
            use crate::transport::x11;
            self.runner = Some(x11::transport_for_config(self.ctx.config())?);
        }
        Ok(self.runner.as_ref().unwrap().clone())
    }
}

impl ScreenshotBackend for LiveScreenshotBackend<'_> {
    fn skill_capture(&mut self, skill: &str) -> SkillCapture {
        // The raw-SKILL route needs the Admin capability. That is a
        // permission decision, not a capture failure: falling back to X11
        // would quietly answer a request the caller was not allowed to make.
        if let Some(reason) = self.client.check_capability(skill) {
            return SkillCapture::Unavailable(VirtuosoError::Execution(reason));
        }
        match execute_screenshot_skill(self.client, skill, self.route) {
            Ok(r) => {
                if r.skill_ok() {
                    if r.output.trim_matches('"') == "no-match" {
                        SkillCapture::NoMatch
                    } else {
                        SkillCapture::Captured
                    }
                } else {
                    let detail = if r.output.is_empty() {
                        r.errors.join("; ")
                    } else {
                        r.output.clone()
                    };
                    SkillCapture::Failed(detail)
                }
            }
            Err(VirtuosoError::Auth(msg)) => SkillCapture::Unavailable(VirtuosoError::Auth(msg)),
            Err(e) => SkillCapture::Failed(e.to_string()),
        }
    }

    fn capture_window(&mut self, window_id: &str, path: &str) -> Result<()> {
        use crate::transport::x11;
        let runner = self.runner()?;
        let user = self.ctx.config().remote_user.as_deref();
        x11::capture_window(
            runner.as_ref(),
            user,
            self.explicit_display,
            window_id,
            path,
        )
    }

    fn list_windows(&mut self) -> Result<Vec<X11Candidate>> {
        use crate::transport::x11;
        let runner = self.runner()?;
        let config = self.ctx.config();
        let user = config.remote_user.as_deref();
        let (_env, windows) = x11::list_windows(
            runner.as_ref(),
            &x11::client_id_for(config),
            user,
            self.explicit_display,
        )?;
        Ok(windows
            .into_iter()
            .map(|w| X11Candidate {
                window_id: w.window_id,
                title: w.title,
            })
            .collect())
    }
}

/// Execute a fixed-semantics X11 action on a specific window.
///
/// This is the CLI-facing wrapper around `transport::x11::action_x11`.
/// It validates parameters, calls the transport layer, and returns structured JSON.
#[allow(clippy::too_many_arguments)]
pub fn action_x11(
    ctx: &crate::context::CommandContext,
    window_id: &str,
    pid: Option<u32>,
    display: &str,
    operation: &str,
    x: Option<i32>,
    y: Option<i32>,
    text: Option<&str>,
    button: Option<u8>,
    output_dir: Option<&str>,
    timeout_secs: u64,
    explicit_display: Option<&str>,
    direct: bool,
) -> Result<Value> {
    use crate::transport::x11;

    let op = x11::X11Operation::from_str(operation)?;
    let _details = x11::validate_action_params(
        window_id, pid, display, operation, x, y, text, button, output_dir,
    )?;

    let config = ctx.config();
    let runner: Arc<dyn RemoteTransport> = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let actual_display = explicit_display.unwrap_or(display);

    let client_id = x11::client_id_for(config);
    let inputs = x11::ActionX11Inputs {
        window_id,
        pid,
        display: actual_display,
        operation: op,
        x,
        y,
        button,
        text,
        output_dir,
        timeout_secs,
        direct,
    };
    let result = x11::action_x11(runner.as_ref(), &client_id, user, &inputs)?;

    let out = serde_json::to_value(&result).map_err(|e| {
        VirtuosoError::Execution(format!("failed to serialize X11 action result: {e}"))
    })?;
    Ok(out)
}

/// Execute multiple X11 actions from a JSONL file in one process invocation.
///
/// Each line of the file is a JSON object with fields: window_id, pid (optional),
/// display (optional), operation, x, y, button, text. pid/display fall back to
/// the CLI defaults. When `direct` is true, every action skips the list-windows
/// scan for maximum throughput.
#[allow(clippy::too_many_arguments)]
pub fn action_x11_batch(
    ctx: &crate::context::CommandContext,
    file_path: &str,
    direct: bool,
    default_pid: Option<u32>,
    default_display: Option<&str>,
    explicit_display: Option<&str>,
    timeout_secs: u64,
) -> Result<Value> {
    use crate::transport::x11;
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(file_path)
        .map_err(|e| VirtuosoError::Config(format!("cannot open batch file '{file_path}': {e}")))?;
    let reader = BufReader::new(file);
    let mut actions = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| {
            VirtuosoError::Config(format!("failed to read line {}: {e}", line_no + 1))
        })?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let action: x11::BatchAction = serde_json::from_str(trimmed).map_err(|e| {
            VirtuosoError::Config(format!("invalid JSON on line {}: {e}", line_no + 1))
        })?;
        actions.push(action);
    }
    if actions.is_empty() {
        return Err(VirtuosoError::Config(
            "batch file contains no actions".into(),
        ));
    }

    let config = ctx.config();
    let runner: Arc<dyn RemoteTransport> = x11::transport_for_config(config)?;
    let user = config.remote_user.as_deref();
    let client_id = x11::client_id_for(config);
    let effective_default_display = explicit_display.or(default_display);

    let batch_inputs = x11::BatchX11Inputs {
        actions: &actions,
        default_pid,
        default_display: effective_default_display,
        direct,
        timeout_secs,
    };
    let result = x11::action_x11_batch(runner.as_ref(), &client_id, user, &batch_inputs)?;

    let out = serde_json::to_value(&result)
        .map_err(|e| VirtuosoError::Execution(format!("failed to serialize batch result: {e}")))?;
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::x11::{DialogInfo, DismissResult};

    fn dialog(id: &str) -> DialogInfo {
        DialogInfo {
            window_id: id.into(),
            requested_window_id: None,
            resolved_window_id: None,
            display: None,
            title: "Save Changes".into(),
            x: 0,
            y: 0,
            w: 239,
            h: 142,
            child: None,
            action: None,
            still_mapped: None,
        }
    }

    fn result(
        found: Vec<DialogInfo>,
        dismissed: Vec<DialogInfo>,
        errors: Vec<String>,
    ) -> DismissResult {
        DismissResult {
            display: ":0".into(),
            found,
            dismissed,
            errors,
            raw_log: String::new(),
        }
    }

    #[test]
    fn unverified_dismissal_reports_failed_status() {
        let r = result(
            vec![dialog("0x1")],
            vec![],
            vec!["dismissal of 0x2 unverified: still mapped".into()],
        );
        assert_eq!(x11_dismiss_status(&r), "failed");
        assert_eq!(x11_dismiss_window_status(&r), "failed");
    }

    #[test]
    fn verified_dismissal_reports_dismissed_status() {
        let r = result(vec![], vec![dialog("0x2")], vec![]);
        assert_eq!(x11_dismiss_status(&r), "dismissed");
        assert_eq!(x11_dismiss_window_status(&r), "dismissed");
    }

    #[test]
    fn found_but_not_dismissed_is_not_a_failure() {
        let r = result(vec![dialog("0x1")], vec![], vec![]);
        assert_eq!(x11_dismiss_status(&r), "found");
        // The single-window path has no "found" state: the target was explicit.
        assert_eq!(x11_dismiss_window_status(&r), "not-found");
    }

    #[test]
    fn no_dialog_is_not_a_failure() {
        let r = result(vec![], vec![], vec![]);
        assert_eq!(x11_dismiss_status(&r), "no-dialog");
        assert_eq!(x11_dismiss_window_status(&r), "not-found");
    }

    #[test]
    fn failed_payload_carries_a_non_zero_exit_code() {
        let r = result(
            vec![dialog("0x1")],
            vec![],
            vec!["dismissal of 0x2 unverified: still mapped".into()],
        );
        let payload =
            x11_dismiss_payload(&r, x11_dismiss_status(&r), "escape", &[]).expect("payload");
        assert_eq!(payload["status"], json!("failed"));
        assert_eq!(payload["exit_code"], json!(1));
        // The mapping from the payload to the process exit code is the one
        // main() applies — pin it here so the two cannot drift apart.
        assert_ne!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::SUCCESS
        );
        assert_eq!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::GENERAL_ERROR
        );
    }

    #[test]
    fn successful_payload_has_no_exit_code_field() {
        let r = result(vec![], vec![dialog("0x2")], vec![]);
        let payload =
            x11_dismiss_payload(&r, x11_dismiss_status(&r), "escape", &[]).expect("payload");
        assert_eq!(payload["status"], json!("dismissed"));
        assert!(payload.get("exit_code").is_none());
        assert_eq!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::SUCCESS
        );
    }

    #[test]
    fn dry_run_payload_keeps_the_dry_run_exit_code() {
        let r = result(vec![], vec![], vec![]);
        let payload = x11_dismiss_payload(
            &r,
            x11_dismiss_status(&r),
            "escape",
            &[("dry_run", json!(true))],
        )
        .expect("payload");
        assert_eq!(payload["status"], json!("no-dialog"));
        assert_eq!(payload["dry_run"], json!(true));
        assert!(payload.get("exit_code").is_none());
        assert_eq!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::DRY_RUN_OK
        );
    }

    // --- dismiss-window (the single-window X11 path) ------------------------

    #[test]
    fn failed_window_payload_carries_a_non_zero_exit_code() {
        // Mirrors the dialog path: `vcli window dismiss-window --x11` must
        // not exit 0 when the dismissal could not be verified.
        let r = result(
            vec![],
            vec![],
            vec!["dismissal of 0x2 unverified: still mapped".into()],
        );
        let payload = x11_dismiss_payload(
            &r,
            x11_dismiss_window_status(&r),
            "enter",
            &[("window_id", json!("0x1"))],
        )
        .expect("payload");
        assert_eq!(payload["status"], json!("failed"));
        assert_eq!(payload["window_id"], json!("0x1"));
        assert_eq!(payload["exit_code"], json!(1));
        assert_ne!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::SUCCESS
        );
        assert_eq!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::GENERAL_ERROR
        );
    }

    #[test]
    fn successful_window_payload_has_no_exit_code_field() {
        let r = result(vec![], vec![dialog("0x2")], vec![]);
        let payload = x11_dismiss_payload(
            &r,
            x11_dismiss_window_status(&r),
            "enter",
            &[("window_id", json!("0x1"))],
        )
        .expect("payload");
        assert_eq!(payload["status"], json!("dismissed"));
        assert!(payload.get("exit_code").is_none());
        assert_eq!(
            crate::exit_codes::exit_code_for_value(&payload),
            crate::exit_codes::SUCCESS
        );
    }
}

#[cfg(test)]
mod screenshot_fallback_tests {
    use super::*;

    /// A backend that stands in for the SKILL route and for X11.
    struct FakeBackend {
        skill: SkillCapture,
        windows: Vec<X11Candidate>,
        captured: Vec<String>,
        capture_error: Option<String>,
        list_error: Option<String>,
    }

    impl FakeBackend {
        /// A backend whose SKILL route fails with `skill_error`.
        fn skill_failed(skill_error: &str) -> Self {
            Self {
                skill: SkillCapture::Failed(skill_error.to_string()),
                windows: vec![X11Candidate {
                    window_id: "0x1400010".into(),
                    title: "Schematic Editor — amp".into(),
                }],
                captured: Vec::new(),
                capture_error: None,
                list_error: None,
            }
        }

        fn capturing(skill: SkillCapture) -> Self {
            Self {
                skill,
                windows: Vec::new(),
                captured: Vec::new(),
                capture_error: None,
                list_error: None,
            }
        }
    }

    impl ScreenshotBackend for FakeBackend {
        fn skill_capture(&mut self, _skill: &str) -> SkillCapture {
            match &self.skill {
                SkillCapture::Captured => SkillCapture::Captured,
                SkillCapture::NoMatch => SkillCapture::NoMatch,
                SkillCapture::Failed(m) => SkillCapture::Failed(m.clone()),
                SkillCapture::Unavailable(e) => SkillCapture::Unavailable(match e {
                    VirtuosoError::Execution(m) => VirtuosoError::Execution(m.clone()),
                    other => {
                        let _ = other;
                        VirtuosoError::Execution("unavailable".into())
                    }
                }),
            }
        }

        fn capture_window(&mut self, window_id: &str, _path: &str) -> Result<()> {
            if let Some(e) = &self.capture_error {
                return Err(VirtuosoError::Execution(e.clone()));
            }
            self.captured.push(window_id.to_string());
            Ok(())
        }

        fn list_windows(&mut self) -> Result<Vec<X11Candidate>> {
            if let Some(e) = &self.list_error {
                return Err(VirtuosoError::Execution(e.clone()));
            }
            Ok(self.windows.clone())
        }
    }

    /// A SKILL timeout must not end the story: the X11 route takes over.
    #[test]
    fn screenshot_falls_back_to_x11_on_skill_timeout_without_window_id() {
        let mut backend = FakeBackend::skill_failed("timeout after 30s");
        let out = screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", None, None)
            .expect("the X11 route must recover the screenshot");

        assert_eq!(out["capture"], json!("x11"));
        assert_eq!(out["window_id"], json!("0x1400010"));
        assert_eq!(out["skill_error"], json!("timeout after 30s"));
        assert_eq!(backend.captured, vec!["0x1400010".to_string()]);
    }

    /// A hung channel is the case this fallback exists for: the SKILL route
    /// can never come back, but X11 does not go through the SKILL interpreter.
    #[test]
    fn screenshot_falls_back_to_x11_when_the_channel_is_hung() {
        let hung = crate::channel_state::hung_message(&crate::channel_state::StuckCall::new(
            "maeOpenSetup(\"L\" \"C\" \"maestro\")",
            "2026-11-10T22:29:33Z",
            1_762_792_173,
            30,
        ));
        let mut backend = FakeBackend::skill_failed(&hung);
        let out = screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", None, None)
            .expect("the X11 route must recover the screenshot");

        assert_eq!(out["capture"], json!("x11"));
        assert!(out["skill_error"]
            .as_str()
            .unwrap()
            .contains(crate::channel_state::HUNG_TEXT_ZH));
        assert_eq!(backend.captured, vec!["0x1400010".to_string()]);
    }

    /// When both routes fail, the error must name both failures: the
    /// operator needs to know that the SKILL route failed *and* why the X11
    /// route could not pick it up.
    #[test]
    fn screenshot_reports_both_routes_when_both_fail() {
        let mut backend = FakeBackend::skill_failed("timeout after 30s");
        backend.capture_error = Some("import -window failed: missing an image filename".into());

        let err = screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", None, None)
            .expect_err("both routes failed");
        let text = err.to_string();
        assert!(
            text.contains("timeout after 30s"),
            "skill reason missing: {text}"
        );
        assert!(
            text.contains("import -window failed"),
            "x11 reason missing: {text}"
        );
    }

    /// A caller that named a window gets that window — the fallback must not
    /// second-guess it.
    #[test]
    fn screenshot_uses_the_window_id_the_caller_gave() {
        let mut backend = FakeBackend::skill_failed("timeout after 30s");
        let out = screenshot_via_backend(
            &mut backend,
            "skill",
            "/tmp/a.png",
            None,
            Some("0xdeadbeef"),
        )
        .expect("the X11 route must recover the screenshot");

        assert_eq!(out["capture"], json!("x11"));
        assert_eq!(out["window_id"], json!("0xdeadbeef"));
        assert_eq!(backend.captured, vec!["0xdeadbeef".to_string()]);
    }

    /// The SKILL route still wins when it works — the fallback must not
    /// change the normal path.
    #[test]
    fn a_working_skill_route_is_used_as_before() {
        let mut backend = FakeBackend::capturing(SkillCapture::Captured);
        let out = screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", None, None)
            .expect("the skill route must succeed");

        assert_eq!(out["capture"], json!("skill"));
        assert!(backend.captured.is_empty(), "X11 must not be touched");
    }

    /// A missing capability is a permission decision, not a capture failure:
    /// the X11 route must not quietly answer a request the caller was not
    /// allowed to make.
    #[test]
    fn a_missing_capability_is_not_papered_over_by_the_x11_route() {
        let mut backend = FakeBackend::capturing(SkillCapture::Unavailable(
            VirtuosoError::Execution("raw SKILL exec is not permitted".into()),
        ));
        let err = screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", None, None)
            .expect_err("the permission error must surface");
        assert!(err.to_string().contains("raw SKILL exec is not permitted"));
        assert!(backend.captured.is_empty(), "X11 must not be touched");
    }

    /// Without a pattern the fallback picks the window an engineer is most
    /// likely looking at.
    #[test]
    fn the_fallback_prefers_the_schematic_editor() {
        let candidates = vec![
            X11Candidate {
                window_id: "0x1".into(),
                title: "CIW".into(),
            },
            X11Candidate {
                window_id: "0x2".into(),
                title: "Virtuoso Schematic Editor L1".into(),
            },
            X11Candidate {
                window_id: "0x3".into(),
                title: "ADE Explorer".into(),
            },
        ];
        let picked = select_x11_window(&candidates, None).expect("a window must be picked");
        assert_eq!(picked.window_id, "0x2");
    }

    /// With a pattern the same pattern is matched against the title.
    #[test]
    fn the_fallback_honours_the_window_pattern() {
        let candidates = vec![
            X11Candidate {
                window_id: "0x1".into(),
                title: "CIW".into(),
            },
            X11Candidate {
                window_id: "0x2".into(),
                title: "ADE Explorer Loading".into(),
            },
        ];
        let picked = select_x11_window(&candidates, Some("ADE")).expect("a window must be picked");
        assert_eq!(picked.window_id, "0x2");
        assert!(select_x11_window(&candidates, Some("nothing-matches")).is_none());
    }

    /// "No window matched" keeps its old meaning when the X11 route finds
    /// nothing either.
    #[test]
    fn a_pattern_that_matches_nothing_stays_a_not_found() {
        let mut backend = FakeBackend::capturing(SkillCapture::NoMatch);
        backend.windows = vec![X11Candidate {
            window_id: "0x1".into(),
            title: "CIW".into(),
        }];
        let err =
            screenshot_via_backend(&mut backend, "skill", "/tmp/a.png", Some("nomatch"), None)
                .expect_err("nothing matched");
        assert!(matches!(err, VirtuosoError::NotFound(_)), "got: {err}");
    }
}

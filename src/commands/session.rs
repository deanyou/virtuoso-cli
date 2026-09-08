use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::context::CommandContext;
use crate::error::{Result, VirtuosoError};
use crate::models::{SessionInfo, TunnelState};
use crate::output::OutputFormat;
use crate::transport::identity::{IdentityError, ProcessIdentity};
use crate::transport::tunnel::{classify_ssh_pid, PidVerdict, SSHClient};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Shared probe resolution (used by both `list` and `show`)
//
// A single function decides whether a session may be probed online, and if
// so, which local port to connect to. Two paths:
//   - Local direct: config is local AND the session belongs to this machine.
//   - Remote tunnel: a 6-step verification chain confirms the TunnelState
//     points at this session, matches the current context, and the forward
//     process is still alive and verifiable.
//
// Any failure returns `ProbeSkip` — never an error — so the caller can
// always fall back to showing cached metadata (phase 1 of `show`).
// ---------------------------------------------------------------------------

/// An endpoint that may be probed (TCP connect + SKILL queries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeEndpoint {
    /// Local direct connection: the session runs on this machine, use its
    /// own `port`.
    Local { port: u16 },
    /// Remote tunnel: the session runs on a remote host, use the verified
    /// local forward port from `TunnelState`.
    RemoteTunnel { port: u16 },
}

/// Why probing was skipped. The `reason` string is surfaced in JSON output
/// as `probe_skip_reason` so users can diagnose why no live data was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeSkip {
    pub reason: &'static str,
}

/// Shared entry point: resolve a probe endpoint for a session under the
/// given context. Returns `Ok(endpoint)` if probing is allowed,
/// `Err(skip)` otherwise (caller shows cached data).
///
/// Loads TunnelState internally; for batch callers (e.g. `list`) use
/// `resolve_probe_endpoint_with_state` to avoid reloading the same state
/// file for every session.
pub(crate) fn resolve_probe_endpoint(
    ctx: &CommandContext,
    session: &SessionInfo,
) -> std::result::Result<ProbeEndpoint, ProbeSkip> {
    let state = if ctx.config().is_remote() {
        match TunnelState::load_with_profile(ctx.config().profile.as_deref()) {
            Ok(Some(s)) => Some(s),
            Ok(None) => None,
            Err(_) => {
                return Err(ProbeSkip {
                    reason: "tunnel_state_error",
                })
            }
        }
    } else {
        None
    };
    resolve_probe_endpoint_with_state(ctx, session, state.as_ref())
}

/// Like `resolve_probe_endpoint`, but accepts a pre-loaded `TunnelState`
/// for efficiency. Used by `list()` which loads state once and reuses it
/// across all sessions. `state` may be `None` (no tunnel exists).
pub(crate) fn resolve_probe_endpoint_with_state(
    ctx: &CommandContext,
    session: &SessionInfo,
    state: Option<&TunnelState>,
) -> std::result::Result<ProbeEndpoint, ProbeSkip> {
    if ctx.config().is_remote() {
        resolve_remote_tunnel(ctx, session, state)
    } else {
        resolve_local_direct(ctx, session)
    }
}

/// Local direct path: config has no remote_host, AND the session belongs to
/// this machine. We cannot rely solely on `!cfg.is_remote()` — a public
/// session cache may contain remote records, and legacy mode skips ownership
/// validation, so we must independently confirm the session is local.
fn resolve_local_direct(
    ctx: &CommandContext,
    session: &SessionInfo,
) -> std::result::Result<ProbeEndpoint, ProbeSkip> {
    let cfg = ctx.config();

    // Step L1: confirm the session belongs to this machine.
    if !is_local_session(cfg, session) {
        return Err(ProbeSkip {
            reason: "local_ownership_unverified",
        });
    }

    // Step L2: ownership validation when a target is selected.
    if ctx.target_id().is_some() && ctx.validate_session_ownership(session).is_err() {
        return Err(ProbeSkip {
            reason: "ownership_mismatch",
        });
    }

    // Step L3: local port must be reachable.
    if !tcp_reachable(session.port) {
        return Err(ProbeSkip {
            reason: "local_port_unreachable",
        });
    }

    Ok(ProbeEndpoint::Local { port: session.port })
}

/// Remote tunnel path: 6-step verification chain. Every step must pass; any
/// failure skips probing (cached view only).
///
/// 1. Session ownership matches the selected target.
/// 2. TunnelState exists, is "attached", and its attached_session_id matches.
/// 3. TunnelState's remote host and remote port directly match the session.
/// 4. TunnelState matches the current context (validate_tunnel_ownership).
/// 5. The recorded forward process is alive, verifiable as ssh, and its
///    start identity matches (no PID reuse).
/// 6. The local forward port is reachable.
fn resolve_remote_tunnel(
    ctx: &CommandContext,
    session: &SessionInfo,
    state: Option<&TunnelState>,
) -> std::result::Result<ProbeEndpoint, ProbeSkip> {
    // Step 1: session ownership.
    if ctx.target_id().is_some() && ctx.validate_session_ownership(session).is_err() {
        return Err(ProbeSkip {
            reason: "ownership_mismatch",
        });
    }

    // Step 2: TunnelState basic checks (state is pre-loaded by caller).
    let state = match state {
        Some(s) => s,
        None => {
            return Err(ProbeSkip {
                reason: "no_tunnel",
            })
        }
    };

    if state.mode.as_deref() != Some("attached") {
        return Err(ProbeSkip {
            reason: "tunnel_not_attached",
        });
    }

    if state.attached_session_id.as_deref() != Some(&session.id) {
        return Err(ProbeSkip {
            reason: "session_id_mismatch",
        });
    }

    // Step 3: state ↔ session direct correspondence (same remote host + port).
    if state.remote_host != session.host {
        return Err(ProbeSkip {
            reason: "tunnel_host_mismatch",
        });
    }

    let state_remote_port = state.remote_bridge_port.or(state.attached_remote_port);
    if state_remote_port != Some(session.port) {
        return Err(ProbeSkip {
            reason: "tunnel_remote_port_mismatch",
        });
    }

    // Step 4: state matches current context.
    if ctx.validate_tunnel_ownership(state).is_err() {
        return Err(ProbeSkip {
            reason: "tunnel_context_mismatch",
        });
    }

    // Step 5: forward process identity.
    if state.pid == 0 {
        return Err(ProbeSkip {
            reason: "tunnel_no_pid",
        });
    }

    let expected_identity = match state.start_identity {
        Some(id) if id > 0 => id,
        _ => {
            return Err(ProbeSkip {
                reason: "tunnel_identity_unverified",
            })
        }
    };

    let actual_identity = match ProcessIdentity::of_pid(state.pid) {
        Ok(id) => id,
        Err(IdentityError::NoSuchProcess(_)) => {
            return Err(ProbeSkip {
                reason: "tunnel_process_dead",
            })
        }
        Err(_) => {
            return Err(ProbeSkip {
                reason: "tunnel_identity_unreadable",
            })
        }
    };

    // Verify the process is actually an ssh executable (reuse existing logic).
    match classify_ssh_pid(state.pid) {
        PidVerdict::VerifiedSsh => {}
        PidVerdict::Gone => {
            return Err(ProbeSkip {
                reason: "tunnel_process_dead",
            })
        }
        PidVerdict::NotVerifiable { .. } => {
            return Err(ProbeSkip {
                reason: "tunnel_process_not_ssh",
            })
        }
    }

    if actual_identity.start_identity != expected_identity {
        return Err(ProbeSkip {
            reason: "tunnel_process_reused",
        });
    }

    // Step 6: local forward port reachable.
    if !tcp_reachable(state.port) {
        return Err(ProbeSkip {
            reason: "forward_port_unreachable",
        });
    }

    Ok(ProbeEndpoint::RemoteTunnel { port: state.port })
}

/// Get the system hostname via libc::gethostname(2). Returns None on
/// failure or if the hostname is empty. This is preferred over reading
/// $HOSTNAME, which is not guaranteed to be set in every process
/// environment (e.g. non-login shells, CI containers).
fn system_hostname() -> Option<String> {
    unsafe {
        let mut buf = [0u8; 256];
        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) != 0 {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if len == 0 {
            return None;
        }
        String::from_utf8(buf[..len].to_vec()).ok()
    }
}

/// Compare two hostnames, allowing short-name vs FQDN equivalence.
///
/// - Exact match (case-sensitive, as hostnames are on Unix).
/// - Short-name match: if either contains a dot, compare the part before
///   the first dot. This lets "compute-eda-42" match
///   "compute-eda-42.internal.corp".
fn hostnames_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_short = a.split('.').next().unwrap_or(a);
    let b_short = b.split('.').next().unwrap_or(b);
    !a_short.is_empty() && a_short == b_short
}

/// Check whether a session belongs to this machine, given the config.
///
/// - If `cfg.remote_host` is set, the session is local only if its host
///   matches exactly.
/// - If `cfg.remote_host` is unset, we check known local hostnames
///   (`localhost`, `127.0.0.1`, `::1`), the system hostname (via
///   libc::gethostname, with short/FQDN equivalence), and finally the
///   `HOSTNAME` env var as a last resort.
/// - Anything that cannot be confirmed is treated as NOT local (skip probe).
fn is_local_session(cfg: &Config, session: &SessionInfo) -> bool {
    const LOCAL_HOSTNAMES: &[&str] = &["localhost", "127.0.0.1", "::1"];

    if let Some(host) = &cfg.remote_host {
        return session.host == *host;
    }

    if LOCAL_HOSTNAMES.contains(&session.host.as_str()) {
        return true;
    }

    // System hostname (preferred — does not depend on process env).
    if let Some(sys_host) = system_hostname() {
        if !sys_host.is_empty() && hostnames_match(&sys_host, &session.host) {
            return true;
        }
    }

    // Fall back to HOSTNAME env var (may not be set in all environments).
    if let Ok(env_host) = std::env::var("HOSTNAME") {
        if !env_host.is_empty() && hostnames_match(&env_host, &session.host) {
            return true;
        }
    }

    false
}

/// TCP reachability check with an explicit 200ms timeout. Used by both
/// local and remote probe paths.
fn tcp_reachable(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// List active Virtuoso sessions.
///
/// Behaviour differs by mode:
///
/// **Legacy (no target selected):** filter to locally-alive sessions (TCP
/// reachable on `127.0.0.1:port`). Files are **never deleted** here —
/// deletion is `session cleanup`'s job.
///
/// **Target mode:** show **all** cached sessions (including unattached /
/// remote-only) with three independent status fields:
/// - `endpoint_match`: `"match"` / `"mismatch"` / `"unchecked"` — does the
///   session's host+port match the selected target?
/// - `tunnel_verified`: `true` / `false` — is there an attached tunnel whose
///   `attached_session_id` points at this session?
/// - `probe_status`: `"reachable"` / `"unreachable"` / `"not_probed"` — only
///   probed when `tunnel_verified` is true; otherwise `"not_probed"` (we do
///   NOT equate "local port unreachable" with "remote daemon dead").
///
/// Remote sync uses the resolved `ctx.config()` — never re-reads env.
/// Sync failures are surfaced as `sync_status: "failed"` but the cached
/// list is still returned.
pub fn list(ctx: &CommandContext, format: OutputFormat) -> Result<Value> {
    let cfg = ctx.config();

    // Remote sync: use the SAME immutable config as the rest of the invocation.
    let sync_status: &str = if cfg.is_remote() {
        match SSHClient::from_config(cfg, cfg.keep_remote_files) {
            Ok(client) => {
                if SessionInfo::sync_from_remote(client.transport().as_ref()).is_ok() {
                    "ok"
                } else {
                    "failed"
                }
            }
            Err(_) => "failed",
        }
    } else {
        "ok"
    };

    let sessions = SessionInfo::list()
        .map_err(|e| VirtuosoError::Execution(format!("failed to read sessions: {e}")))?;

    let has_target = ctx.target_id().is_some();

    // Legacy: filter to locally-alive, but NEVER delete files.
    // Target: show all cached sessions (no filtering).
    let displayed: Vec<&SessionInfo> = if has_target {
        sessions.iter().collect()
    } else {
        sessions.iter().filter(|s| s.is_alive()).collect()
    };

    // Load TunnelState once for target mode (performance: don't reload per
    // session).
    let tunnel_state = if has_target {
        TunnelState::load_with_profile(cfg.profile.as_deref())
            .ok()
            .flatten()
    } else {
        None
    };

    // Build per-session JSON entries.
    let session_entries: Vec<Value> = displayed
        .iter()
        .map(|s| {
            if has_target {
                let endpoint_match = compute_endpoint_match(ctx, s);
                // Use the SHARED verification chain (same 6-step path as
                // show). tunnel_verified=true only when the full chain
                // passes, OR when the only failure is
                // forward_port_unreachable (identity verified, port just
                // down). Any earlier failure (wrong host, dead process,
                // mismatched session id, …) → tunnel_verified=false.
                let probe_result = resolve_probe_endpoint_with_state(ctx, s, tunnel_state.as_ref());
                let (tunnel_verified, probe_status) = match probe_result {
                    Ok(_) => (true, "reachable"),
                    Err(ProbeSkip {
                        reason: "forward_port_unreachable",
                    }) => (true, "unreachable"),
                    Err(_) => (false, "not_probed"),
                };
                json!({
                    "id": s.id,
                    "port": s.port,
                    "pid": s.pid,
                    "host": s.host,
                    "user": s.user,
                    "created": s.created,
                    "endpoint_match": endpoint_match,
                    "tunnel_verified": tunnel_verified,
                    "probe_status": probe_status,
                })
            } else {
                json!({
                    "id": s.id,
                    "port": s.port,
                    "pid": s.pid,
                    "host": s.host,
                    "user": s.user,
                    "created": s.created,
                })
            }
        })
        .collect();

    // JSON output.
    if format == OutputFormat::Json {
        let mut result = json!({
            "status": "success",
            "count": displayed.len(),
            "sync_status": sync_status,
            "sessions": session_entries,
        });
        if sync_status == "failed" {
            result["note"] = json!("remote sync failed; showing cached sessions only");
        }
        return Ok(result);
    }

    // Table output.
    if displayed.is_empty() {
        println!("No active Virtuoso sessions found.");
        println!("Start Virtuoso and run RBStart() in CIW to register a session.");
        return Ok(json!({
            "status": "success",
            "count": 0,
            "sync_status": sync_status
        }));
    }

    if has_target {
        println!(
            "{:<20} {:>6}  {:<14}  {:<10}  {:<12}  CREATED",
            "SESSION ID", "PORT", "HOST", "ENDPOINT", "PROBE"
        );
        println!("{}", "-".repeat(90));
        for entry in &session_entries {
            println!(
                "{:<20} {:>6}  {:<14}  {:<10}  {:<12}  {}",
                entry["id"].as_str().unwrap_or(""),
                entry["port"].as_u64().unwrap_or(0),
                entry["host"].as_str().unwrap_or(""),
                entry["endpoint_match"].as_str().unwrap_or(""),
                entry["probe_status"].as_str().unwrap_or(""),
                entry["created"].as_str().unwrap_or(""),
            );
        }
    } else {
        println!(
            "{:<20} {:>6}  {:>7}  {:<12}  CREATED",
            "SESSION ID", "PORT", "PID", "HOST"
        );
        println!("{}", "-".repeat(72));
        for s in &displayed {
            println!(
                "{:<20} {:>6}  {:>7}  {:<12}  {}",
                s.id, s.port, s.pid, s.host, s.created
            );
        }
    }

    Ok(json!({
        "status": "success",
        "count": displayed.len(),
        "sync_status": sync_status
    }))
}

pub fn current() -> Result<Value> {
    let live = SessionInfo::list_alive();
    match live.len() {
        0 => Ok(
            json!({"status": "success", "session": null, "note": "no live sessions; VB_PORT will be used"}),
        ),
        1 => Ok(json!({
            "status": "success",
            "session": live[0].id,
            "port": live[0].port,
            "auto_selected": true,
        })),
        _ => {
            let ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
            Ok(json!({
                "status": "ambiguous",
                "sessions": ids,
                "note": "use --session <id> to select one",
            }))
        }
    }
}

pub fn cleanup() -> Result<Value> {
    let all = SessionInfo::list().unwrap_or_default();
    let dir = SessionInfo::sessions_dir();
    let mut removed = Vec::new();
    for s in &all {
        if !s.is_alive() {
            let path = dir.join(format!("{}.json", s.id));
            if std::fs::remove_file(&path).is_ok() {
                removed.push(s.id.clone());
            }
        }
    }
    Ok(json!({
        "status": "success",
        "removed": removed.len(),
        "sessions": removed,
    }))
}

pub fn history(id: &str, only_skill: bool, only_cmd: bool, limit: usize) -> Result<Value> {
    let show_skill = !only_cmd;
    let show_cmd = !only_skill;

    let skill_entries: Vec<Value> = if show_skill {
        crate::history::load_skill(id, limit)
            .into_iter()
            .map(|e| serde_json::json!({"type":"skill","ts":e.ts,"ok":e.ok,"skill":e.skill,"output":e.output}))
            .collect()
    } else {
        vec![]
    };

    let cmd_entries: Vec<Value> = if show_cmd {
        crate::history::load_cmd(Some(id), limit)
            .into_iter()
            .map(
                |e| serde_json::json!({"type":"cmd","ts":e.ts,"cmd":e.cmd,"exit_code":e.exit_code}),
            )
            .collect()
    } else {
        vec![]
    };

    Ok(json!({
        "status": "success",
        "session": id,
        "skill_count": skill_entries.len(),
        "cmd_count": cmd_entries.len(),
        "skill": skill_entries,
        "cmd": cmd_entries,
    }))
}

/// Show details for a specific session.
///
/// Two-phase behaviour:
///
/// **Phase 1 (always):** load the session file and present cached metadata.
/// Errors here are real (file missing / corrupt → `NotFound`).
///
/// **Phase 2 (only when `resolve_probe_endpoint` succeeds):** connect to the
/// verified local port, query the daemon for `$USER` / version / liveness,
/// and write fresh values back to the session file. Any verification failure
/// (ownership mismatch, no tunnel, dead process, corrupt state, …) sets
/// `probe_skipped: true` with a `probe_skip_reason` — the cached view is
/// still returned, never an error.
pub fn show(ctx: &CommandContext, id: &str, _format: OutputFormat) -> Result<Value> {
    // Phase 1: load session. File missing / corrupt is a real error.
    let s = SessionInfo::load(id)
        .map_err(|e| VirtuosoError::NotFound(format!("session '{id}' not found: {e}")))?;

    let endpoint_match = compute_endpoint_match(ctx, &s);

    // Phase 2: resolve a probe endpoint, then probe if allowed.
    // daemon_responsive is Option<bool>: Some(_) = probe ran, None = skipped.
    // daemon_user / daemon_version hold the LIVE probe values when probe
    // succeeded; the output layer falls back to s.daemon_user / s.daemon_version
    // (cached) when probe was skipped.
    let (
        daemon_user,
        daemon_user_warning,
        daemon_responsive,
        daemon_version,
        daemon_version_warning,
        probe_skipped,
        probe_skip_reason,
        probe_source,
        probe_port,
    ) = match resolve_probe_endpoint(ctx, &s) {
        Ok(endpoint) => {
            let port = match &endpoint {
                ProbeEndpoint::Local { port } => *port,
                ProbeEndpoint::RemoteTunnel { port } => *port,
            };
            let source = match &endpoint {
                ProbeEndpoint::Local { .. } => "local",
                ProbeEndpoint::RemoteTunnel { .. } => "remote_tunnel",
            };

            let client = VirtuosoClient::new("127.0.0.1", port, 3);
            let user_result = client.get_daemon_user();
            let version_result = client.get_daemon_version();
            let alive = client.daemon_alive();
            let ver = version_result.as_ref().unwrap_or(&None).clone();
            let ver_warn = match &version_result {
                Ok(Some(v)) => check_version_skew(v),
                Ok(None) => None,
                Err(e) => Some(format!("daemon version query failed: {e}")),
            };
            let (user, user_warn) = match user_result {
                Ok(user_opt) => (user_opt, None),
                Err(e) => (None, Some(format!("daemon user query failed: {e}"))),
            };

            (
                user,
                user_warn,
                Some(alive),
                ver,
                ver_warn,
                false,
                None,
                Some(source),
                Some(port),
            )
        }
        Err(skip) => (
            // Probe skipped: use cached values from the session file.
            s.daemon_user.clone(),
            None,
            None, // responsive unknown — not "false"
            s.daemon_version.clone(),
            None,
            true,
            Some(skip.reason),
            None,
            None,
        ),
    };

    // Cross-user check uses the resolved config (never re-reads env).
    let cross_user_warning = check_cross_user(ctx.config(), &s, daemon_user.as_deref());

    // Stale-daemon hint only when the probe actually ran and the daemon
    // didn't respond (daemon_responsive == Some(false)).
    let stale_daemon_hint = if !probe_skipped && daemon_responsive == Some(false) {
        Some(
            "CIW daemon port is bound but the daemon is not responding to SKILL.\n\
             In the Virtuoso CIW, run:\n\
               RBStop()\n\
               (load \"/absolute/path/to/ramic_bridge.il\")\n\
             If that does not clear it, use RBStopAll() before loading again."
                .to_string(),
        )
    } else {
        None
    };

    // Write back only when the probe succeeded and produced fresh data.
    // Skipped probes never mutate the cache.
    if !probe_skipped && (daemon_user.is_some() || daemon_version.is_some()) {
        let mut s_mut = s.clone();
        if let Some(u) = daemon_user.as_ref() {
            s_mut.daemon_user = Some(u.clone());
        }
        if let Some(v) = daemon_version.as_ref() {
            s_mut.daemon_version = Some(v.clone());
        }
        s_mut.save_to_session_file();
    }

    let has_warnings = cross_user_warning.is_some()
        || daemon_version_warning.is_some()
        || daemon_user_warning.is_some()
        || stale_daemon_hint.is_some();

    // data_source: "live" when probe ran, "cached" when skipped (values
    // come from the session file).
    let data_source = if probe_skipped { "cached" } else { "live" };

    Ok(json!({
        "status": if has_warnings { "warning" } else { "success" },
        "session": {
            "id": s.id,
            "port": s.port,
            "pid": s.pid,
            "host": s.host,
            "user": s.user,
            "created": s.created,
            "endpoint_match": endpoint_match,
            "daemon_responsive": daemon_responsive,
            "daemon_user": daemon_user,
            "daemon_version": daemon_version,
            "data_source": data_source,
            "cli_version": env!("CARGO_PKG_VERSION"),
        },
        "probe": {
            "skipped": probe_skipped,
            "skip_reason": probe_skip_reason,
            "source": probe_source,
            "port": probe_port,
        },
        "warnings": {
            "daemon_user": daemon_user_warning,
            "cross_user": cross_user_warning,
            "version_skew": daemon_version_warning,
            "stale_daemon": stale_daemon_hint,
        }
    }))
}

/// Compute the endpoint match status for a session under the current context.
///
/// - `"unchecked"` — no target selected (legacy mode); ownership is not verified.
/// - `"match"` — target selected and session host (+ port when explicit) matches.
/// - `"mismatch"` — target selected but session does not match.
fn compute_endpoint_match(ctx: &CommandContext, session: &SessionInfo) -> &'static str {
    if ctx.target_id().is_none() {
        return "unchecked";
    }
    match ctx.validate_session_ownership(session) {
        Ok(_) => "match",
        Err(_) => "mismatch",
    }
}

/// Compare the daemon's reported version (from `RBDVersion` global) with the
/// version compiled into this vcli binary. A mismatch usually means the
/// user installed a new vcli but forgot to reload `ramic_bridge.il` (or vice
/// versa), leaving the daemon binary out of sync with the SKILL wrapper.
fn check_version_skew(daemon_version: &str) -> Option<String> {
    const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
    if daemon_version == CLI_VERSION {
        return None;
    }
    Some(format!(
        "daemon reports version {daemon_version:?} but vcli binary is {CLI_VERSION:?}. \
         They are out of sync — reinstall vcli (`cargo install --path .` or pull a fresh release) \
         and reload ramic_bridge.il in the CIW to align versions."
    ))
}

/// Compare the daemon's Unix user with the configured `remote_user`.
///
/// Returns `Some(warning)` if a mismatch is detected, `None` otherwise.
/// Set `allow_cross_user_daemon: true` in config (env: `VB_ALLOW_CROSS_USER_DAEMON=1`,
/// or target config field) to suppress the warning.
///
/// Uses the resolved `Config` — never re-reads environment variables.
/// `Config::from_env()` already handles the `VB_REMOTE_USER_<profile>` →
/// `VB_REMOTE_USER` fallback, so this function simply uses `cfg.remote_user`.
fn check_cross_user(
    cfg: &Config,
    session: &SessionInfo,
    daemon_user: Option<&str>,
) -> Option<String> {
    let expected = cfg
        .remote_user
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    let daemon_user = daemon_user?;
    if daemon_user == expected {
        return None;
    }

    // allow_cross_user_daemon only suppresses the warning — it does NOT
    // bypass target/session ownership validation (that happens in
    // resolve_probe_endpoint, independently).
    if cfg.allow_cross_user_daemon {
        return None;
    }

    Some(format!(
        "daemon Unix user {daemon_user:?} does not match configured remote_user {expected:?} \
         for session {sid}. Set allow_cross_user_daemon: true (or VB_ALLOW_CROSS_USER_DAEMON=1) to override intentionally.",
        sid = session.id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionInfo;
    use serial_test::serial;

    fn session() -> SessionInfo {
        SessionInfo {
            id: "meowu-meow-40567".into(),
            port: 40567,
            pid: 0,
            host: "meowu".into(),
            user: "meow".into(),
            created: "Jun  1 08:14:16 2026".into(),
            daemon_user: None,
            daemon_version: None,
        }
    }

    /// Test helper: construct a Config with only the fields check_cross_user
    /// cares about. All other fields are set to safe defaults. This avoids
    /// mutating process environment (no #[serial] needed).
    fn test_config(remote_user: Option<&str>, allow_cross: bool) -> Config {
        Config {
            profile: None,
            remote_host: Some("test-host".into()),
            remote_user: remote_user.map(|s| s.to_string()),
            port: 40000,
            port_explicit: true,
            jump_host: None,
            jump_user: None,
            ssh_port: None,
            ssh_key: None,
            ssh_config: None,
            ssh_backend: None,
            disable_control_master: false,
            timeout: 30,
            read_timeout: 30,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: 4,
            ssh_max_sessions: 10,
            ssh_max_bulk_sessions: 2,
            ssh_reconnect_max_attempts: 3,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 0,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 5,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: crate::config::RemoteRoles::default(),
            transport_daemon_socket: None,
            transport_daemon_token: None,
            allow_cross_user_daemon: allow_cross,
        }
    }

    #[test]
    fn cross_user_match_returns_none() {
        let cfg = test_config(Some("meow"), false);
        let r = check_cross_user(&cfg, &session(), Some("meow"));
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_mismatch_returns_warning() {
        let cfg = test_config(Some("alice"), false);
        let r = check_cross_user(&cfg, &session(), Some("bob"));
        let w = r.expect("expected warning for user mismatch");
        assert!(
            w.contains("\"bob\""),
            "warning should name daemon user: {w}"
        );
        assert!(
            w.contains("\"alice\""),
            "warning should name configured user: {w}"
        );
        assert!(
            w.contains("allow_cross_user_daemon"),
            "warning should mention override: {w}"
        );
        assert!(
            w.contains("meowu-meow-40567"),
            "warning should name session: {w}"
        );
    }

    #[test]
    fn cross_user_mismatch_suppressed_by_override() {
        let cfg = test_config(Some("alice"), true);
        let r = check_cross_user(&cfg, &session(), Some("bob"));
        assert!(r.is_none(), "allow_cross_user_daemon=true should suppress");
    }

    #[test]
    fn cross_user_no_remote_user_returns_none() {
        // Config has no remote_user — nothing to compare against.
        let cfg = test_config(None, false);
        let r = check_cross_user(&cfg, &session(), Some("anyone"));
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_no_daemon_user_returns_none() {
        let cfg = test_config(Some("meow"), false);
        // daemon_user is None — we don't know, so don't warn
        let r = check_cross_user(&cfg, &session(), None);
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_whitespace_remote_user_treated_as_unset() {
        let cfg = test_config(Some("   "), false);
        let r = check_cross_user(&cfg, &session(), Some("bob"));
        assert!(
            r.is_none(),
            "whitespace-only remote_user should be treated as unset"
        );
    }

    #[test]
    fn cross_user_override_does_not_bypass_ownership() {
        // allow_cross_user_daemon only suppresses the cross-user WARNING.
        // It does NOT affect ownership validation (which happens in
        // resolve_probe_endpoint, independently). This test verifies the
        // function still returns None (suppressed) when override is on,
        // confirming the override is scoped to warning suppression only.
        let cfg = test_config(Some("alice"), true);
        let r = check_cross_user(&cfg, &session(), Some("bob"));
        assert!(r.is_none(), "override suppresses warning");
        // The config still records the mismatch — ownership is a separate concern.
        assert_eq!(cfg.remote_user.as_deref(), Some("alice"));
        assert!(cfg.allow_cross_user_daemon);
    }

    // ------------------------------------------------------------------
    // check_version_skew
    // ------------------------------------------------------------------

    #[test]
    fn version_skew_match_returns_none() {
        const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
        assert!(check_version_skew(CLI_VERSION).is_none());
    }

    #[test]
    fn version_skew_mismatch_returns_warning_with_both_versions() {
        const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
        // Pick something that's guaranteed not to equal CLI_VERSION.
        let other = if CLI_VERSION == "0.0.0" {
            "9.9.9"
        } else {
            "0.0.0"
        };
        let w = check_version_skew(other).expect("mismatch must warn");
        assert!(
            w.contains(other),
            "warning should name daemon version {other:?}: {w}"
        );
        assert!(
            w.contains(CLI_VERSION),
            "warning should name CLI version {CLI_VERSION:?}: {w}"
        );
        assert!(
            w.contains("reinstall") || w.contains("reload"),
            "warning should mention remediation: {w}"
        );
    }

    #[test]
    fn version_skew_empty_string_warns() {
        // Old daemon that never set RBDVersion reports "" — must warn so the
        // user notices they need to upgrade.
        assert!(
            check_version_skew("").is_some(),
            "empty RBDVersion should produce a skew warning"
        );
    }

    #[test]
    fn version_skew_question_mark_placeholder_warns() {
        // The .il uses "?" as the placeholder when RBDVersion is "".
        assert!(
            check_version_skew("?").is_some(),
            "'?' placeholder should produce a skew warning"
        );
    }

    // ------------------------------------------------------------------
    // resolve_probe_endpoint — behaviour acceptance tests
    // ------------------------------------------------------------------

    /// RAII guard: set VB_STATE_DIR to a temp dir for the duration of a
    /// test, restore the original value afterwards. This lets tests save
    /// and load TunnelState without polluting the user's real state dir.
    struct StateDirGuard {
        original: Option<String>,
        _tempdir: tempfile::TempDir,
    }

    impl StateDirGuard {
        fn new() -> Self {
            let original = std::env::var("VB_STATE_DIR").ok();
            let tempdir = tempfile::tempdir().expect("failed to create temp dir");
            std::env::set_var("VB_STATE_DIR", tempdir.path());
            Self {
                original,
                _tempdir: tempdir,
            }
        }
    }

    impl Drop for StateDirGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(v) => std::env::set_var("VB_STATE_DIR", v),
                None => std::env::remove_var("VB_STATE_DIR"),
            }
        }
    }

    /// Helper: construct a TunnelState with the given fields. All other
    /// fields are set to safe defaults.
    fn make_tunnel_state(
        remote_host: &str,
        remote_port: u16,
        local_port: u16,
        pid: u32,
        start_identity: Option<u64>,
        attached_session_id: Option<&str>,
        mode: Option<&str>,
    ) -> crate::models::TunnelState {
        crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: local_port,
            pid,
            remote_host: remote_host.into(),
            setup_path: None,
            profile: None,
            backend: None,
            daemon_nonce: None,
            executable_path: None,
            start_identity,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: mode.map(|s| s.into()),
            attached_remote_port: Some(remote_port),
            remote_bridge_port: Some(remote_port),
            attached_session_id: attached_session_id.map(|s| s.into()),
        }
    }

    /// Helper: construct a remote Config + CommandContext for testing.
    fn remote_ctx(remote_host: &str, port: u16) -> CommandContext {
        let mut cfg = test_config(None, false);
        cfg.remote_host = Some(remote_host.into());
        cfg.port = port;
        cfg.port_explicit = true;
        CommandContext::new(cfg, Some("test-target".into())).expect("ctx")
    }

    #[test]
    fn probe_local_direct_skipped_when_session_is_remote() {
        // Local config (no remote_host) + a session whose host is a remote
        // machine → must NOT probe (could connect to a wrong local service).
        let mut cfg = test_config(None, false);
        cfg.remote_host = None; // truly local config
        let ctx = CommandContext::new(cfg, None).expect("ctx");
        let mut s = session();
        s.host = "remote-eda-01".into();

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "local_ownership_unverified"
                })
            ),
            "remote session in local config should be skipped, got {r:?}"
        );
    }

    #[test]
    fn probe_local_direct_skipped_when_port_unreachable() {
        // Local config + local session + port not bound → skip.
        let mut cfg = test_config(None, false);
        cfg.remote_host = None; // truly local config
        let ctx = CommandContext::new(cfg, None).expect("ctx");
        let mut s = session();
        s.host = "localhost".into();
        s.port = 1; // effectively never bound

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "local_port_unreachable"
                })
            ),
            "unreachable local port should be skipped, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_no_tunnel_returns_no_tunnel() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "no_tunnel"
                })
            ),
            "no tunnel state should return no_tunnel, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_tunnel_deployed_not_attached() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            12345,
            Some(999),
            Some(&s.id),
            Some("deployed"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_not_attached"
                })
            ),
            "deployed tunnel should return tunnel_not_attached, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_session_id_mismatch() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            12345,
            Some(999),
            Some("different-session-id"),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "session_id_mismatch"
                })
            ),
            "different attached_session_id should return session_id_mismatch, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_host_mismatch() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        // State points at a DIFFERENT remote host than the session.
        let state = make_tunnel_state(
            "remote-eda-02",
            40000,
            14000,
            12345,
            Some(999),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_host_mismatch"
                })
            ),
            "state host mismatch should return tunnel_host_mismatch, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_remote_port_mismatch() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        // State's remote_bridge_port differs from session.port.
        let state = make_tunnel_state(
            "remote-eda-01",
            40099, // wrong port
            14000,
            12345,
            Some(999),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_remote_port_mismatch"
                })
            ),
            "state remote port mismatch should return tunnel_remote_port_mismatch, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_no_pid() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            0, // no PID
            Some(999),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_no_pid"
                })
            ),
            "pid=0 should return tunnel_no_pid, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_identity_unverified_when_start_identity_missing() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            std::process::id(),
            None, // no start_identity
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_identity_unverified"
                })
            ),
            "missing start_identity should return tunnel_identity_unverified, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_process_dead_for_nonexistent_pid() {
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            99999, // PID that (almost certainly) doesn't exist
            Some(1),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_process_dead"
                })
            ),
            "nonexistent PID should return tunnel_process_dead, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_current_process_is_not_ssh() {
        // The current test process is not ssh — classify_ssh_pid should
        // return NotVerifiable, causing tunnel_process_not_ssh.
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let my_pid = std::process::id();
        let my_identity = crate::transport::identity::ProcessIdentity::of_pid(my_pid)
            .map(|id| id.start_identity)
            .unwrap_or(1);

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            my_pid,
            Some(my_identity),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_process_not_ssh"
                })
            ),
            "current process (not ssh) should return tunnel_process_not_ssh, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn probe_remote_ownership_mismatch_skipped_before_tunnel_check() {
        // Session host doesn't match target → ownership_mismatch, even if a
        // tunnel state exists.
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-99".into(); // wrong host
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-99",
            40000,
            14000,
            12345,
            Some(999),
            Some(&s.id),
            Some("attached"),
        );
        state.save_with_profile(None).expect("save state");

        let r = resolve_probe_endpoint(&ctx, &s);
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "ownership_mismatch"
                })
            ),
            "session host mismatch should return ownership_mismatch, got {r:?}"
        );
    }

    // ------------------------------------------------------------------
    // hostnames_match — short-name / FQDN equivalence
    // ------------------------------------------------------------------

    #[test]
    fn hostnames_match_exact() {
        assert!(hostnames_match("compute-eda-42", "compute-eda-42"));
    }

    #[test]
    fn hostnames_match_short_vs_fqdn() {
        assert!(hostnames_match(
            "compute-eda-42",
            "compute-eda-42.internal.corp"
        ));
        assert!(hostnames_match(
            "compute-eda-42.internal.corp",
            "compute-eda-42"
        ));
    }

    #[test]
    fn hostnames_match_different_hosts() {
        assert!(!hostnames_match("compute-eda-42", "compute-eda-43"));
        assert!(!hostnames_match(
            "compute-eda-42.internal.corp",
            "compute-eda-43.internal.corp"
        ));
    }

    #[test]
    fn hostnames_match_empty_short_name() {
        // A hostname that is just ".domain" has empty short name — must not
        // match everything.
        assert!(!hostnames_match(".example.com", "anything"));
    }

    // ------------------------------------------------------------------
    // resolve_probe_endpoint_with_state — same ID, wrong tunnel
    // ------------------------------------------------------------------

    #[test]
    #[serial]
    fn with_state_same_id_but_wrong_remote_host_is_not_verified() {
        // TunnelState has matching attached_session_id but points at a
        // DIFFERENT remote host than the session. The shared chain must
        // reject it (tunnel_host_mismatch) — list must NOT mark this
        // tunnel_verified=true.
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        // State: same session ID, but remote_host is wrong.
        let state = make_tunnel_state(
            "remote-eda-99", // wrong host
            40000,
            14000,
            std::process::id(),
            Some(1),
            Some(&s.id),
            Some("attached"),
        );

        let r = resolve_probe_endpoint_with_state(&ctx, &s, Some(&state));
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_host_mismatch"
                })
            ),
            "wrong remote host must be rejected, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn with_state_same_id_but_wrong_remote_port_is_not_verified() {
        // TunnelState has matching session_id and host, but remote_bridge_port
        // differs from session.port. Must reject.
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40999, // wrong remote port
            14000,
            std::process::id(),
            Some(1),
            Some(&s.id),
            Some("attached"),
        );

        let r = resolve_probe_endpoint_with_state(&ctx, &s, Some(&state));
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_remote_port_mismatch"
                })
            ),
            "wrong remote port must be rejected, got {r:?}"
        );
    }

    #[test]
    #[serial]
    fn with_state_dead_process_is_not_verified() {
        // TunnelState passes host/port/id checks but the recorded PID is
        // dead. Must reject with tunnel_process_dead.
        let _guard = StateDirGuard::new();
        let ctx = remote_ctx("remote-eda-01", 40000);
        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;

        let state = make_tunnel_state(
            "remote-eda-01",
            40000,
            14000,
            99999, // dead PID
            Some(1),
            Some(&s.id),
            Some("attached"),
        );

        let r = resolve_probe_endpoint_with_state(&ctx, &s, Some(&state));
        assert!(
            matches!(
                r,
                Err(ProbeSkip {
                    reason: "tunnel_process_dead"
                })
            ),
            "dead PID must be rejected, got {r:?}"
        );
    }

    // ------------------------------------------------------------------
    // CacheDirGuard — redirect VB_CACHE_DIR for list/show integration tests
    // ------------------------------------------------------------------

    struct CacheDirGuard {
        original: Option<String>,
        _tempdir: tempfile::TempDir,
    }

    impl CacheDirGuard {
        fn new() -> Self {
            let original = std::env::var("VB_CACHE_DIR").ok();
            let tempdir = tempfile::tempdir().expect("failed to create temp dir");
            std::env::set_var("VB_CACHE_DIR", tempdir.path());
            Self {
                original,
                _tempdir: tempdir,
            }
        }
    }

    impl Drop for CacheDirGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(v) => std::env::set_var("VB_CACHE_DIR", v),
                None => std::env::remove_var("VB_CACHE_DIR"),
            }
        }
    }

    // ------------------------------------------------------------------
    // list — integration tests
    // ------------------------------------------------------------------

    #[test]
    #[serial]
    fn list_target_mode_does_not_delete_dead_session_files() {
        // In target mode, list must show ALL cached sessions and NEVER
        // delete files — even if the local port is unreachable.
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        // Create a session file with an unreachable port.
        let mut s = session();
        s.port = 1; // unreachable
        s.save_to_session_file();

        let ctx = remote_ctx("remote-eda-01", 40000);
        let result = list(&ctx, OutputFormat::Json).expect("list should succeed");

        // The session should appear in the list.
        let sessions = result["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1, "dead session should still be listed");
        assert_eq!(sessions[0]["id"], s.id);

        // probe_status should be not_probed (tunnel_verified=false).
        assert_eq!(sessions[0]["probe_status"], "not_probed");
        assert_eq!(sessions[0]["tunnel_verified"], false);

        // File must still exist (not deleted).
        let path = SessionInfo::sessions_dir().join(format!("{}.json", s.id));
        assert!(path.exists(), "session file must NOT be deleted by list");
    }

    #[test]
    #[serial]
    fn list_target_mode_shows_unattached_remote_session() {
        // A remote session that has no attached tunnel must still appear
        // in target-mode list, with tunnel_verified=false and
        // probe_status=not_probed.
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;
        s.save_to_session_file();

        // No TunnelState saved → unattached.
        let ctx = remote_ctx("remote-eda-01", 40000);
        let result = list(&ctx, OutputFormat::Json).expect("list should succeed");

        let sessions = result["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["tunnel_verified"], false);
        assert_eq!(sessions[0]["probe_status"], "not_probed");
        // endpoint_match should be "match" (host+port match the target).
        assert_eq!(sessions[0]["endpoint_match"], "match");
    }

    // ------------------------------------------------------------------
    // show — integration tests
    // ------------------------------------------------------------------

    #[test]
    #[serial]
    fn show_skipped_probe_preserves_cached_metadata() {
        // When probe is skipped (no tunnel), show must return the cached
        // daemon_user / daemon_version from the session file, with
        // data_source="cached" and daemon_responsive=null.
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;
        s.daemon_user = Some("cacheduser".into());
        s.daemon_version = Some("1.2.3-cached".into());
        s.save_to_session_file();

        let ctx = remote_ctx("remote-eda-01", 40000);
        let result = show(&ctx, &s.id, OutputFormat::Json).expect("show should succeed");

        // Cached values must be present.
        assert_eq!(result["session"]["daemon_user"], "cacheduser");
        assert_eq!(result["session"]["daemon_version"], "1.2.3-cached");
        assert_eq!(result["session"]["data_source"], "cached");

        // daemon_responsive must be null (not false — we never probed).
        assert!(
            result["session"]["daemon_responsive"].is_null(),
            "daemon_responsive should be null when probe skipped, got {}",
            result["session"]["daemon_responsive"]
        );

        // Probe must be marked skipped.
        assert_eq!(result["probe"]["skipped"], true);
        assert_eq!(result["probe"]["skip_reason"], "no_tunnel");
    }

    #[test]
    #[serial]
    fn show_skipped_probe_does_not_mutate_session_file() {
        // When probe is skipped, show must NOT write back to the session
        // file (no mutation).
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let mut s = session();
        s.host = "remote-eda-01".into();
        s.port = 40000;
        s.daemon_user = Some("originaluser".into());
        s.save_to_session_file();

        let path = SessionInfo::sessions_dir().join(format!("{}.json", s.id));
        let before = std::fs::read_to_string(&path).expect("read before");

        let ctx = remote_ctx("remote-eda-01", 40000);
        let _ = show(&ctx, &s.id, OutputFormat::Json).expect("show should succeed");

        let after = std::fs::read_to_string(&path).expect("read after");
        assert_eq!(
            before, after,
            "session file must NOT be mutated when probe is skipped"
        );
    }
}

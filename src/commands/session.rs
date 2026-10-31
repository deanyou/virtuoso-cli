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

/// How the identity probe ended, as seen by the cross-user check.
///
/// "Skipped" and "ran but confirmed nothing" are different facts with
/// different remedies, so the warning wording differs for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityProbeOutcome {
    /// The probe ran. Whether the identity was confirmed is carried by the
    /// daemon-user value itself.
    Ran,
    /// The probe never ran; carries the reason it was skipped.
    Skipped(&'static str),
}

/// Outcome of the phase-2 daemon probe in [`show`].
///
/// `skipped` separates "the probe never ran" from "the probe ran and
/// produced nothing" — the two must not look alike in the output.
struct ProbeOutcome {
    /// Live daemon user when the probe confirmed one; the cached value
    /// when the probe was skipped.
    daemon_user: Option<String>,
    /// Warning about a failed identity probe.
    daemon_user_warning: Option<String>,
    /// `Some(true|false)` when the liveness probe ran; `None` when the
    /// probe never ran (unknown — not "false").
    daemon_responsive: Option<bool>,
    /// Live daemon version when the probe produced one; cached otherwise.
    daemon_version: Option<String>,
    /// Warning about a failed version probe.
    daemon_version_warning: Option<String>,
    /// True when the probe did not run.
    skipped: bool,
    /// Machine-readable reason the probe was skipped.
    skip_reason: Option<&'static str>,
    /// Where the probe connected: "local" or "remote_tunnel".
    source: Option<&'static str>,
    /// The port the probe connected to.
    port: Option<u16>,
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

/// Get the system hostname in a cross-platform way.
///
/// - **Unix** (Linux, macOS, BSD): `libc::gethostname(2)`.
/// - **Windows**: `%COMPUTERNAME%` env var (libc crate does not provide
///   `gethostname` on Windows).
/// - **Other / fallback**: `$HOSTNAME` env var.
///
/// Returns None on failure or if the hostname is empty.
fn system_hostname() -> Option<String> {
    #[cfg(unix)]
    {
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
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty())
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty())
    }
}

/// Returns true if `s` parses as an IP address (IPv4 or IPv6). IP addresses
/// must always be compared exactly — no short-name matching.
fn is_ip_address(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// Compare two hostnames with strict matching rules.
///
/// - **Exact match** always wins.
/// - **IP addresses** (either side) must match exactly — no truncation.
/// - **Two fully-qualified names** (both contain a dot) must match exactly.
///   `compute-42.prod.example` does NOT match `compute-42.test.example`.
/// - **Short name vs FQDN**: if exactly one side has no dot (a short
///   hostname), compare the short side against the first label of the FQDN.
///   `compute-42` matches `compute-42.internal.corp`.
fn hostnames_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // IP addresses: exact match only.
    if is_ip_address(a) || is_ip_address(b) {
        return false;
    }
    let a_has_dot = a.contains('.');
    let b_has_dot = b.contains('.');
    // Both are FQDNs: exact match only (already failed above).
    if a_has_dot && b_has_dot {
        return false;
    }
    // At least one side is a short hostname (no dot). Allow first-label
    // comparison.
    let a_short = a.split('.').next().unwrap_or(a);
    let b_short = b.split('.').next().unwrap_or(b);
    !a_short.is_empty() && a_short == b_short
}

/// Check whether a session belongs to this machine, given the config.
///
/// - If `cfg.remote_host` is set, the session is local only if its host
///   matches exactly.
/// - If `cfg.remote_host` is unset, we check known local hostnames
///   (`localhost`, `127.0.0.1`, `::1`), then the system hostname (via
///   `system_hostname()`, with strict `hostnames_match()` rules). If the
///   system hostname is available but does NOT match, we return false
///   immediately — `$HOSTNAME` is not allowed to override an authoritative
///   system-name mismatch.
/// - `$HOSTNAME` is consulted only when the system hostname is unavailable.
/// - Anything that cannot be confirmed is treated as NOT local (skip probe).
fn is_local_session(cfg: &Config, session: &SessionInfo) -> bool {
    const LOCAL_HOSTNAMES: &[&str] = &["localhost", "127.0.0.1", "::1"];

    if let Some(host) = &cfg.remote_host {
        return session.host == *host;
    }

    if LOCAL_HOSTNAMES.contains(&session.host.as_str()) {
        return true;
    }

    // System hostname (authoritative — does not depend on process env).
    // If available and non-empty, its verdict is final: a mismatch here is
    // NOT overridden by $HOSTNAME below.
    if let Some(sys_host) = system_hostname() {
        if !sys_host.is_empty() {
            return hostnames_match(&sys_host, &session.host);
        }
    }

    // $HOSTNAME fallback — only when system hostname is unavailable.
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
                // Distinguish local direct vs remote tunnel:
                // - Local direct: tunnel_verified=false (no tunnel exists),
                //   probe_status=reachable/unreachable based on port check.
                // - Remote tunnel: tunnel_verified=true only when the full
                //   6-step chain passes (or the only failure is
                //   forward_port_unreachable, meaning identity verified but
                //   port is down).
                // - Any other failure → tunnel_verified=false, not_probed.
                let (tunnel_verified, probe_status) = match probe_result {
                    Ok(ProbeEndpoint::Local { .. }) => (false, "reachable"),
                    Ok(ProbeEndpoint::RemoteTunnel { .. }) => (true, "reachable"),
                    Err(ProbeSkip {
                        reason: "forward_port_unreachable",
                    }) => (true, "unreachable"),
                    Err(ProbeSkip {
                        reason: "local_port_unreachable",
                    }) => (false, "unreachable"),
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

/// Show the current (active) session for the selected target.
///
/// Uses the same shared verification chain as `list` / `show`
/// (`resolve_probe_endpoint`) rather than the legacy `list_alive()` which
/// only checks local port reachability and can misclassify remote sessions.
///
/// Target mode: only considers sessions whose endpoint matches the current
/// target. Legacy mode: considers all locally-reachable sessions (preserves
/// existing behaviour for non-target invocations).
pub fn current(ctx: &CommandContext) -> Result<Value> {
    let all = SessionInfo::list().unwrap_or_default();
    let is_target = ctx.target_id().is_some();

    let mut reachable: Vec<&SessionInfo> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new(); // (id, reason)

    for s in &all {
        // Target mode: skip sessions that don't match the target endpoint.
        if is_target {
            match compute_endpoint_match(ctx, s) {
                "match" => {}
                "mismatch" => {
                    skipped.push((s.id.clone(), "endpoint_mismatch".into()));
                    continue;
                }
                _ => {
                    // Legacy/unverifiable — still consider if reachable.
                }
            }
        }

        // Determine liveness.
        //
        // Legacy mode (no target): preserve the original `list_alive()` behaviour
        // — a session whose local port is reachable is alive.
        //
        // Target mode: use the shared verification chain. Only sessions whose
        // probe endpoint resolves successfully are considered reachable.
        let is_reachable = if is_target {
            resolve_probe_endpoint(ctx, s).is_ok()
        } else {
            s.is_alive()
        };

        if is_reachable {
            reachable.push(s);
        } else if is_target {
            // In target mode, record why a matching session was not reachable.
            if let Err(skip) = resolve_probe_endpoint(ctx, s) {
                skipped.push((s.id.clone(), skip.reason.into()));
            }
        }
    }

    match reachable.len() {
        0 => Ok(json!({
            "status": "success",
            "session": null,
            "target": ctx.target_id(),
            "note": if is_target {
                "no reachable sessions for this target; check tunnel attach"
            } else {
                "no live sessions; VB_PORT will be used"
            },
            "skipped_count": skipped.len(),
        })),
        1 => Ok(json!({
            "status": "success",
            "session": reachable[0].id,
            "port": reachable[0].port,
            "host": reachable[0].host,
            "target": ctx.target_id(),
            "auto_selected": true,
            "endpoint_match": if is_target { "match" } else { "unchecked" },
        })),
        _ => {
            let ids: Vec<&str> = reachable.iter().map(|s| s.id.as_str()).collect();
            Ok(json!({
                "status": "ambiguous",
                "sessions": ids,
                "target": ctx.target_id(),
                "note": "use --session <id> to select one",
            }))
        }
    }
}

/// Remove dead session records.
///
/// Uses the shared verification chain (`resolve_probe_endpoint`) to determine
/// whether a session is dead, rather than the legacy `is_alive()` which only
/// checks local port reachability and can misclassify remote sessions that
/// haven't been attached yet.
///
/// Target mode: only removes sessions whose endpoint matches the current target.
/// Legacy mode: preserves existing global cleanup behaviour.
///
/// `dry_run`: when true, reports what would be removed without deleting files.
pub fn cleanup(ctx: &CommandContext, dry_run: bool) -> Result<Value> {
    let all = SessionInfo::list().unwrap_or_default();
    let dir = SessionInfo::sessions_dir();
    let is_target = ctx.target_id().is_some();

    let mut removed: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new(); // (id, reason)

    for s in &all {
        // Target mode: skip sessions that don't match the target endpoint.
        if is_target {
            match compute_endpoint_match(ctx, s) {
                "match" => {}
                "mismatch" => {
                    skipped.push((s.id.clone(), "endpoint_mismatch".into()));
                    continue;
                }
                _ => {
                    // Legacy/unverifiable — still evaluate.
                }
            }
        }

        // Determine liveness.
        //
        // Legacy mode (no target): preserve the original `is_alive()` behaviour
        // — a session whose local port is unreachable is dead. This avoids
        // breaking existing cleanup semantics for non-target invocations.
        //
        // Target mode: use the shared verification chain. A session is "dead"
        // only if we have DEFINITIVE evidence that the remote daemon is down.
        // `forward_port_unreachable` does NOT qualify: the full tunnel chain
        // passed (SSH process alive, identity verified, not reused) — only the
        // local forward leg is unreachable. That could be a local firewall /
        // forwarding issue while the remote daemon is still healthy. Deleting
        // the cached session here would lose a recoverable entry. Preserve the
        // file and report "cannot_verify" so the operator can investigate.
        let (is_dead, skip_reason) = if is_target {
            match resolve_probe_endpoint(ctx, s) {
                Ok(_) => (false, None),
                Err(skip) => {
                    let is_definitive_death = skip.reason == "local_port_unreachable";
                    let reason: String = if is_definitive_death {
                        skip.reason.into()
                    } else if skip.reason == "forward_port_unreachable" {
                        // Tunnel verified, local leg down — cannot confirm
                        // remote death. Preserve, don't delete.
                        "cannot_verify:forward_port_unreachable".into()
                    } else {
                        // Other skip reasons (no_tunnel, mismatch, etc.) —
                        // session doesn't match this context OR tunnel state
                        // is incomplete; preserve the cache.
                        format!("cannot_verify:{}", skip.reason)
                    };
                    (is_definitive_death, Some(reason))
                }
            }
        } else {
            (!s.is_alive(), None)
        };

        if is_dead {
            if dry_run {
                removed.push(s.id.clone());
            } else {
                let path = dir.join(format!("{}.json", s.id));
                if std::fs::remove_file(&path).is_ok() {
                    removed.push(s.id.clone());
                } else {
                    skipped.push((s.id.clone(), "remove_failed".into()));
                }
            }
        } else {
            let reason = match skip_reason {
                Some(r) => format!("skipped:{}", r),
                None => "reachable".into(),
            };
            skipped.push((s.id.clone(), reason));
        }
    }

    Ok(json!({
        "status": "success",
        "dry_run": dry_run,
        "target": ctx.target_id(),
        "removed": removed.len(),
        "sessions": removed,
        "skipped_count": skipped.len(),
        "skipped": skipped.iter().map(|(id, reason)| json!({"id": id, "reason": reason})).collect::<Vec<_>>(),
    }))
}

/// Show command/SKILL history for a session.
///
/// Read-only: history files are per-session logs and don't require a live
/// connection. The session's endpoint ownership is verified against the
/// current target context; a mismatch surfaces as a warning but does not
/// block reading (history is local cache, not a connection operation).
pub fn history(
    ctx: &CommandContext,
    id: &str,
    only_skill: bool,
    only_cmd: bool,
    limit: usize,
) -> Result<Value> {
    let show_skill = !only_cmd;
    let show_cmd = !only_skill;

    // Load session metadata for ownership verification (best-effort).
    let session_meta = SessionInfo::load(id).ok();
    let (endpoint_match, ownership_warning) = match (&session_meta, ctx.target_id()) {
        (Some(s), Some(_)) => {
            let m = compute_endpoint_match(ctx, s);
            let warning = if m == "mismatch" {
                Some(format!(
                    "session {} endpoint does not match target {}; showing history anyway",
                    id,
                    ctx.target_id().unwrap_or_default()
                ))
            } else {
                None
            };
            (m.to_string(), warning)
        }
        _ => ("unchecked".to_string(), None),
    };

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

    let mut result = json!({
        "status": "success",
        "session": id,
        "target": ctx.target_id(),
        "endpoint_match": endpoint_match,
        "skill_count": skill_entries.len(),
        "cmd_count": cmd_entries.len(),
        "skill": skill_entries,
        "cmd": cmd_entries,
    });

    if let Some(warning) = ownership_warning {
        result["ownership_warning"] = json!(warning);
    }

    Ok(result)
}

/// Show details for a specific session.
///
/// Two-phase behaviour:
///
/// **Phase 1 (always):** load the session file and present cached metadata.
/// Errors here are real (file missing / corrupt → `NotFound`).
///
/// **Phase 2 (only when `resolve_probe_endpoint` succeeds):** connect to the
/// verified local port, check liveness first, then query the daemon for
/// `$USER` / version, and write fresh values back to the session file. Any
/// verification failure (ownership mismatch, no tunnel, dead process, dead
/// daemon, corrupt state, …) sets `probe_skipped: true` with a
/// `probe_skip_reason` — the cached view is still returned, never an error.
pub fn show(ctx: &CommandContext, id: &str, _format: OutputFormat) -> Result<Value> {
    // Phase 1: load session. File missing / corrupt is a real error.
    let s = SessionInfo::load(id)
        .map_err(|e| VirtuosoError::NotFound(format!("session '{id}' not found: {e}")))?;

    let endpoint_match = compute_endpoint_match(ctx, &s);

    // Phase 2: resolve a probe endpoint, then probe if allowed.
    // daemon_responsive is Option<bool>: Some(_) = liveness probe ran,
    // None = the probe never ran. daemon_user / daemon_version hold the LIVE
    // probe values when the probe succeeded; the cached values from the
    // session file are shown when it was skipped.
    let probe = match resolve_probe_endpoint(ctx, &s) {
        Ok(endpoint) => {
            let (port, source) = match &endpoint {
                ProbeEndpoint::Local { port } => (*port, "local"),
                ProbeEndpoint::RemoteTunnel { port } => (*port, "remote_tunnel"),
            };

            // The probe client inherits the resolved config timeout
            // (VB_TIMEOUT): the daemon-identity probe must get the same
            // headroom the user granted the rest of the session, not a
            // hardcoded cap. The liveness probe keeps its own short budget
            // (daemon_alive passes Some(3)).
            let client = VirtuosoClient::new("127.0.0.1", port, ctx.config().timeout);

            // LIVENESS FIRST. A dead or wedged daemon must not make the
            // user wait out the whole identity-probe budget before they
            // learn that the daemon is gone: the liveness probe has its own
            // short budget, so a dead daemon is detected in seconds instead
            // of after VB_TIMEOUT.
            let alive = client.daemon_alive();
            if !alive {
                // Daemon is not answering: skip the identity and version
                // probes entirely. The skip is reported through the
                // existing probe.skip_reason channel; the cached values are
                // shown (data_source="cached").
                ProbeOutcome {
                    daemon_user: s.daemon_user.clone(),
                    daemon_user_warning: None,
                    daemon_responsive: Some(false),
                    daemon_version: s.daemon_version.clone(),
                    daemon_version_warning: None,
                    skipped: true,
                    skip_reason: Some("daemon_not_alive"),
                    source: Some(source),
                    port: Some(port),
                }
            } else {
                let user_result = client.get_daemon_user();
                let version_result = client.get_daemon_version();
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

                ProbeOutcome {
                    daemon_user: user,
                    daemon_user_warning: user_warn,
                    daemon_responsive: Some(true),
                    daemon_version: ver,
                    daemon_version_warning: ver_warn,
                    skipped: false,
                    skip_reason: None,
                    source: Some(source),
                    port: Some(port),
                }
            }
        }
        Err(skip) => ProbeOutcome {
            // Probe skipped: use cached values from the session file.
            daemon_user: s.daemon_user.clone(),
            daemon_user_warning: None,
            daemon_responsive: None, // responsive unknown — not "false"
            daemon_version: s.daemon_version.clone(),
            daemon_version_warning: None,
            skipped: true,
            skip_reason: Some(skip.reason),
            source: None,
            port: None,
        },
    };

    // Cross-user check uses the resolved config (never re-reads env).
    let probe_outcome = match (probe.skipped, probe.skip_reason) {
        (true, Some(reason)) => IdentityProbeOutcome::Skipped(reason),
        (true, None) => IdentityProbeOutcome::Skipped("unknown"),
        (false, _) => IdentityProbeOutcome::Ran,
    };
    let cross_user_warning = check_cross_user(
        ctx.config(),
        &s,
        probe.daemon_user.as_deref(),
        probe_outcome,
    );

    // Stale-daemon hint when the liveness probe ran and the daemon did not
    // answer (daemon_responsive == Some(false)). The identity and version
    // probes are skipped in that case, but the hint must still fire: the
    // port is bound while the daemon itself is wedged.
    let stale_daemon_hint = if probe.daemon_responsive == Some(false) {
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

    // Write back only when the probe RAN. A probe that ran and produced NO
    // confirmed identity must CLEAR the stored daemon_user: the connect-time
    // guard compares the stored value, so a stale one would be compared as if
    // it had been verified. A skipped probe never mutates the cache —
    // nothing was learned either way.
    if !probe.skipped {
        let mut s_mut = s.clone();
        let mut dirty = false;
        match probe.daemon_user.as_ref() {
            Some(u) => {
                if s_mut.daemon_user.as_deref() != Some(u.as_str()) {
                    s_mut.daemon_user = Some(u.clone());
                    dirty = true;
                }
            }
            // Identity unconfirmed → drop the stale cached value instead of
            // leaving it to look verified.
            None => {
                if s_mut.daemon_user.is_some() {
                    s_mut.daemon_user = None;
                    dirty = true;
                }
            }
        }
        if let Some(v) = probe.daemon_version.as_ref() {
            if s_mut.daemon_version.as_deref() != Some(v.as_str()) {
                s_mut.daemon_version = Some(v.clone());
                dirty = true;
            }
        }
        if dirty {
            s_mut.save_to_session_file();
        }
    }

    let has_warnings = cross_user_warning.is_some()
        || probe.daemon_version_warning.is_some()
        || probe.daemon_user_warning.is_some()
        || stale_daemon_hint.is_some();

    // data_source: "live" when probe ran, "cached" when skipped (values
    // come from the session file).
    let data_source = if probe.skipped { "cached" } else { "live" };

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
            "daemon_responsive": probe.daemon_responsive,
            "daemon_user": probe.daemon_user,
            "daemon_version": probe.daemon_version,
            "data_source": data_source,
            "cli_version": env!("CARGO_PKG_VERSION"),
        },
        "probe": {
            "skipped": probe.skipped,
            "skip_reason": probe.skip_reason,
            "source": probe.source,
            "port": probe.port,
        },
        "warnings": {
            "daemon_user": probe.daemon_user_warning,
            "cross_user": cross_user_warning,
            "version_skew": probe.daemon_version_warning,
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
/// Returns `Some(warning)` when the daemon user could NOT be confirmed
/// (the identity probe returned no user) while an expected user *is*
/// configured, and `Some(warning)` on an actual mismatch. Returns `None`
/// when the users match, when no expected user is configured, or when the
/// mismatch is explicitly overridden.
///
/// "Unconfirmed" and "mismatch" are different facts and get different
/// wording: an unconfirmed identity means the cross-user check did not run
/// at all (the probe timed out or the daemon reported no `USER`), which is
/// not the same as "the daemon runs as somebody else".
///
/// Set `allow_cross_user_daemon: true` in config (env: `VB_ALLOW_CROSS_USER_DAEMON=1`,
/// or target config field) to suppress the mismatch warning. It does not
/// suppress the unconfirmed-identity warning: that override authorises a
/// *different* daemon user, it cannot confirm an *unknown* one.
///
/// Uses the resolved `Config` — never re-reads environment variables.
/// `Config::from_env()` already handles the `VB_REMOTE_USER_<profile>` →
/// `VB_REMOTE_USER` fallback, so this function simply uses `cfg.remote_user`.
///
/// The `probe` outcome decides the wording of the UNCONFIRMED warning: a
/// probe that was skipped and a probe that ran but confirmed nothing are
/// different facts with different remedies. Telling the user to run
/// `vcli session show` again — this very command — would be useless.
fn check_cross_user(
    cfg: &Config,
    session: &SessionInfo,
    daemon_user: Option<&str>,
    probe: IdentityProbeOutcome,
) -> Option<String> {
    let expected = cfg
        .remote_user
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    // Expected user configured, but the daemon user is unknown → the
    // cross-user check was NOT performed. Say so, instead of returning
    // "no warning" and letting an unchecked identity look verified.
    let daemon_user = match daemon_user {
        Some(u) => u,
        None => {
            return Some(match probe {
                IdentityProbeOutcome::Skipped(reason) => format!(
                    "daemon Unix user unconfirmed for session {sid}: the identity probe was \
                     skipped ({reason}), so the cross-user check against configured remote_user \
                     {expected:?} was NOT performed (UNCONFIRMED identity — not a mismatch).",
                    sid = session.id
                ),
                IdentityProbeOutcome::Ran => format!(
                    "daemon Unix user unconfirmed for session {sid}: the identity probe ran but \
                     returned no user, so the cross-user check against configured remote_user \
                     {expected:?} was NOT performed (UNCONFIRMED identity — not a mismatch). \
                     Check the daemon load, or raise VB_TIMEOUT so the probe gets enough budget.",
                    sid = session.id
                ),
            })
        }
    };
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
        let r = check_cross_user(&cfg, &session(), Some("meow"), IdentityProbeOutcome::Ran);
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_mismatch_returns_warning() {
        let cfg = test_config(Some("alice"), false);
        let r = check_cross_user(&cfg, &session(), Some("bob"), IdentityProbeOutcome::Ran);
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
        let r = check_cross_user(&cfg, &session(), Some("bob"), IdentityProbeOutcome::Ran);
        assert!(r.is_none(), "allow_cross_user_daemon=true should suppress");
    }

    #[test]
    fn cross_user_no_remote_user_returns_none() {
        // Config has no remote_user — nothing to compare against.
        let cfg = test_config(None, false);
        let r = check_cross_user(&cfg, &session(), Some("anyone"), IdentityProbeOutcome::Ran);
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_unconfirmed_daemon_user_returns_warning() {
        // An expected user IS configured but the daemon user could not be
        // determined (identity probe returned no USER). The check was NOT
        // performed — that must not look like "no problem found".
        let cfg = test_config(Some("meow"), false);
        let r = check_cross_user(&cfg, &session(), None, IdentityProbeOutcome::Ran);
        let w = r.expect("unconfirmed identity must warn");
        let lower = w.to_lowercase();
        assert!(
            lower.contains("unconfirmed"),
            "warning must mark the identity as unconfirmed: {w}"
        );
        assert!(
            lower.contains("not performed"),
            "warning must say the check was not performed: {w}"
        );
        assert!(
            w.contains("meowu-meow-40567"),
            "warning should name session: {w}"
        );
        assert!(
            w.contains("\"meow\""),
            "warning should name the configured user: {w}"
        );
        assert!(
            !lower.contains("does not match"),
            "unconfirmed must be distinguishable from a mismatch: {w}"
        );
    }

    /// A SKIPPED probe and a FAILED probe are different facts: the skipped
    /// one must cite the actual skip reason and must not tell the user to
    /// re-run the very command that produced the warning.
    #[test]
    fn cross_user_skipped_probe_cites_skip_reason_without_reprobe_advice() {
        let cfg = test_config(Some("meow"), false);
        let r = check_cross_user(
            &cfg,
            &session(),
            None,
            IdentityProbeOutcome::Skipped("no_tunnel"),
        );
        let w = r.expect("unconfirmed identity must warn");
        let lower = w.to_lowercase();
        assert!(
            lower.contains("unconfirmed"),
            "warning must mark the identity as unconfirmed: {w}"
        );
        assert!(
            lower.contains("not performed"),
            "warning must say the check was not performed: {w}"
        );
        assert!(
            w.contains("no_tunnel"),
            "warning must cite the actual skip reason: {w}"
        );
        assert!(
            !lower.contains("re-probe"),
            "a skipped probe must not advise re-probing: {w}"
        );
        assert!(
            !lower.contains("session show"),
            "a skipped probe must not tell the user to run this command again: {w}"
        );
    }

    /// A probe that ran and confirmed nothing must point at the two real
    /// remedies: a busy daemon, or a too-small VB_TIMEOUT.
    #[test]
    fn cross_user_failed_probe_advises_daemon_load_or_timeout() {
        let cfg = test_config(Some("meow"), false);
        let r = check_cross_user(&cfg, &session(), None, IdentityProbeOutcome::Ran);
        let w = r.expect("unconfirmed identity must warn");
        let lower = w.to_lowercase();
        assert!(
            lower.contains("daemon load"),
            "warning must advise checking the daemon load: {w}"
        );
        assert!(
            lower.contains("vb_timeout"),
            "warning must advise raising VB_TIMEOUT: {w}"
        );
        assert!(
            !lower.contains("re-probe"),
            "a failed probe must not advise re-probing this command: {w}"
        );
    }

    #[test]
    fn cross_user_no_remote_user_and_no_daemon_user_returns_none() {
        // Nothing configured to compare against AND no daemon user — there
        // is no check to run, so there is nothing to warn about.
        let cfg = test_config(None, false);
        let r = check_cross_user(&cfg, &session(), None, IdentityProbeOutcome::Ran);
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_whitespace_remote_user_treated_as_unset() {
        let cfg = test_config(Some("   "), false);
        let r = check_cross_user(&cfg, &session(), Some("bob"), IdentityProbeOutcome::Ran);
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
        let r = check_cross_user(&cfg, &session(), Some("bob"), IdentityProbeOutcome::Ran);
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

    /// Helper: construct a local-target CommandContext (no remote_host).
    fn local_ctx() -> CommandContext {
        let mut cfg = test_config(None, false);
        cfg.remote_host = None;
        cfg.port = 0;
        cfg.port_explicit = false;
        CommandContext::new(cfg, Some("local-target".into())).expect("ctx")
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

    #[test]
    fn hostnames_match_different_domains_same_short_label() {
        // Two FQDNs with the same first label but different domains must
        // NOT match. compute-42.prod.example ≠ compute-42.test.example.
        assert!(!hostnames_match(
            "compute-42.prod.example",
            "compute-42.test.example"
        ));
        assert!(!hostnames_match(
            "compute-42.prod.example",
            "compute-42.prod.other"
        ));
    }

    #[test]
    fn hostnames_match_different_ips() {
        // IP addresses must match exactly — no first-octet truncation.
        assert!(!hostnames_match("192.168.1.1", "192.168.1.2"));
        assert!(!hostnames_match("10.0.0.1", "10.0.0.2"));
        // IP vs hostname with same first label must not match.
        assert!(!hostnames_match("192.168.1.1", "192"));
    }

    #[test]
    fn hostnames_match_short_vs_fqdn_same_label() {
        // A short hostname DOES match an FQDN whose first label is the same.
        assert!(hostnames_match("compute-42", "compute-42.internal.corp"));
        assert!(hostnames_match("compute-42.internal.corp", "compute-42"));
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

    #[test]
    #[serial]
    fn list_local_target_marks_direct_connection_not_tunnel() {
        // A local target (no remote_host) with a reachable local session must
        // show tunnel_verified=false (no SSH tunnel exists — this is a direct
        // local connection) and probe_status=reachable.
        let _cache = CacheDirGuard::new();

        // Bind a real listener to simulate a local daemon port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();

        let mut s = session();
        s.host = "localhost".into();
        s.port = port;
        s.save_to_session_file();

        let ctx = local_ctx();
        let result = list(&ctx, OutputFormat::Json).expect("list should succeed");

        let sessions = result["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1, "local session should be listed");
        // Local direct connection: tunnel_verified must be false.
        assert_eq!(
            sessions[0]["tunnel_verified"], false,
            "local direct connection must not be marked tunnel_verified"
        );
        assert_eq!(
            sessions[0]["probe_status"], "reachable",
            "reachable local port should show probe_status=reachable"
        );
    }

    #[test]
    #[serial]
    fn list_local_target_unreachable_port_shows_unreachable() {
        // A local target with an unreachable local port must show
        // tunnel_verified=false and probe_status=unreachable (not
        // not_probed — we did attempt the probe).
        let _cache = CacheDirGuard::new();

        let mut s = session();
        s.host = "localhost".into();
        s.port = 1; // effectively never bound
        s.save_to_session_file();

        let ctx = local_ctx();
        let result = list(&ctx, OutputFormat::Json).expect("list should succeed");

        let sessions = result["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["tunnel_verified"], false);
        assert_eq!(
            sessions[0]["probe_status"], "unreachable",
            "unreachable local port should show probe_status=unreachable, not not_probed"
        );
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

        // Explicitly: the cached daemon_user survives a skipped probe —
        // nothing was learned, so nothing is claimed and nothing is cleared.
        let reloaded = SessionInfo::load(&s.id).expect("reload session");
        assert_eq!(
            reloaded.daemon_user.as_deref(),
            Some("originaluser"),
            "a skipped probe must keep the cached daemon_user"
        );
    }

    /// How a [`spawn_fake_daemon`] fake daemon answers.
    #[derive(Clone, Copy)]
    enum FakeDaemonMode {
        /// The daemon is wedged: every SKILL request is answered with a
        /// NAK (transport error), so the liveness probe fails.
        Wedged,
        /// The daemon answers everything, but the identity probe returns
        /// `nil` — no `USER` on the daemon side, so the identity stays
        /// unconfirmed.
        NoIdentity,
    }

    /// Fake daemon that records every SKILL request it receives.
    ///
    /// Returns the port, the recorded requests, and a keep-alive handle
    /// (the listener thread ends when it is dropped).
    fn spawn_fake_daemon(
        mode: FakeDaemonMode,
    ) -> (
        u16,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        std::sync::Arc<()>,
    ) {
        use std::io::{Read, Write};
        use std::net::Shutdown;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let keep = std::sync::Arc::new(());
        let seen2 = std::sync::Arc::clone(&seen);
        let keep2 = std::sync::Arc::clone(&keep);
        // The version answer matches the CLI version so the version-skew
        // warning does not pollute the assertions.
        let version = format!("\"{}\"", env!("CARGO_PKG_VERSION"));
        std::thread::spawn(move || {
            let _keep = keep2;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                if n == 0 {
                    // Reachability probe (no payload) — nothing to answer.
                    continue;
                }
                seen2
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(request.clone());

                let (marker, payload): (u8, &[u8]) = match mode {
                    FakeDaemonMode::Wedged => (0x15, b"wedged"),
                    FakeDaemonMode::NoIdentity => {
                        if request.contains("getShellEnvVar") {
                            // STX + nil → the identity stays unconfirmed.
                            (0x02, b"nil")
                        } else if request.contains("RBDVersion") {
                            (0x02, version.as_bytes())
                        } else {
                            // Liveness probe → a number.
                            (0x02, b"2")
                        }
                    }
                };
                let mut out = vec![marker];
                out.extend_from_slice(payload);
                let _ = stream.write_all(&out);
                let _ = stream.shutdown(Shutdown::Write);
            }
        });
        (port, seen, keep)
    }

    /// `session show` must check liveness FIRST: a dead/wedged daemon must
    /// not make the user wait out the whole identity-probe budget before
    /// they learn that the daemon is gone.
    #[test]
    #[serial]
    fn show_checks_liveness_first_and_skips_the_identity_probe() {
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let (port, seen, _keep) = spawn_fake_daemon(FakeDaemonMode::Wedged);

        let mut s = session();
        s.host = "localhost".into();
        s.port = port;
        s.daemon_user = Some("cacheduser".into());
        s.save_to_session_file();

        let mut cfg = test_config(None, false);
        cfg.remote_host = None;
        cfg.port = 0;
        cfg.port_explicit = false;
        // A large budget: if the identity probe ran, it would burn 30s.
        cfg.timeout = 30;
        let ctx = CommandContext::new(cfg, Some("local-target".into())).expect("ctx");

        let started = std::time::Instant::now();
        let result = show(&ctx, &s.id, OutputFormat::Json).expect("show should succeed");
        let elapsed = started.elapsed();

        // The identity and version probes must never have been sent.
        let requests = seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            !requests.iter().any(|r| r.contains("getShellEnvVar")),
            "the identity probe must be skipped when the daemon is not alive, saw: {requests:?}"
        );
        assert!(
            !requests.iter().any(|r| r.contains("RBDVersion")),
            "the version probe must be skipped when the daemon is not alive, saw: {requests:?}"
        );

        // The skip is visible through the existing probe fields.
        assert_eq!(result["probe"]["skipped"], true);
        assert_eq!(result["probe"]["skip_reason"], "daemon_not_alive");
        assert_eq!(result["session"]["daemon_responsive"], false);
        assert_eq!(result["session"]["data_source"], "cached");

        // The stale-daemon hint must survive the skip: the port is bound
        // while the daemon itself is wedged.
        assert!(
            result["warnings"]["stale_daemon"].as_str().is_some(),
            "a wedged daemon must still produce the stale-daemon hint, got {}",
            result["warnings"]["stale_daemon"]
        );

        assert!(
            elapsed.as_secs_f64() < 5.0,
            "a dead daemon must be detected in seconds, not after the 30s identity budget, \
             took {elapsed:?}"
        );
    }

    /// An unconfirmed identity probe must CLEAR the stored daemon_user:
    /// the connect-time guard compares the stored value, so a stale one
    /// would be compared as if it had been verified.
    #[test]
    #[serial]
    fn show_clears_stored_daemon_user_when_the_identity_probe_is_unconfirmed() {
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let (port, _seen, _keep) = spawn_fake_daemon(FakeDaemonMode::NoIdentity);

        let mut s = session();
        s.host = "localhost".into();
        s.port = port;
        s.daemon_user = Some("staleuser".into());
        s.save_to_session_file();

        let mut cfg = test_config(Some("expected-user"), false);
        cfg.remote_host = None;
        cfg.port = 0;
        cfg.port_explicit = false;
        cfg.timeout = 5;
        let ctx = CommandContext::new(cfg, Some("local-target".into())).expect("ctx");

        let result = show(&ctx, &s.id, OutputFormat::Json).expect("show should succeed");

        // The probe ran and confirmed nothing.
        assert_eq!(result["probe"]["skipped"], false);
        assert!(
            result["session"]["daemon_user"].is_null(),
            "the reported daemon user must be the (unconfirmed) live value, got {}",
            result["session"]["daemon_user"]
        );

        // The stored value must be gone — not left behind to look verified.
        let reloaded = SessionInfo::load(&s.id).expect("reload session");
        assert!(
            reloaded.daemon_user.is_none(),
            "an unconfirmed identity probe must clear the stored daemon_user, got {:?}",
            reloaded.daemon_user
        );

        // The cross-user warning must say the probe RAN (not that it was
        // skipped) and point at the real remedies.
        let warning = result["warnings"]["cross_user"]
            .as_str()
            .expect("cross-user warning for an unconfirmed identity");
        assert!(
            warning.contains("VB_TIMEOUT"),
            "the warning must advise raising VB_TIMEOUT: {warning}"
        );
    }

    /// Fake daemon that swallows the daemon-user identity probe and answers
    /// every other request immediately.
    ///
    /// The connection is keyed on the SKILL payload, not on its ordinal: the
    /// endpoint-reachability check opens a payload-less connection first, and
    /// the identity probe is the one carrying `getShellEnvVar`. The version
    /// and liveness probes are answered fast, so the wall-clock of
    /// `session show` is dominated by the identity probe's timeout budget —
    /// which is exactly what the test measures.
    fn spawn_daemon_swallows_identity_probe() -> (u16, std::sync::Arc<()>) {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let keep = std::sync::Arc::new(());
        let keep2 = std::sync::Arc::clone(&keep);
        std::thread::spawn(move || {
            let _keep = keep2;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                if request.contains("getShellEnvVar") {
                    // Identity probe: never answer. Hold the socket open so
                    // the client burns its own timeout budget.
                    std::thread::spawn(move || {
                        let mut sink = [0u8; 1024];
                        let _ = stream.read(&mut sink);
                        std::thread::sleep(Duration::from_secs(30));
                    });
                    continue;
                }
                if n == 0 {
                    // Reachability probe (no payload) — nothing to answer.
                    continue;
                }
                // Version probe → a version string; liveness probe → a number.
                let answer: &[u8] = if request.contains("RBDVersion") {
                    b"\"9.9.9\""
                } else {
                    b"2"
                };
                let mut payload = vec![0x02];
                payload.extend_from_slice(answer);
                let _ = stream.write_all(&payload);
                let _ = stream.shutdown(Shutdown::Write);
            }
        });
        (port, keep)
    }

    #[test]
    #[serial]
    fn show_identity_probe_uses_configured_timeout_not_a_hardcoded_cap() {
        // The identity probe inside `session show` must spend the resolved
        // config timeout (VB_TIMEOUT), not a hardcoded 3s/5s cap: a busy
        // daemon needs the headroom the user granted the rest of the
        // session.
        //
        // With `timeout = 1` the probe must give up after ~1s. The old
        // hardcoded 3s client budget (or the hardcoded 5s probe budget)
        // would push the call past 2.5s.
        let _cache = CacheDirGuard::new();
        let _state = StateDirGuard::new();

        let (port, _keep) = spawn_daemon_swallows_identity_probe();

        let mut s = session();
        s.host = "localhost".into();
        s.port = port;
        s.save_to_session_file();

        let mut cfg = test_config(None, false);
        cfg.remote_host = None;
        cfg.port = 0;
        cfg.port_explicit = false;
        cfg.timeout = 1;
        let ctx = CommandContext::new(cfg, Some("local-target".into())).expect("ctx");

        let started = std::time::Instant::now();
        let result = show(&ctx, &s.id, OutputFormat::Json).expect("show should succeed");
        let elapsed = started.elapsed();

        assert!(
            elapsed.as_secs_f64() < 2.5,
            "identity probe must honor the 1s configured timeout, took {elapsed:?}"
        );
        // The probe ran (not skipped) and its failure is surfaced.
        assert_eq!(result["probe"]["skipped"], false);
        assert!(
            result["warnings"]["daemon_user"]
                .as_str()
                .is_some_and(|w| w.contains("daemon user query failed")),
            "a failed identity probe must be surfaced, got {}",
            result["warnings"]["daemon_user"]
        );
    }
}

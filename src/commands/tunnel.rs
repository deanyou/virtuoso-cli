use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::context::CommandContext;
use crate::error::{Result, VirtuosoError};
use crate::models::{SessionInfo, TunnelState, TUNNEL_MODE_ATTACHED, TUNNEL_MODE_DEPLOYED};
use crate::output::OutputFormat;
use crate::transport::session_discovery::pick_live_session;
use crate::transport::tunnel::SSHClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn start(ctx: &CommandContext, timeout: Option<u64>, dry_run: bool) -> Result<Value> {
    let cfg = ctx.config();

    if dry_run {
        return Ok(json!({
            "action": "start",
            "resource": "tunnel",
            "target": {
                "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
                "port": cfg.port,
            },
            "dry_run": true,
        }));
    }

    let mut client = SSHClient::from_config(cfg, cfg.keep_remote_files)?;
    client.warm(timeout)?;

    // Auto-discover remote sessions and sync them to local cache.
    // This allows `vcli skill exec` to find the Virtuoso daemon port
    // without manual docker cp or session file copying.
    let transport = client.transport();
    let sessions_synced = SessionInfo::sync_from_remote(transport.as_ref()).unwrap_or(0);

    let vc = VirtuosoClient::from_context(ctx)?;
    let daemon_ok = matches!(vc.test_connection(Some(cfg.timeout)), Ok(true));

    Ok(json!({
        "status": "started",
        "port": client.port,
        "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
        "daemon_responsive": daemon_ok,
        "sessions_synced": sessions_synced,
    }))
}

/// Connect to a Virtuoso daemon that already exists on the remote host.
///
/// This is the non-destructive counterpart to [`start`]: instead of deploying
/// a fresh daemon + bridge.il via `tunnel start`, `tunnel attach` discovers
/// an existing daemon (written by `bridge.il` to
/// `~/.cache/virtuoso_bridge/sessions/*.json`), verifies its TCP listener,
/// and opens a single-port SSH tunnel to it.
///
/// The remote daemon is **not** touched — it belongs to Virtuoso and must
/// outlive this command. To drop the local side of the connection, run
/// `tunnel detach` (which kills the tunnel SSH process and clears state).
///
/// Returns `Err(NotFound)` if no live daemon can be discovered.
pub fn attach(ctx: &CommandContext, dry_run: bool) -> Result<Value> {
    let cfg = ctx.config();

    // Refuse if a tunnel of any mode is already up. The user can pick the
    // matching verb to clean up (`detach` for attached, `stop` for deployed).
    if let Some(existing) = TunnelState::load()? {
        let mode = existing.mode.as_deref().unwrap_or(TUNNEL_MODE_DEPLOYED);
        let verb = if mode == TUNNEL_MODE_ATTACHED {
            "detach"
        } else {
            "stop"
        };
        return Err(VirtuosoError::Execution(format!(
            "tunnel already exists on port {} (mode={}); run `vcli tunnel {verb}` first",
            existing.port, mode
        )));
    }

    // SSHClient here is used purely as a transport wrapper — we don't call
    // warm() (which would deploy a fresh daemon). The runner is configured
    // by from_config() but no remote command runs until we explicitly call one.
    let mut client = SSHClient::from_config(cfg, cfg.keep_remote_files)?;
    let transport = client.transport();

    let sessions = SessionInfo::list_remote(transport.as_ref())?;
    if sessions.is_empty() {
        return Err(VirtuosoError::NotFound(
            "no Virtuoso sessions found on remote; run `vcli tunnel start` to deploy a fresh daemon".into()
        ));
    }

    let host_hint = cfg.remote_host.as_deref();
    let live = pick_live_session(sessions, transport.as_ref(), host_hint)?.ok_or_else(|| {
        VirtuosoError::NotFound(
            "found session(s) on remote but no live daemons (port not listening); \
                 check that Virtuoso is running and the daemon process is alive"
                .into(),
        )
    })?;

    if dry_run {
        return Ok(json!({
            "action": "attach",
            "resource": "tunnel",
            "discovered": {
                "session_id": live.id,
                "remote_port": live.port,
                "remote_host": live.host,
                "created": live.created,
                "user": live.user,
            },
            "dry_run": true,
        }));
    }

    // Use the same port locally as the remote daemon listens on. Same-port
    // forwarding keeps scripts that bind to the canonical daemon port
    // working without reconfiguration. `open_tunnel` runs `ssh -L
    // 127.0.0.1:<port>:127.0.0.1:<port>`, so this forwards to the
    // discovered listener exactly.
    let local_port = live.port;
    client.open_tunnel(local_port)?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let state = TunnelState {
        version: crate::models::CURRENT_STATE_VERSION,
        port: local_port,
        pid: client.tunnel_pid().unwrap_or(0),
        remote_host: live.host.clone(),
        setup_path: None,
        profile: cfg.profile.clone(),
        backend: Some("openssh".to_string()),
        daemon_nonce: None,
        executable_path: None,
        start_identity: None,
        ipc_endpoint: None,
        token_path: None,
        local_forward: Some(format!("L*:{local_port}")),
        start_time_unix_ms: Some(now_ms),
        health: None,
        config_digest: None,
        mode: Some(TUNNEL_MODE_ATTACHED.into()),
        attached_remote_port: Some(live.port),
        attached_session_id: Some(live.id.clone()),
    };
    state
        .save()
        .map_err(|e| VirtuosoError::Ssh(format!("save tunnel state: {e}")))?;

    // Mirror the remote session metadata into the local cache so subsequent
    // `vcli session show` works without another SSH round-trip.
    let transport = client.transport();
    let sessions_synced = SessionInfo::sync_from_remote(transport.as_ref()).unwrap_or(0);

    Ok(json!({
        "status": "attached",
        "mode": TUNNEL_MODE_ATTACHED,
        "session_id": live.id,
        "remote_port": live.port,
        "local_port": local_port,
        "remote_host": live.host,
        "pid": client.tunnel_pid().unwrap_or(0),
        "sessions_synced": sessions_synced,
    }))
}

pub fn stop(ctx: &CommandContext, force: bool, dry_run: bool) -> Result<Value> {
    let cfg = ctx.config();

    let state = TunnelState::load()?;
    let state = match state {
        Some(s) => s,
        None => return Err(VirtuosoError::NotFound("no running tunnel found".into())),
    };

    let mode = state.mode.as_deref().unwrap_or(TUNNEL_MODE_DEPLOYED);

    if dry_run {
        return Ok(json!({
            "action": "stop",
            "resource": "tunnel",
            "target": {
                "port": state.port,
                "pid": state.pid,
                "remote_host": state.remote_host,
                "mode": mode,
            },
            // Attached daemons belong to Virtuoso — stop must not rm -rf them.
            "will_cleanup_remote": !cfg.keep_remote_files && mode == TUNNEL_MODE_DEPLOYED,
            "dry_run": true,
        }));
    }

    // Single, authoritative stop path. Cross-platform pid verification, the
    // native two-tier assessment, remote scratch cleanup and state clearing all
    // live in one place, so nothing here duplicates (or can drift from) them.
    //
    // Both cleanup gates are enforced inside stop_saved_tunnel:
    //   1. authorization — cleanup only after the verdict says the recorded
    //      tunnel is ours or proven gone (a live/unverifiable daemon is kept)
    //   2. ownership — only a `deployed` tunnel owns its remote setup dir; an
    //      `attached` daemon belongs to Virtuoso and is never rm -rf'd
    crate::transport::tunnel::stop_saved_tunnel(cfg, &state, force)?;

    Ok(json!({
        "status": "stopped",
        "mode": mode,
        "port": state.port,
        "pid": state.pid,
    }))
}

/// Disconnect a tunnel that was opened by [`attach`].
///
/// Unlike [`stop`], `detach` never touches the remote daemon: it kills the
/// local SSH tunnel process and clears state. The remote Virtuoso session
/// keeps running and its daemon keeps listening; users reconnect with
/// `vcli tunnel attach` (or the daemon is shut down independently by Virtuoso).
///
/// Returns `Err(NotFound)` when there is no tunnel, and
/// `Err(Execution)` when the recorded tunnel is in `deployed` mode (use
/// `tunnel stop` instead, since deployed tunnels own a setup dir that
/// needs cleanup).
pub fn detach(ctx: &CommandContext) -> Result<Value> {
    let state = TunnelState::load()?
        .ok_or_else(|| VirtuosoError::NotFound("no attached tunnel found".into()))?;

    // P0-A ownership (F05): never detach a tunnel that belongs to another
    // target's host.
    if let Some(tid) = ctx.target_id() {
        let target_host = ctx.config().remote_host.as_deref().unwrap_or("");
        if !target_host.is_empty() && state.remote_host != target_host {
            return Err(VirtuosoError::Config(format!(
                "tunnel belongs to host '{}' but target '{tid}' resolves to '{}'; \
                 refusing to touch another target's tunnel",
                state.remote_host, target_host
            )));
        }
    }

    let mode = state.mode.as_deref().unwrap_or(TUNNEL_MODE_DEPLOYED);
    if mode != TUNNEL_MODE_ATTACHED {
        return Err(VirtuosoError::Execution(format!(
            "this is a {mode} tunnel, not attached; use `vcli tunnel stop` instead"
        )));
    }

    kill_tunnel_pid(state.pid, false);

    TunnelState::clear()?;

    Ok(json!({
        "status": "detached",
        "mode": TUNNEL_MODE_ATTACHED,
        "port": state.port,
        "pid": state.pid,
        "session_id": state.attached_session_id,
        "remote_port": state.attached_remote_port,
        "remote_host": state.remote_host,
    }))
}

pub fn restart(ctx: &CommandContext, timeout: Option<u64>) -> Result<Value> {
    let stop_result = match stop(ctx, false, false) {
        Ok(v) => Some(v),
        Err(VirtuosoError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let start_result = start(ctx, timeout, false)?;

    Ok(json!({
        "stop": stop_result,
        "start": start_result,
    }))
}

pub fn diagnose(ctx: &CommandContext) -> Result<Value> {
    let cfg = ctx.config();
    let port = TunnelState::load()?.map(|s| s.port).unwrap_or(cfg.port);

    // TCP reachability
    let tcp_ok = std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        std::time::Duration::from_secs(2),
    )
    .is_ok();

    // Daemon responsiveness + latency
    let (daemon_ok, latency_ms, virtuoso_version) = if tcp_ok {
        let vc = crate::client::bridge::VirtuosoClient::new("127.0.0.1", port, cfg.timeout);
        let start = std::time::Instant::now();
        match vc.test_connection(Some(5)) {
            Ok(true) => {
                let lat = start.elapsed().as_millis();
                // Try to get Virtuoso version
                // getVersion() is a fixed read-only probe; idempotent, so it
                // may drain a stale queued ticket.
                let ver = vc
                    .execute_skill_idempotent_probe("getVersion()", None)
                    .ok()
                    .and_then(|r| {
                        if r.skill_ok() {
                            Some(r.output.trim_matches('"').to_string())
                        } else {
                            None
                        }
                    });
                (true, Some(lat as u64), ver)
            }
            _ => (false, None, None),
        }
    } else {
        (false, None, None)
    };

    // SKILL eval test
    let skill_ok = if daemon_ok {
        let vc = VirtuosoClient::new("127.0.0.1", port, cfg.timeout);
        // Idempotent health probe — same expression test_connection uses.
        vc.execute_skill_idempotent_probe("1+1", None)
            .map(|r| r.output.trim() == "2")
            .unwrap_or(false)
    } else {
        false
    };

    // Hostname verification — see `HostnameCheck` doc. Skip when no
    // remote host is configured (local mode). Gated on `tcp_ok` (not
    // `daemon_ok`) because on strict daemons test_connection's `1+1`
    // SKILL call is blocked, but getHostName() via execute_skill_unchecked
    // still works — the hostname check has its own error path for
    // genuinely-unreachable daemons.
    let hostname_check = if tcp_ok && cfg.is_remote() {
        let vc = VirtuosoClient::new("127.0.0.1", port, cfg.timeout);
        match HostnameCheck::run(&vc, cfg.remote_host.as_deref(), Some(5)) {
            Ok(Some(c)) => Some(c.to_json()),
            Ok(None) => None, // local mode (shouldn't reach here given gate)
            Err(e) => Some(json!({ "skipped": format!("daemon error: {e}") })),
        }
    } else {
        None
    };

    let summary = if skill_ok {
        if let Some(ref hc) = hostname_check {
            if hc.get("mismatch").and_then(|v| v.as_bool()) == Some(true) {
                "fully operational BUT hostname mismatch (jump host misconfig?)"
            } else {
                "fully operational"
            }
        } else {
            "fully operational"
        }
    } else if daemon_ok {
        "daemon responds but SKILL eval failed"
    } else if tcp_ok {
        "TCP reachable but daemon not responding"
    } else {
        "not reachable"
    };

    let mut result = json!({
        "port": port,
        "tcp_reachable": tcp_ok,
        "daemon_responsive": daemon_ok,
        "skill_eval_ok": skill_ok,
        "latency_ms": latency_ms,
        "virtuoso_version": virtuoso_version,
        "summary": summary,
    });
    if let Some(hc) = hostname_check {
        result["hostname_check"] = hc;
    }
    Ok(result)
}

pub fn status(ctx: &CommandContext, format: OutputFormat) -> Result<Value> {
    let cfg = ctx.config();

    let mut result = json!({
        "config": {
            "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
            "port": cfg.port,
            "timeout": cfg.timeout,
            "target": ctx.target_id(),
            "config_digest": ctx.config_digest(),
        }
    });

    // Multi-host role split. Each role is independently resolvable; when
    // all four collapse to remote_host the JSON shows the same value four
    // times (no-op for legacy single-host setups).
    let fb = cfg.remote_host.as_deref();
    result["config"]["roles"] = json!({
        "gui_host": cfg.roles.gui_host(fb),
        "deploy_host": cfg.roles.deploy_host(fb),
        "daemon_host": cfg.roles.daemon_host(fb),
        "spectre_host": cfg.roles.spectre_host(fb),
        "scratch_root": cfg.roles.scratch_root(),
    });

    let tunnel_info = if let Some(state) = TunnelState::load()? {
        let port_open = std::net::TcpStream::connect(format!("127.0.0.1:{}", state.port)).is_ok();
        let host_match = !cfg.is_remote() || Some(&state.remote_host) == cfg.remote_host.as_ref();

        // Backend diagnostics: report both the config-selected backend and the
        // backend recorded on disk, so an operator can spot a drift (e.g. they
        // ran a native daemon last, then re-launched with `VB_SSH_BACKEND=
        // openssh` and the legacy state file still says `native`).
        let (config_backend_value, tunnel_backend_value, drift_warning) =
            backend_diagnostics(cfg, Some(state.backend_or_openssh()));
        result["config"]["backend"] = config_backend_value;
        if let Some(warning) = drift_warning {
            result["config"]["backend_warning"] = json!(warning);
        }

        json!({
            "running": true,
            "port": state.port,
            "pid": state.pid,
            "remote_host": state.remote_host,
            "port_reachable": port_open,
            "host_match": host_match,
            "backend": tunnel_backend_value,
        })
    } else {
        // No live tunnel — report the config-selected backend alone; the
        // recorded backend is irrelevant until a tunnel is actually running.
        let (config_backend_value, _, _) = backend_diagnostics(cfg, None);
        result["config"]["backend"] = config_backend_value;
        json!({ "running": false })
    };
    result["tunnel"] = tunnel_info;

    let port = TunnelState::load()?.map(|s| s.port).unwrap_or(cfg.port);

    let mut daemon_info = if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
        let vc = VirtuosoClient::new("127.0.0.1", port, cfg.timeout);
        match vc.test_connection(Some(5)) {
            Ok(true) => json!({ "responsive": true }),
            Ok(false) => json!({ "responsive": false, "detail": "unexpected response" }),
            Err(e) => json!({ "responsive": false, "detail": e.to_string() }),
        }
    } else {
        json!({ "responsive": false, "detail": "port not reachable" })
    };

    // Hostname verification: ask the daemon what host it thinks it's on,
    // compare to VB_REMOTE_HOST. Most common EDA misconfig is pointing
    // VB_REMOTE_HOST at the jump host instead of the compute host.
    // Uses execute_skill_unchecked because tunnel status is a diagnostic
    // command — it must work without Admin capability.
    if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
        let vc = VirtuosoClient::new("127.0.0.1", port, cfg.timeout);
        match HostnameCheck::run(&vc, cfg.remote_host.as_deref(), Some(5)) {
            Ok(Some(check)) => {
                daemon_info["hostname_check"] = check.to_json();
                if check.mismatch {
                    daemon_info["warning"] = json!(check.warning_message());
                }
            }
            Ok(None) => {
                daemon_info["hostname_check"] = json!({ "skipped": "local mode" });
            }
            Err(e) => {
                daemon_info["hostname_check"] =
                    json!({ "skipped": format!("daemon did not respond: {e}") });
            }
        }
    }

    result["daemon"] = daemon_info;

    if format == OutputFormat::Table {
        let obj = result.as_object().unwrap();
        println!("=== Virtuoso CLI Status ===\n");
        if let Some(config) = obj.get("config") {
            println!("config:");
            for (k, v) in config.as_object().unwrap() {
                if k == "roles" {
                    // Render roles as nested keys; the same string repeated
                    // four times when roles collapse onto remote_host is
                    // the expected behavior of the legacy single-host setup.
                    if let Some(roles) = v.as_object() {
                        for (rk, rv) in roles {
                            let display = match rv {
                                Value::String(s) if s.is_empty() => "(unset)".to_string(),
                                Value::Null => "(unset)".to_string(),
                                other => other.to_string(),
                            };
                            println!("  {rk}: {display}");
                        }
                    }
                    continue;
                }
                println!("  {k}: {v}");
            }
            println!();
        }
        if let Some(tunnel) = obj.get("tunnel") {
            println!("tunnel:");
            for (k, v) in tunnel.as_object().unwrap() {
                let display = match v {
                    Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                println!("  {k}: {display}");
            }
            println!();
        }
        if let Some(daemon) = obj.get("daemon") {
            println!("daemon:");
            for (k, v) in daemon.as_object().unwrap() {
                let display = match v {
                    Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                println!("  {k}: {display}");
            }
            // If hostname check found a mismatch, surface a prominent warning.
            if let Some(check) = daemon.get("hostname_check") {
                if check.get("mismatch").and_then(|v| v.as_bool()) == Some(true) {
                    if let (Some(actual), Some(configured)) = (
                        check.get("actual").and_then(|v| v.as_str()),
                        check.get("configured").and_then(|v| v.as_str()),
                    ) {
                        println!();
                        println!("  ⚠ hostname mismatch:");
                        println!("    VB_REMOTE_HOST    = {configured}");
                        println!("    daemon reports    = {actual}");
                        println!("    Make sure VB_REMOTE_HOST points to the machine running");
                        println!("    Virtuoso, NOT the jump host. See `vcli tunnel status` JSON");
                        println!("    for full details.");
                    }
                }
            }
            println!();
        }
    }

    Ok(result)
}

/// Build the backend diagnostics block for `tunnel status` JSON.
///
/// Returns `(config_backend_value, tunnel_backend_value, drift_warning)`:
/// - `config_backend_value` reports what the running `Config` selected, plus
///   whether the `native-ssh` Cargo feature is compiled into this build, so an
///   operator asking for `native` on an OpenSSH-only binary gets an immediate,
///   explicit error instead of an `UnsupportedBackend` only when they actually
///   try to use the transport.
/// - `tunnel_backend_value` is the backend recorded on the live `TunnelState`
///   file (`openssh` for v1 / legacy, `native` for native daemons). `None` for
///   the running-branch call means no state file is loaded — caller should
///   pass `None` when there is no live tunnel.
/// - `drift_warning` is `Some(_)` iff the two backends disagree; callers
///   surface it as a `config.backend_warning` field so dashboards / scripts can
///   detect a stale `state.json` without diffing files by hand.
pub(crate) fn backend_diagnostics(
    cfg: &Config,
    state_backend: Option<&str>,
) -> (Value, Value, Option<String>) {
    let selected = cfg.ssh_backend.as_deref().unwrap_or("openssh");
    let config_backend_value = json!({
        "selected": selected,
        "supported_in_build": match selected {
            "native" => cfg!(feature = "native-ssh"),
            "openssh" => true,
            // Unknown values surface honestly rather than being silently
            // treated as openssh — the design forbids silent fallback.
            _ => false,
        },
    });

    let tunnel_backend_value = match state_backend {
        Some(b) => json!(b),
        None => Value::Null,
    };

    let drift_warning = match state_backend {
        Some(on_disk) if on_disk != selected => Some(format!(
            "backend drift: config selects '{selected}' but tunnel state records '{on_disk}'; \
             the live tunnel was started with a different backend"
        )),
        _ => None,
    };

    (config_backend_value, tunnel_backend_value, drift_warning)
}

/// Hostname verification result — compares the user-configured remote host
/// (`VB_REMOTE_HOST`) to the actual hostname the Virtuoso daemon reports
/// via `getHostName()`. A mismatch is the most common EDA misconfig:
/// pointing `VB_REMOTE_HOST` at a jump host instead of the compute host
/// where Virtuoso actually runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostnameCheck {
    /// The configured `VB_REMOTE_HOST` (or whatever profile variant).
    /// `None` means local mode — no check is performed.
    pub configured: Option<String>,
    /// The actual hostname the daemon reports via `getHostName()`.
    pub actual: String,
    /// `true` when `configured != actual` and both are non-empty.
    pub mismatch: bool,
}

impl HostnameCheck {
    /// Run the check by executing `getHostName()` on the daemon. Returns:
    /// - `Ok(None)` if `configured` is `None` (local mode — nothing to verify).
    /// - `Ok(Some(check))` if the check ran.
    /// - `Err(_)` if the daemon is unreachable or returned a non-string value.
    ///
    /// `timeout` is the SKILL call timeout; pass `None` for the daemon's default.
    pub fn run(
        vc: &VirtuosoClient,
        configured: Option<&str>,
        timeout: Option<u64>,
    ) -> Result<Option<Self>> {
        // Local mode — no configured remote host, nothing to verify.
        let configured = match configured {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return Ok(None),
        };

        // Use the idempotent probe path because tunnel status / diagnose are
        // diagnostic commands — they must work without Admin capability, and
        // getHostName() is read-only (the worst it can leak is the host name).
        let result = vc.execute_skill_idempotent_probe("getHostName()", timeout)?;
        if !result.skill_ok() {
            return Err(VirtuosoError::Execution(format!(
                "getHostName() failed: {}",
                result.errors.first().cloned().unwrap_or_default()
            )));
        }

        let actual = Self::parse_gethostname_output(&result.output)?;
        let mismatch = actual != configured;
        Ok(Some(Self {
            configured: Some(configured),
            actual,
            mismatch,
        }))
    }

    /// Parse the raw output of `getHostName()`. The function is pure and
    /// extracted from `run()` for testability — see the unit tests below.
    ///
    /// `getHostName()` returns a SKILL string like `"myhost\n"`. We strip:
    ///   - surrounding whitespace and trailing newlines (the RBIPC channel
    ///     sometimes appends a `\n`)
    ///   - a single pair of surrounding double quotes (the SKILL string
    ///     representation wraps a quoted value)
    ///
    /// Returns `Err` if the result is empty after stripping, since that
    /// indicates the daemon returned something nonsensical (the empty
    /// string is the only case where we can't produce a meaningful check).
    pub(crate) fn parse_gethostname_output(raw: &str) -> Result<String> {
        let trimmed = raw.trim();
        // strip one leading + one trailing double quote if present
        let stripped = trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(trimmed)
            .trim();
        if stripped.is_empty() {
            return Err(VirtuosoError::Execution(
                "getHostName() returned empty string".into(),
            ));
        }
        Ok(stripped.to_string())
    }

    /// Build a HostnameCheck directly. Used by tests and by any caller
    /// that already has both the configured and actual values.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_parts(configured: String, actual: String) -> Self {
        let mismatch = configured != actual;
        Self {
            configured: Some(configured),
            actual,
            mismatch,
        }
    }

    /// Human-readable warning text for the table output. Empty when there's
    /// no mismatch — the caller can `is_empty()` to decide whether to print.
    pub fn warning_message(&self) -> String {
        if !self.mismatch {
            return String::new();
        }
        let configured = self.configured.as_deref().unwrap_or("");
        format!(
            "VB_REMOTE_HOST='{configured}' but daemon is running on '{actual}'. \
             Most common cause: VB_REMOTE_HOST points to the jump host instead \
             of the compute host. See AGENTS.md 'three-host model' for the correct setup.",
            configured = configured,
            actual = self.actual,
        )
    }

    /// JSON shape for the `daemon.hostname_check` field of `tunnel status`.
    pub fn to_json(&self) -> Value {
        json!({
            "configured": self.configured,
            "actual": self.actual,
            "mismatch": self.mismatch,
        })
    }
}

#[cfg(unix)]
fn kill_tunnel_pid(pid: u32, force: bool) {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let is_ssh = std::fs::read_to_string(&cmdline_path)
        .map(|c| c.contains("ssh"))
        .unwrap_or(false);

    if is_ssh || force {
        let result = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if result != 0 && !force {
            tracing::warn!("could not kill process {pid}");
        }
    } else {
        tracing::warn!("PID {pid} is not an SSH process, skipping kill (use --force to override)");
    }
}

#[cfg(not(unix))]
fn kill_tunnel_pid(pid: u32, _force: bool) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(configured: &str, actual: &str) -> HostnameCheck {
        HostnameCheck {
            configured: Some(configured.into()),
            actual: actual.into(),
            mismatch: actual != configured,
        }
    }

    // ─── Warning text + JSON shape (existing 4 tests) ────────────────────

    #[test]
    fn warning_message_empty_when_no_mismatch() {
        let c = check("eda-1", "eda-1");
        assert!(c.warning_message().is_empty());
    }

    #[test]
    fn warning_message_includes_both_hostnames_on_mismatch() {
        let c = check("jump-bastion", "compute-1");
        let msg = c.warning_message();
        assert!(msg.contains("jump-bastion"), "got: {msg}");
        assert!(msg.contains("compute-1"), "got: {msg}");
        assert!(msg.contains("jump host"), "got: {msg}");
    }

    #[test]
    fn to_json_shape() {
        let c = check("eda-1", "eda-1");
        let j = c.to_json();
        assert_eq!(j["configured"], "eda-1");
        assert_eq!(j["actual"], "eda-1");
        assert_eq!(j["mismatch"], false);
    }

    #[test]
    fn to_json_shape_mismatch() {
        let c = check("jump", "compute");
        let j = c.to_json();
        assert_eq!(j["mismatch"], true);
    }

    // ─── parse_gethostname_output (new) ──────────────────────────────────

    #[test]
    fn parse_gethostname_output_strips_trailing_newline() {
        // Most common case — the RBIPC channel appends a trailing newline.
        assert_eq!(
            HostnameCheck::parse_gethostname_output("myhost\n").unwrap(),
            "myhost"
        );
    }

    #[test]
    fn parse_gethostname_output_strips_surrounding_quotes() {
        // SKILL string repr is `"myhost"` (note the quotes).
        assert_eq!(
            HostnameCheck::parse_gethostname_output("\"myhost\"").unwrap(),
            "myhost"
        );
    }

    #[test]
    fn parse_gethostname_output_strips_quotes_and_newline_together() {
        // The realistic raw output from the bridge.
        assert_eq!(
            HostnameCheck::parse_gethostname_output("\"myhost\"\n").unwrap(),
            "myhost"
        );
    }

    #[test]
    fn parse_gethostname_output_strips_internal_padding() {
        // Defensive: some channels pad with spaces.
        assert_eq!(
            HostnameCheck::parse_gethostname_output("  myhost  \n").unwrap(),
            "myhost"
        );
    }

    #[test]
    fn parse_gethostname_output_preserves_underscores_and_dashes() {
        // Common EDA hostname pattern.
        assert_eq!(
            HostnameCheck::parse_gethostname_output("compute-eda_42\n").unwrap(),
            "compute-eda_42"
        );
    }

    #[test]
    fn parse_gethostname_output_handles_fully_qualified_names() {
        // FQDN: dots must be preserved.
        assert_eq!(
            HostnameCheck::parse_gethostname_output("eda-42.corp.example.com\n").unwrap(),
            "eda-42.corp.example.com"
        );
    }

    #[test]
    fn parse_gethostname_output_errors_on_empty() {
        assert!(HostnameCheck::parse_gethostname_output("").is_err());
    }

    #[test]
    fn parse_gethostname_output_errors_on_whitespace_only() {
        assert!(HostnameCheck::parse_gethostname_output("   \n").is_err());
    }

    #[test]
    fn parse_gethostname_output_errors_on_just_quotes() {
        // The pair of quotes is stripped, leaving an empty string.
        assert!(HostnameCheck::parse_gethostname_output("\"\"").is_err());
    }

    // ─── from_parts (new) ────────────────────────────────────────────────

    #[test]
    fn from_parts_constructs_matching_check() {
        let c = HostnameCheck::from_parts("eda-1".into(), "eda-1".into());
        assert!(!c.mismatch);
        assert_eq!(c.configured.as_deref(), Some("eda-1"));
        assert_eq!(c.actual, "eda-1");
    }

    #[test]
    fn from_parts_constructs_mismatching_check() {
        let c = HostnameCheck::from_parts("jump".into(), "compute".into());
        assert!(c.mismatch);
    }

    // ─── Mismatch edge cases (new) ───────────────────────────────────────

    #[test]
    fn mismatch_when_actual_is_empty_string() {
        // If getHostName() somehow returned an empty actual, the check
        // should still distinguish mismatch (the configured host is not "").
        let c = check("eda-1", "");
        assert!(c.mismatch);
    }

    #[test]
    fn mismatch_case_sensitive() {
        // Hostnames are case-sensitive on Linux. Make sure we don't
        // accidentally do a case-insensitive comparison.
        let c = check("EDA-1", "eda-1");
        assert!(c.mismatch, "hostname comparison must be case-sensitive");
    }

    #[test]
    fn match_for_identical_fqdn() {
        let c = check(
            "compute-eda-42.corp.example.com",
            "compute-eda-42.corp.example.com",
        );
        assert!(!c.mismatch);
    }

    // The run() method needs a live VirtuosoClient; the parsing is
    // covered by parse_gethostname_output tests above. The
    // execute_skill_unchecked path is exercised by the bridge's own
    // tests in client/bridge.rs.
}

#[cfg(test)]
mod backend_diagnostics_tests {
    use super::*;
    use crate::config::Config;

    fn cfg_with(backend: Option<&str>) -> Config {
        // Config has no Default impl; build a minimal fixture inline so the
        // tests stay hermetic (no env, no filesystem). Only the fields the
        // backend-diagnostics logic actually reads matter here.
        Config {
            profile: None,
            remote_host: None,
            remote_user: None,
            port: 0,
            jump_host: None,
            jump_user: None,
            ssh_port: None,
            ssh_key: None,
            ssh_config: None,
            ssh_backend: backend.map(String::from),
            disable_control_master: false,
            timeout: 30,
            read_timeout: 120,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: 8,
            ssh_max_sessions: 10,
            ssh_max_bulk_sessions: 2,
            ssh_reconnect_max_attempts: 8,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 30,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 5,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: crate::config::RemoteRoles::default(),
            transport_daemon_socket: None,
            transport_daemon_token: None,
        }
    }

    #[test]
    fn config_default_reports_openssh_supported() {
        let (config_value, tunnel_value, warning) = backend_diagnostics(&cfg_with(None), None);
        assert_eq!(config_value["selected"], "openssh");
        assert_eq!(config_value["supported_in_build"], true);
        assert_eq!(tunnel_value, Value::Null);
        assert!(warning.is_none());
    }

    #[test]
    fn config_native_reports_native_supported_when_feature_is_on() {
        let (config_value, _, warning) = backend_diagnostics(&cfg_with(Some("native")), None);
        assert_eq!(config_value["selected"], "native");
        assert_eq!(
            config_value["supported_in_build"],
            cfg!(feature = "native-ssh"),
            "supported_in_build must mirror the compile-time feature"
        );
        assert!(warning.is_none());
    }

    #[test]
    fn config_unknown_backend_is_honest_not_silently_falls_back() {
        // The design forbids silent fallback; an unrecognised value must
        // surface as unsupported rather than being silently treated as
        // openssh. Drift detection against the on-disk backend still runs.
        let (config_value, _, warning) =
            backend_diagnostics(&cfg_with(Some("banana")), Some("openssh"));
        assert_eq!(config_value["selected"], "banana");
        assert_eq!(config_value["supported_in_build"], false);
        assert!(
            warning.is_some(),
            "drift warning must fire when on-disk backend disagrees with config"
        );
    }

    #[test]
    fn state_openssh_with_config_openssh_has_no_drift_warning() {
        let (_, tunnel_value, warning) = backend_diagnostics(&cfg_with(None), Some("openssh"));
        assert_eq!(tunnel_value, "openssh");
        assert!(warning.is_none());
    }

    #[test]
    fn state_native_with_config_openssh_fires_drift_warning() {
        // Live tunnel was started by a native daemon, but the operator
        // is now running a CLI build that defaults to openssh. The
        // status JSON must surface the mismatch so they don't act on
        // a stale backend assumption.
        let (_, tunnel_value, warning) = backend_diagnostics(&cfg_with(None), Some("native"));
        assert_eq!(tunnel_value, "native");
        let msg = warning.expect("drift warning must fire on config/state mismatch");
        assert!(msg.contains("backend drift"), "got: {msg}");
        assert!(msg.contains("config selects 'openssh'"), "got: {msg}");
        assert!(msg.contains("tunnel state records 'native'"), "got: {msg}");
    }

    #[test]
    fn state_native_with_config_native_has_no_drift_warning() {
        let (_, tunnel_value, warning) =
            backend_diagnostics(&cfg_with(Some("native")), Some("native"));
        assert_eq!(tunnel_value, "native");
        assert!(warning.is_none());
    }
}

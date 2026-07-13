use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::{SessionInfo, TunnelState};
use crate::output::OutputFormat;
use crate::transport::tunnel::SSHClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn start(timeout: Option<u64>, dry_run: bool) -> Result<Value> {
    let cfg = Config::from_env()?;

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

    let mut client = SSHClient::from_env(cfg.keep_remote_files)?;
    client.warm(timeout)?;

    // Auto-discover remote sessions and sync them to local cache.
    // This allows `vcli skill exec` to find the Virtuoso daemon port
    // without manual docker cp or session file copying.
    let sessions_synced = SessionInfo::sync_from_remote(&client.runner).unwrap_or(0);

    let vc = crate::client::bridge::VirtuosoClient::from_env()?;
    let daemon_ok = matches!(vc.test_connection(Some(cfg.timeout)), Ok(true));

    Ok(json!({
        "status": "started",
        "port": client.port,
        "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
        "daemon_responsive": daemon_ok,
        "sessions_synced": sessions_synced,
    }))
}

pub fn stop(force: bool, dry_run: bool) -> Result<Value> {
    let cfg = Config::from_env()?;

    let state = TunnelState::load()?;
    let state = match state {
        Some(s) => s,
        None => return Err(VirtuosoError::NotFound("no running tunnel found".into())),
    };

    if dry_run {
        return Ok(json!({
            "action": "stop",
            "resource": "tunnel",
            "target": {
                "port": state.port,
                "pid": state.pid,
                "remote_host": state.remote_host,
            },
            "will_cleanup_remote": !cfg.keep_remote_files,
            "dry_run": true,
        }));
    }

    // Clean up remote files BEFORE killing tunnel.
    // The cleanup path is profile-scoped (see transport::tunnel::setup_dir_for_profile)
    // so stopping profile A's tunnel doesn't wipe profile B's setup.
    if !cfg.keep_remote_files {
        match SSHClient::from_env(cfg.keep_remote_files) {
            Ok(client) => {
                let setup_dir =
                    crate::transport::tunnel::setup_dir_for_profile(cfg.profile.as_deref());
                if let Err(e) = client.run_command(&format!("rm -rf {setup_dir}")) {
                    tracing::warn!("remote cleanup failed: {e}");
                }
            }
            Err(e) => tracing::warn!("could not connect for cleanup: {e}"),
        }
    }

    #[cfg(unix)]
    {
        let cmdline_path = format!("/proc/{}/cmdline", state.pid);
        let is_ssh = std::fs::read_to_string(&cmdline_path)
            .map(|c| c.contains("ssh"))
            .unwrap_or(false);

        if is_ssh || force {
            let result = unsafe { libc::kill(state.pid as i32, libc::SIGTERM) };
            if result != 0 && !force {
                tracing::warn!("could not kill process {}", state.pid);
            }
        } else {
            tracing::warn!(
                "PID {} is not an SSH process, skipping kill (use --force to override)",
                state.pid
            );
        }
    }

    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &state.pid.to_string(), "/F"])
            .output();
    }

    TunnelState::clear()?;

    Ok(json!({
        "status": "stopped",
        "port": state.port,
        "pid": state.pid,
    }))
}

pub fn restart(timeout: Option<u64>) -> Result<Value> {
    let stop_result = match stop(false, false) {
        Ok(v) => Some(v),
        Err(VirtuosoError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let start_result = start(timeout, false)?;

    Ok(json!({
        "stop": stop_result,
        "start": start_result,
    }))
}

pub fn diagnose() -> Result<Value> {
    let cfg = Config::from_env()?;
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
                let ver = vc.execute_skill("getVersion()", None).ok().and_then(|r| {
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
        vc.execute_skill("1+1", None)
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

pub fn status(format: OutputFormat) -> Result<Value> {
    let cfg = Config::from_env()?;

    let mut result = json!({
        "config": {
            "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
            "port": cfg.port,
            "timeout": cfg.timeout,
        }
    });

    let tunnel_info = if let Some(state) = TunnelState::load()? {
        let port_open = std::net::TcpStream::connect(format!("127.0.0.1:{}", state.port)).is_ok();
        let host_match = !cfg.is_remote() || Some(&state.remote_host) == cfg.remote_host.as_ref();

        json!({
            "running": true,
            "port": state.port,
            "pid": state.pid,
            "remote_host": state.remote_host,
            "port_reachable": port_open,
            "host_match": host_match,
        })
    } else {
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

        // Use execute_skill_unchecked because tunnel status / diagnose are
        // diagnostic commands — they must work without Admin capability.
        // getHostName() is read-only; the worst it can leak is the host name.
        let result = vc.execute_skill_unchecked("getHostName()", timeout)?;
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

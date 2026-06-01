use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::SessionInfo;
use crate::output::OutputFormat;
use crate::transport::tunnel::SSHClient;
use serde_json::{json, Value};

pub fn list(format: OutputFormat) -> Result<Value> {
    // In remote mode, sync session files from remote host first.
    // Best effort: failures are silent so local cache still works.
    if let Ok(cfg) = Config::from_env() {
        if cfg.is_remote() {
            if let Ok(client) = SSHClient::from_env(cfg.keep_remote_files) {
                let _ = SessionInfo::sync_from_remote(&client.runner);
            }
        }
    }

    let mut sessions = SessionInfo::list()
        .map_err(|e| VirtuosoError::Execution(format!("failed to read sessions: {e}")))?;

    let sessions_dir = SessionInfo::sessions_dir();
    sessions.retain(|s| {
        if s.is_alive() {
            true
        } else {
            let _ = std::fs::remove_file(sessions_dir.join(format!("{}.json", s.id)));
            false
        }
    });

    if format == OutputFormat::Json {
        return Ok(json!({
            "status": "success",
            "count": sessions.len(),
            "sessions": sessions.iter().map(|s| json!({
                "id": s.id,
                "port": s.port,
                "pid": s.pid,
                "host": s.host,
                "user": s.user,
                "created": s.created,
            })).collect::<Vec<_>>(),
        }));
    }

    if sessions.is_empty() {
        println!("No active Virtuoso sessions found.");
        println!("Start Virtuoso and run RBStart() in CIW to register a session.");
        return Ok(json!({"status": "success", "count": 0}));
    }

    println!(
        "{:<20} {:>6}  {:>7}  {:<12}  CREATED",
        "SESSION ID", "PORT", "PID", "HOST"
    );
    println!("{}", "-".repeat(72));
    for s in &sessions {
        println!(
            "{:<20} {:>6}  {:>7}  {:<12}  {}",
            s.id, s.port, s.pid, s.host, s.created
        );
    }

    Ok(json!({"status": "success", "count": sessions.len()}))
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

pub fn show(id: &str, _format: OutputFormat) -> Result<Value> {
    let s = SessionInfo::load(id)
        .map_err(|e| VirtuosoError::NotFound(format!("session '{id}' not found: {e}")))?;

    // Best-effort liveness + identity probes.
    //   - `is_alive()` is just a TCP-connect probe; cheap and tells us
    //     whether the daemon port is bound.
    //   - `daemon_alive()` is a SKILL-level probe (ipcIsProcessRunning);
    //     catches "port bound but daemon is wedged" cases.
    //   - `get_daemon_user()` queries the daemon's Unix $USER so we can
    //     warn about SSH-tunnel-to-wrong-user misconfigurations.
    let port_open = s.is_alive();
    let (daemon_user, daemon_user_warning, daemon_responsive) = if port_open {
        let client = VirtuosoClient::new("127.0.0.1", s.port, 3);
        let user_result = client.get_daemon_user();
        let alive = client.daemon_alive();
        match user_result {
            Ok(user_opt) => (user_opt, None, alive),
            Err(e) => (None, Some(format!("daemon user query failed: {e}")), alive),
        }
    } else {
        (None, None, false)
    };

    // Cross-user check: if user has configured VB_REMOTE_USER_<profile>
    // (or plain VB_REMOTE_USER) and the daemon reports a different Unix
    // user, refuse to call this a healthy session.
    let cross_user_warning = check_cross_user(&s, daemon_user.as_deref());

    // Stale-daemon recovery hint: when the port is open but the daemon is
    // not responding to SKILL, the user is looking at a port held by
    // another instance. Tell them how to clear it.
    let stale_daemon_hint = if port_open && !daemon_responsive {
        Some(
            "CIW daemon port is bound but the daemon is not responding to SKILL.\n\
             In the Virtuoso CIW, run:\n\
             \x20 RBStop()\n\
             \x20 (load \"<path-to>/ramic_bridge.il\")\n\
             If that does not clear it, use RBStopAll() before loading again."
                .to_string(),
        )
    } else {
        None
    };

    Ok(json!({
        "status": if cross_user_warning.is_some() { "warning" } else { "success" },
        "session": {
            "id": s.id,
            "port": s.port,
            "pid": s.pid,
            "host": s.host,
            "user": s.user,
            "created": s.created,
            "alive": port_open,
            "daemon_responsive": daemon_responsive,
            "daemon_user": daemon_user,
        },
        "warnings": {
            "daemon_user": daemon_user_warning,
            "cross_user": cross_user_warning,
            "stale_daemon": stale_daemon_hint,
        }
    }))
}

/// Compare the daemon's Unix user with the configured `VB_REMOTE_USER[<profile>]`.
/// Returns `Some(warning)` if a mismatch is detected, `None` otherwise.
/// Set `VB_ALLOW_CROSS_USER_DAEMON=1` to suppress the warning.
fn check_cross_user(
    session: &crate::models::SessionInfo,
    daemon_user: Option<&str>,
) -> Option<String> {
    let profile = std::env::var("VB_PROFILE").ok();
    let expected = std::env::var(format!(
        "VB_REMOTE_USER{}",
        profile
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|p| format!("_{p}"))
            .unwrap_or_default()
    ))
    .ok()
    .or_else(|| std::env::var("VB_REMOTE_USER").ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())?;

    let daemon_user = daemon_user?;
    if daemon_user == expected {
        return None;
    }
    if std::env::var("VB_ALLOW_CROSS_USER_DAEMON")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
    {
        return None;
    }
    Some(format!(
        "daemon Unix user {daemon_user:?} does not match configured VB_REMOTE_USER {expected:?} \
         for session {sid}. Set VB_ALLOW_CROSS_USER_DAEMON=1 to override intentionally.",
        sid = session.id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionInfo;
    use std::sync::Mutex;

    // `std::env::set_var` is process-wide and not thread-safe; cargo test runs
    // tests in parallel by default. A global mutex serializes the env-mutating
    // tests in this module so they don't race with each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn session() -> SessionInfo {
        SessionInfo {
            id: "meowu-meow-40567".into(),
            port: 40567,
            pid: 0,
            host: "meowu".into(),
            user: "meow".into(),
            created: "Jun  1 08:14:16 2026".into(),
            daemon_user: None,
        }
    }

    /// Helper: clear all relevant env vars before each test that mutates them.
    fn clear_remote_user_env() {
        std::env::remove_var("VB_REMOTE_USER");
        std::env::remove_var("VB_REMOTE_USER_default");
        std::env::remove_var("VB_REMOTE_USER_testprofile");
        std::env::remove_var("VB_PROFILE");
        std::env::remove_var("VB_ALLOW_CROSS_USER_DAEMON");
    }

    #[test]
    fn cross_user_match_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_REMOTE_USER", "meow");
        let r = check_cross_user(&session(), Some("meow"));
        assert!(r.is_none());
        clear_remote_user_env();
    }

    #[test]
    fn cross_user_mismatch_returns_warning() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_REMOTE_USER", "alice");
        let r = check_cross_user(&session(), Some("bob"));
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
            w.contains("VB_ALLOW_CROSS_USER_DAEMON=1"),
            "warning should mention override: {w}"
        );
        assert!(
            w.contains("meowu-meow-40567"),
            "warning should name session: {w}"
        );
        clear_remote_user_env();
    }

    #[test]
    fn cross_user_mismatch_suppressed_by_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_REMOTE_USER", "alice");
        std::env::set_var("VB_ALLOW_CROSS_USER_DAEMON", "1");
        let r = check_cross_user(&session(), Some("bob"));
        assert!(r.is_none(), "VB_ALLOW_CROSS_USER_DAEMON=1 should suppress");
        clear_remote_user_env();
    }

    #[test]
    fn cross_user_mismatch_override_truthy_values() {
        for v in ["true", "yes", "on", "TRUE", "Yes", "  on  "] {
            let _g = ENV_LOCK.lock().unwrap();
            clear_remote_user_env();
            std::env::set_var("VB_REMOTE_USER", "alice");
            std::env::set_var("VB_ALLOW_CROSS_USER_DAEMON", v);
            let r = check_cross_user(&session(), Some("bob"));
            assert!(r.is_none(), "override {v:?} should suppress warning");
        }
        clear_remote_user_env();
    }

    #[test]
    fn cross_user_no_env_var_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        // No VB_REMOTE_USER set
        let r = check_cross_user(&session(), Some("anyone"));
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_no_daemon_user_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_REMOTE_USER", "meow");
        // daemon_user is None — we don't know, so don't warn
        let r = check_cross_user(&session(), None);
        assert!(r.is_none());
    }

    #[test]
    fn cross_user_profile_scoped_env_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_PROFILE", "testprofile");
        std::env::set_var("VB_REMOTE_USER_testprofile", "alice");
        // Profile-scoped env should trigger check
        let r = check_cross_user(&session(), Some("bob"));
        assert!(r.is_some(), "profile-scoped env var should trigger check");

        // When profile-scoped is set but matches, no warning
        let r = check_cross_user(&session(), Some("alice"));
        assert!(r.is_none());
        clear_remote_user_env();
    }

    #[test]
    fn cross_user_empty_env_var_is_treated_as_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_remote_user_env();
        std::env::set_var("VB_REMOTE_USER", "   ");
        let r = check_cross_user(&session(), Some("bob"));
        assert!(
            r.is_none(),
            "whitespace-only env var should be treated as unset"
        );
        clear_remote_user_env();
    }
}

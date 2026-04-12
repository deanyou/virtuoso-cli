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

pub fn show(id: &str, _format: OutputFormat) -> Result<Value> {
    let s = SessionInfo::load(id)
        .map_err(|e| VirtuosoError::NotFound(format!("session '{id}' not found: {e}")))?;

    Ok(json!({
        "status": "success",
        "session": {
            "id": s.id,
            "port": s.port,
            "pid": s.pid,
            "host": s.host,
            "user": s.user,
            "created": s.created,
            "alive": s.is_alive(),
        }
    }))
}

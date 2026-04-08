use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::TunnelState;
use crate::output::OutputFormat;
use crate::transport::tunnel::SSHClient;
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

    let vc = crate::client::bridge::VirtuosoClient::from_env()?;
    let daemon_ok = match vc.test_connection(Some(cfg.timeout)) {
        Ok(true) => true,
        _ => false,
    };

    Ok(json!({
        "status": "started",
        "port": client.port,
        "remote_host": cfg.remote_host.as_deref().unwrap_or("local"),
        "daemon_responsive": daemon_ok,
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

    // Clean up remote files BEFORE killing tunnel
    if !cfg.keep_remote_files {
        match SSHClient::from_env(cfg.keep_remote_files) {
            Ok(client) => {
                if let Err(e) = client.run_command("rm -rf /tmp/virtuoso_bridge") {
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

    let daemon_info = if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
        let vc = crate::client::bridge::VirtuosoClient::local("127.0.0.1", port, cfg.timeout);
        match vc.test_connection(Some(5)) {
            Ok(true) => json!({ "responsive": true }),
            Ok(false) => json!({ "responsive": false, "detail": "unexpected response" }),
            Err(e) => json!({ "responsive": false, "detail": e.to_string() }),
        }
    } else {
        json!({ "responsive": false, "detail": "port not reachable" })
    };
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
            println!();
        }
    }

    Ok(result)
}

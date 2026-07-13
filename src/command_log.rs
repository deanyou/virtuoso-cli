use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::runtime_paths;

/// Path to the command log file. Honours `VB_LOG_DIR` / `VB_HOME/logs` /
/// `XDG_STATE_HOME/logs` for tests and multi-tenant setups; falls back to
/// `~/.cache/virtuoso_bridge/logs/commands.log` (or platform equivalent).
pub fn log_path() -> PathBuf {
    let dir = runtime_paths::log_root();
    let _ = fs::create_dir_all(&dir);
    runtime_paths::command_log_file()
}

pub fn log_command(kind: &str, command: &str, duration_ms: Option<u128>) {
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
    let dur = duration_ms.map(|d| format!(" ({d}ms)")).unwrap_or_default();
    let line = format!("[{ts}] [{kind}]{dur} {command}\n");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

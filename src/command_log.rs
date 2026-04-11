use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub fn log_path() -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("virtuoso_bridge")
        .join("logs");
    let _ = fs::create_dir_all(&dir);
    dir.join("commands.log")
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

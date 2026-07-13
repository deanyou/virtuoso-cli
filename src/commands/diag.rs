//! Diagnostics: read-only diagnostic tools for stuck Virtuoso state.
//!
//! Currently provides:
//! - `cdslck <LIB>` — enumerate every `.cdslck` lock file under a library
//!   and report owner / host / pid / age. Read-only on purpose: deleting a
//!   live lock corrupts the cellview. Inspired by
//!   <https://github.com/Arcadia-1/virtuoso-bridge-lite/blob/main/examples/01_virtuoso/diagnostics/sniff_cdslck.py>
//!   (MIT, 2026-05).

use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::transport::ssh::SSHRunner;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// One .cdslck entry, as returned by `cdslck()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdsLockInfo {
    /// Absolute path of the .cdslck file on the remote host.
    pub path: String,
    /// Path relative to the library root, for human display.
    pub relative: String,
    /// Cellview the lock belongs to (relative path minus the trailing .cdslck).
    pub cellview: String,
    /// Cadence-format `owner@host:pid:start_time` payload.
    pub owner_record: String,
    /// Parsed owner (user portion of owner@host:...).
    pub owner: Option<String>,
    /// Parsed host portion of owner@host:....
    pub host: Option<String>,
    /// Parsed PID (best-effort).
    pub pid: Option<u32>,
    /// mtime of the lock file in seconds since UNIX_EPOCH (remote).
    pub mtime: Option<i64>,
    /// Age in seconds (relative to the local clock used by SSH stat).
    pub age_seconds: Option<f64>,
}

/// List every `.cdslck` lock under the OA library named `lib`.
///
/// If `view_filter` is set (e.g. "maestro", "layout"), only locks under
/// `<lib>/<cell>/<view>/.cdslck` are returned.
///
/// Implementation notes:
/// - We resolve `readPath` via the named `cell.read_path` RPC method (not
///   raw SKILL exec) so non-admin users can run this command.
/// - We enumerate locks with SSH `find`, not SKILL `system()`, because
///   the SKILL channel is what we may be trying to debug.
/// - We never delete locks; if a lock needs breaking, the user should
///   `ps -p <pid> @ <host>` first and then `rm` manually.
pub fn cdslck(lib: &str, view_filter: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    // Step 1: resolve library readPath via the cell.read_path RPC.
    let req = crate::rpc::dispatcher::RpcRequest {
        method: "cell.read_path".into(),
        params: json!({ "lib": lib }),
        api_key: std::env::var("VCLI_API_KEY").ok().filter(|k| !k.is_empty()),
    };
    let resp = crate::rpc::dispatcher::RpcDispatcher::dispatch(&client, req)?;
    let lib_path = resp
        .get("read_path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            VirtuosoError::NotFound(format!(
                "library {lib:?} is not registered in the remote cds.lib"
            ))
        })?;
    if lib_path.is_empty() {
        return Err(VirtuosoError::NotFound(format!(
            "library {lib:?} returned empty readPath"
        )));
    }

    // Step 2: enumerate lock files via SSH `find`.
    let config = Config::from_env()?;
    if config.remote_host.as_deref().unwrap_or("").is_empty() {
        return Err(VirtuosoError::Config(
            "VB_REMOTE_HOST is not set; cdslck needs SSH to enumerate locks".into(),
        ));
    }
    let runner = SSHRunner::from_config(&config);
    let find_cmd = format!(
        "find {} -name .cdslck -print 2>/dev/null",
        shell_quote(&lib_path)
    );
    let out = runner.run_command(&find_cmd, Some(60))?;
    let lock_paths: Vec<String> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    // Step 3: per-lock cat + stat, batched in one SSH round-trip.
    let locks = batch_read_locks(&runner, &lock_paths)?;
    let locks = match view_filter {
        Some(v) => locks
            .into_iter()
            .filter(|l| {
                l.path.contains(&format!("/{v}/.cdslck"))
                    || l.path.ends_with(&format!("/{v}/.cdslck"))
            })
            .collect(),
        None => locks,
    };

    let lib_root = std::path::Path::new(&lib_path);
    let json_rows: Vec<Value> = locks
        .iter()
        .map(|l| {
            let relative = l
                .path
                .strip_prefix(lib_root.to_str().unwrap_or(""))
                .unwrap_or(&l.path)
                .to_string();
            let cellview = relative.trim_end_matches("/.cdslck").to_string();
            json!({
                "path": l.path,
                "relative": format!("/{}", relative.trim_start_matches('/')),
                "cellview": cellview,
                "owner_record": l.owner_record,
                "owner": l.owner,
                "host": l.host,
                "pid": l.pid,
                "mtime": l.mtime,
                "age_seconds": l.age_seconds,
                "age_human": l.age_seconds.map(format_age),
            })
        })
        .collect();
    Ok(json!({
        "library": lib,
        "read_path": lib_path,
        "view_filter": view_filter,
        "count": json_rows.len(),
        "locks": json_rows,
    }))
}

fn batch_read_locks(runner: &SSHRunner, paths: &[String]) -> Result<Vec<CdsLockInfo>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    // Build a single command that for each path echoes a delimiter, then the
    // cat contents, then the stat mtime. NULL-delimited output keeps us safe
    // from newlines in owner records.
    let mut script = String::from("set -e\n");
    for p in paths {
        let qp = shell_quote(p);
        script.push_str(&format!(
            "printf '---LOCK---\\n'; cat {qp} 2>/dev/null || true; \
             printf '\\n---STAT---\\n'; stat -c '%Y' {qp} 2>/dev/null || true; \
             printf '\\n---END---\\n';"
        ));
    }
    let out = runner.run_command(&script, Some(60))?;
    Ok(parse_batch_output(&out.stdout, paths))
}

fn parse_batch_output(stdout: &str, paths: &[String]) -> Vec<CdsLockInfo> {
    // Output is a sequence of ---LOCK---\n<contents>\n---STAT---\n<mtime>\n---END---\n
    // We split on ---LOCK--- and then ---STAT--- / ---END--- to extract each record.
    let mut out = Vec::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    for chunk in stdout.split("---LOCK---").skip(1) {
        // chunk looks like: "\n<contents>\n---STAT---\n<mtime>\n---END---\n"
        let stat_split: Vec<&str> = chunk.splitn(2, "---STAT---").collect();
        if stat_split.len() != 2 {
            continue;
        }
        let owner_record = stat_split[0].trim().to_string();
        let rest = stat_split[1];
        let end_split: Vec<&str> = rest.splitn(2, "---END---").collect();
        let mtime_str = end_split.first().map(|s| s.trim()).unwrap_or("");
        let mtime: Option<i64> = mtime_str.parse().ok();
        let age = mtime.map(|m| (now - m as f64).max(0.0));
        out.push((owner_record, mtime, age));
    }
    // zip with paths
    out.into_iter()
        .zip(paths.iter())
        .map(|((owner_record, mtime, age), path)| {
            let (owner, host, pid) = parse_owner_record(&owner_record);
            let path_str = path.clone();
            let path_p = Path::new(&path_str);
            let cellview = path_p
                .strip_prefix(path_p.ancestors().last().unwrap_or(Path::new("/")))
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or(&path_str)
                .trim_end_matches("/.cdslck")
                .to_string();
            CdsLockInfo {
                path: path_str.clone(),
                relative: path_str.clone(),
                cellview,
                owner_record,
                owner,
                host,
                pid,
                mtime,
                age_seconds: age,
            }
        })
        .collect()
}

fn parse_owner_record(rec: &str) -> (Option<String>, Option<String>, Option<u32>) {
    // Cadence writes: owner@host:pid:start_time  (e.g. "meow@eda:12345:1717820000")
    // start_time is itself ":"-separated so we split from the right.
    if rec.is_empty() {
        return (None, None, None);
    }
    // owner@host:pid:start_time  →  at most 3 colons.
    let mut iter = rec.rsplit(':');
    let start_time = iter.next(); // ignore
    let pid_str = iter.next().unwrap_or("");
    let rest = iter
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(":");
    let pid = pid_str.parse::<u32>().ok();
    let (owner, host) = match rest.split_once('@') {
        Some((o, h)) => (Some(o.to_string()), Some(h.to_string())),
        None => (None, Some(rest.to_string())),
    };
    let _ = start_time; // start_time is informational; not surfaced separately.
    (owner, host, pid)
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-:@,+=".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn format_age(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.0}s", secs)
    } else if secs < 3600.0 {
        format!("{:.0}m", secs / 60.0)
    } else if secs < 86400.0 {
        format!("{:.1}h", secs / 3600.0)
    } else {
        format!("{:.1}d", secs / 86400.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_owner_record_full() {
        let (o, h, p) = parse_owner_record("meow@eda:12345:1717820000");
        assert_eq!(o.as_deref(), Some("meow"));
        assert_eq!(h.as_deref(), Some("eda"));
        assert_eq!(p, Some(12345));
    }

    #[test]
    fn parse_owner_record_qualified_host() {
        let (o, h, p) = parse_owner_record("meow@eda.corp.com:999:1717820000");
        assert_eq!(o.as_deref(), Some("meow"));
        assert_eq!(h.as_deref(), Some("eda.corp.com"));
        assert_eq!(p, Some(999));
    }

    #[test]
    fn parse_owner_record_missing_owner() {
        let (o, h, p) = parse_owner_record("eda:12345:1717820000");
        assert_eq!(o, None);
        assert_eq!(h.as_deref(), Some("eda"));
        assert_eq!(p, Some(12345));
    }

    #[test]
    fn parse_owner_record_empty() {
        let (o, h, p) = parse_owner_record("");
        assert_eq!(o, None);
        assert_eq!(h, None);
        assert_eq!(p, None);
    }

    #[test]
    fn shell_quote_passes_through_safe_paths() {
        assert_eq!(
            shell_quote("/home/meow/projects/lib"),
            "/home/meow/projects/lib"
        );
        assert_eq!(
            shell_quote("/path/with spaces/lib"),
            "'/path/with spaces/lib'"
        );
    }

    #[test]
    fn format_age_buckets() {
        assert_eq!(format_age(30.0), "30s");
        assert_eq!(format_age(120.0), "2m");
        assert_eq!(format_age(3700.0), "1.0h");
        assert_eq!(format_age(90000.0), "1.0d");
    }

    #[test]
    fn parse_batch_output_parses_three_records() {
        let stdout = "---LOCK---\nmeow@eda:12345:1717820000\n---STAT---\n1717820000\n---END---\n\
                      ---LOCK---\nalice@eda:6789:1717820000\n---STAT---\n1717820000\n---END---\n\
                      ---LOCK---\n\n---STAT---\n0\n---END---\n";
        let paths = vec![
            "/lib/cell/maestro/.cdslck".to_string(),
            "/lib/cell2/layout/.cdslck".to_string(),
            "/lib/cell3/symbol/.cdslck".to_string(),
        ];
        let locks = parse_batch_output(stdout, &paths);
        assert_eq!(locks.len(), 3);
        assert_eq!(locks[0].owner.as_deref(), Some("meow"));
        assert_eq!(locks[0].pid, Some(12345));
        assert_eq!(locks[1].owner.as_deref(), Some("alice"));
        assert_eq!(locks[2].owner, None);
    }

    #[test]
    fn parse_batch_output_handles_missing_stat() {
        let stdout = "---LOCK---\nmeow@eda:12345:1717820000\n---STAT---\n\n---END---\n";
        let paths = vec!["/lib/c/maestro/.cdslck".to_string()];
        let locks = parse_batch_output(stdout, &paths);
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].mtime, None);
    }
}

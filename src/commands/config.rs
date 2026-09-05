//! `vcli config-check` — validate .env configuration and give actionable feedback.
//!
//! Designed for LLM-assisted repair: the output is structured JSON with
//! `errors`, `warnings`, and `suggestions` so an agent can quickly fix
//! misconfiguration instead of staring at a generic "connection failed".

use crate::config::Config;
use crate::error::Result;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;

/// All recognised VB_* environment variables.  Used to detect typos.
const RECOGNIZED_VARS: &[&str] = &[
    "VB_PROFILE",
    "VB_REMOTE_HOST",
    "VB_REMOTE_USER",
    "VB_PORT",
    "VB_JUMP_HOST",
    "VB_JUMP_USER",
    "VB_SSH_PORT",
    "VB_SSH_KEY",
    "VB_SSH_CONFIG",
    "VB_SSH_BACKEND",
    "VB_DISABLE_CONTROL_MASTER",
    "VB_TIMEOUT",
    "VB_READ_TIMEOUT",
    "VB_KEEP_REMOTE_FILES",
    "VB_SPECTRE_CMD",
    "VB_SPECTRE_ARGS",
    "VB_SPECTRE_MAX_WORKERS",
    "VB_SSH_MAX_SESSIONS",
    "VB_SSH_MAX_BULK_SESSIONS",
    "VB_SSH_RECONNECT_MAX_ATTEMPTS",
    "VB_SSH_RECONNECT_MAX_DELAY",
    "VB_SSH_KEEPALIVE_INTERVAL",
    "VB_SSH_KEEPALIVE_FAILURES",
    "VB_TRANSPORT_SHUTDOWN_GRACE",
    "VB_CADENCE_CSHRC",
    "VB_SPECTRE_BIN",
    "VB_GUI_HOST",
    "VB_DEPLOY_HOST",
    "VB_DAEMON_HOST",
    "VB_SPECTRE_HOST",
    "VB_REMOTE_SCRATCH_ROOT",
    "VB_TRANSPORT_DAEMON_SOCKET",
    "VB_TRANSPORT_DAEMON_TOKEN",
    // Deprecated / legacy — detected and warned about
    "VB_SESSION",
];

/// Variables that have been deprecated and should be migrated.
const DEPRECATED_VARS: &[(&str, &str)] = &[
    (
        "VB_SESSION",
        "Use --session <id> CLI argument instead. VB_SESSION is no longer read by Config.",
    ),
];

/// Variables that expect a boolean (1/true or 0/false).
const BOOLEAN_VARS: &[&str] = &[
    "VB_DISABLE_CONTROL_MASTER",
    "VB_KEEP_REMOTE_FILES",
];

/// Variables that expect an integer.
const INTEGER_VARS: &[(&str, u64, u64)] = &[
    ("VB_PORT", 1, 65535),
    ("VB_TIMEOUT", 1, 3600),
    ("VB_READ_TIMEOUT", 1, 3600),
    ("VB_SSH_PORT", 1, 65535),
    ("VB_SPECTRE_MAX_WORKERS", 1, 256),
    ("VB_SSH_MAX_SESSIONS", 1, 100),
    ("VB_SSH_MAX_BULK_SESSIONS", 1, 100),
    ("VB_SSH_RECONNECT_MAX_ATTEMPTS", 1, 100),
    ("VB_SSH_RECONNECT_MAX_DELAY", 1, 600),
    ("VB_SSH_KEEPALIVE_INTERVAL", 1, 600),
    ("VB_SSH_KEEPALIVE_FAILURES", 1, 20),
    ("VB_TRANSPORT_SHUTDOWN_GRACE", 0, 300),
];

/// Variables whose value is a local filesystem path that should exist.
/// Note: VB_CADENCE_CSHRC and VB_SPECTRE_BIN are remote paths (on the
/// Virtuoso host) and are intentionally NOT checked here.
const LOCAL_PATH_VARS: &[&str] = &["VB_SSH_KEY", "VB_SSH_CONFIG"];

/// Run `vcli config check`.
///
/// If `connect` is true, also perform live connectivity checks:
/// TCP port reachability, SSH authentication, and daemon response.
pub fn run(connect: bool) -> Result<Value> {
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();
    let mut recognized: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    let mut unrecognized: Vec<String> = Vec::new();

    // 1. Collect all VB_* env vars (after .env loading by Config::from_env).
    let all_vb_vars: BTreeSet<String> = env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("VB_"))
        .collect();

    // 2. Recognized vs unrecognized
    for var in &all_vb_vars {
        // Strip profile suffix (e.g. VB_REMOTE_HOST_prod → VB_REMOTE_HOST)
        let base = strip_profile_suffix(var);
        if RECOGNIZED_VARS.contains(&base.as_str()) {
            let val = env::var(var).unwrap_or_default();
            recognized.insert(var.clone(), sanitize_value(var, &val));
        } else {
            unrecognized.push(var.clone());
            warnings.push(json!({
                "var": var,
                "message": format!("Unrecognized variable '{var}'. Did you mean one of: {}", suggest_closest(var)),
                "fix": "Remove or rename to a recognized VB_* variable."
            }));
        }
    }

    // 3. Deprecated variables
    for (var, message) in DEPRECATED_VARS {
        if all_vb_vars.iter().any(|v| v == var || v.starts_with(&format!("{var}_"))) {
            warnings.push(json!({
                "var": var,
                "message": message,
                "fix": message
            }));
            suggestions.push(format!("Remove {var} from .env — {message}"));
        }
    }

    // 4. Integer validation (covers both base var and profile-suffixed variants
    //    e.g. VB_PORT and VB_PORT_prod)
    for (var, min, max) in INTEGER_VARS {
        let prefix = format!("{var}_");
        let variants: Vec<String> = env::vars()
            .map(|(k, _)| k)
            .filter(|k| k == var || k.starts_with(&prefix))
            .collect();
        for v in variants {
            if let Ok(raw) = env::var(&v) {
                if raw.is_empty() {
                    continue;
                }
                match raw.parse::<u64>() {
                    Ok(val) => {
                        if val < *min || val > *max {
                            let note = if var == &"VB_PORT" {
                                " — Config::from_env() will fall back to a username-derived default port (65000-65499)"
                            } else {
                                ""
                            };
                            errors.push(json!({
                                "var": v,
                                "value": val,
                                "message": format!("{v}={val} is out of range [{min}, {max}]{note}"),
                                "fix": format!("Set {v} to a value between {min} and {max}")
                            }));
                        }
                    }
                    Err(_) => {
                        let note = if var == &"VB_PORT" {
                            " — Config::from_env() will fall back to a username-derived default port (65000-65499)"
                        } else {
                            ""
                        };
                        errors.push(json!({
                            "var": v,
                            "value": raw,
                            "message": format!("{v}='{raw}' is not a valid integer{note}"),
                            "fix": format!("Set {v} to an integer (e.g. {v}=30)")
                        }));
                    }
                }
            }
        }
    }

    // 5. Boolean validation (covers base and profile-suffixed variants)
    for var in BOOLEAN_VARS {
        let prefix = format!("{var}_");
        let variants: Vec<String> = env::vars()
            .map(|(k, _)| k)
            .filter(|k| k == var || k.starts_with(&prefix))
            .collect();
        for v in variants {
            if let Ok(raw) = env::var(&v) {
                if raw.is_empty() {
                    continue;
                }
                let lower = raw.to_lowercase();
                if !matches!(lower.as_str(), "1" | "0" | "true" | "false" | "yes" | "no") {
                    warnings.push(json!({
                        "var": v,
                        "value": raw,
                        "message": format!("{v}='{raw}' is not a recognized boolean (expected 1/0/true/false)"),
                        "fix": format!("Set {v}=1 or {v}=0")
                    }));
                }
            }
        }
    }

    // 6. SSH backend validation
    if let Ok(backend) = env::var("VB_SSH_BACKEND") {
        let lower = backend.to_lowercase();
        if lower == "native" && !cfg!(feature = "native-ssh") {
            errors.push(json!({
                "var": "VB_SSH_BACKEND",
                "value": backend,
                "message": "VB_SSH_BACKEND=native requires the 'native-ssh' Cargo feature, which is not compiled into this binary",
                "fix": "Either rebuild with --features native-ssh, or set VB_SSH_BACKEND=openssh"
            }));
            suggestions.push("Set VB_SSH_BACKEND=openssh (this binary was built without native-ssh)".into());
        }
        if !matches!(lower.as_str(), "native" | "openssh" | "") {
            warnings.push(json!({
                "var": "VB_SSH_BACKEND",
                "value": backend,
                "message": format!("Unknown SSH backend '{backend}' (expected 'openssh' or 'native')"),
                "fix": "Set VB_SSH_BACKEND=openssh or VB_SSH_BACKEND=native"
            }));
        }
    }

    // 7. Local path existence (only check when set; covers base and profile variants)
    for var in LOCAL_PATH_VARS {
        let prefix = format!("{var}_");
        let variants: Vec<String> = env::vars()
            .map(|(k, _)| k)
            .filter(|k| k == var || k.starts_with(&prefix))
            .collect();
        for v in variants {
            if let Ok(path) = env::var(&v) {
                if path.is_empty() {
                    continue;
                }
                if !std::path::Path::new(&path).exists() {
                    warnings.push(json!({
                        "var": v,
                        "value": path,
                        "message": format!("{v} path '{path}' does not exist locally"),
                        "fix": format!("Verify the path is correct and accessible, or unset {v}")
                    }));
                }
            }
        }
    }

    // 8. Remote mode consistency
    let has_remote_host = env::var("VB_REMOTE_HOST").map(|v| !v.is_empty()).unwrap_or(false);
    let effective_port = if has_remote_host {
        env::var("VB_PORT").ok().filter(|v| !v.is_empty())
    } else {
        warnings.push(json!({
            "var": "VB_REMOTE_HOST",
            "message": "VB_REMOTE_HOST is not set — running in local mode (no SSH tunnel)",
            "fix": "Set VB_REMOTE_HOST=<hostname> for remote Virtuoso access"
        }));
        None
    };

    // 9. VB_SPECTRE_ARGS shell syntax
    if let Ok(args) = env::var("VB_SPECTRE_ARGS") {
        if !args.is_empty() && shlex::split(&args).is_none() {
            errors.push(json!({
                "var": "VB_SPECTRE_ARGS",
                "value": args,
                "message": "VB_SPECTRE_ARGS contains invalid shell syntax (unbalanced quotes?)",
                "fix": "Check quoting in VB_SPECTRE_ARGS"
            }));
        }
    }

    // 10. Transport daemon: socket + token consistency
    let has_daemon_socket = env::var("VB_TRANSPORT_DAEMON_SOCKET").map(|v| !v.is_empty()).unwrap_or(false);
    let has_daemon_token = env::var("VB_TRANSPORT_DAEMON_TOKEN").map(|v| !v.is_empty()).unwrap_or(false);
    if has_daemon_socket && !has_daemon_token {
        warnings.push(json!({
            "var": "VB_TRANSPORT_DAEMON_TOKEN",
            "message": "VB_TRANSPORT_DAEMON_SOCKET is set but VB_TRANSPORT_DAEMON_TOKEN is missing — daemon IPC may reject the connection",
            "fix": "Set VB_TRANSPORT_DAEMON_TOKEN to match the daemon's token"
        }));
    }

    // 11. Cross-variable consistency checks
    // 11a. Remote host set but port is default-derived (user may not realize)
    if has_remote_host && effective_port.is_none() {
        warnings.push(json!({
            "var": "VB_PORT",
            "message": "VB_REMOTE_HOST is set but VB_PORT is not explicitly configured — using username-derived default port. Verify this matches your daemon's port.",
            "fix": "Set VB_PORT explicitly to match your Virtuoso bridge port"
        }));
    }

    // 11b. Jump host set but jump user not set
    let has_jump_host = env::var("VB_JUMP_HOST").map(|v| !v.is_empty()).unwrap_or(false);
    let has_jump_user = env::var("VB_JUMP_USER").map(|v| !v.is_empty()).unwrap_or(false);
    if has_jump_host && !has_jump_user {
        warnings.push(json!({
            "var": "VB_JUMP_USER",
            "message": "VB_JUMP_HOST is set but VB_JUMP_USER is not — SSH will use the current local username, which may not match the jump host account",
            "fix": "Set VB_JUMP_USER to the username for the jump host, or verify the local username is correct"
        }));
    }

    // 11c. VB_TIMEOUT > VB_READ_TIMEOUT is logically inconsistent
    let timeout_val = env::var("VB_TIMEOUT").ok().and_then(|v| v.parse::<u64>().ok());
    let read_timeout_val = env::var("VB_READ_TIMEOUT").ok().and_then(|v| v.parse::<u64>().ok());
    if let (Some(t), Some(rt)) = (timeout_val, read_timeout_val) {
        if t > rt {
            warnings.push(json!({
                "var": "VB_TIMEOUT",
                "message": format!("VB_TIMEOUT={t}s is greater than VB_READ_TIMEOUT={rt}s — write operations would have more time than read operations, which is unusual"),
                "fix": "Typically VB_READ_TIMEOUT should be >= VB_TIMEOUT (reads like list_instances take longer)"
            }));
        }
    }

    // 11d. native backend but all native tuning params are defaults
    let backend = env::var("VB_SSH_BACKEND").unwrap_or_default();
    if backend.eq_ignore_ascii_case("native") {
        let native_tuned = env::var("VB_SSH_MAX_SESSIONS").is_ok()
            || env::var("VB_SSH_MAX_BULK_SESSIONS").is_ok()
            || env::var("VB_SSH_RECONNECT_MAX_ATTEMPTS").is_ok()
            || env::var("VB_SSH_KEEPALIVE_INTERVAL").is_ok();
        if !native_tuned {
            suggestions.push(
                "VB_SSH_BACKEND=native: consider tuning VB_SSH_MAX_SESSIONS, VB_SSH_MAX_BULK_SESSIONS, VB_SSH_KEEPALIVE_INTERVAL for your workload"
                    .into(),
            );
        }
    }

    // 12. .env file syntax and permission checks
    if let Some(env_path) = find_dotenv() {
        check_dotenv_file(&env_path, &mut warnings, &mut errors);
    }

    // Build Config to confirm it parses (catches any remaining issues)
    let config_status = match Config::from_env() {
        Ok(cfg) => json!({
            "parsed": true,
            "is_remote": cfg.is_remote(),
            "port": cfg.port,
            "ssh_backend": cfg.ssh_backend,
            "timeout": cfg.timeout,
            "read_timeout": cfg.read_timeout,
            "effective_port": effective_port
        }),
        Err(e) => {
            errors.push(json!({
                "var": "_config_parse",
                "message": format!("Config::from_env() failed: {e}"),
                "fix": "Fix the errors above and re-run"
            }));
            json!({ "parsed": false, "error": e.to_string() })
        }
    };

    let status = if errors.is_empty() {
        if warnings.is_empty() { "pass" } else { "warn" }
    } else {
        "fail"
    };

    // 13. Optional live connectivity checks (--connect flag)
    let connectivity = if connect {
        Some(run_connectivity_checks(&config_status))
    } else {
        None
    };

    Ok(json!({
        "status": status,
        "errors_count": errors.len(),
        "warnings_count": warnings.len(),
        "errors": errors,
        "warnings": warnings,
        "suggestions": suggestions,
        "recognized_vars": recognized,
        "unrecognized_vars": unrecognized,
        "config": config_status,
        "connectivity": connectivity,
    }))
}

/// Strip a profile suffix from a variable name.
/// `VB_REMOTE_HOST_prod` → `VB_REMOTE_HOST`
fn strip_profile_suffix(var: &str) -> String {
    // Profile suffixes are lowercase identifiers after the last underscore.
    // We check if the part after the last underscore is a valid profile name
    // (all lowercase alphanumeric) and the base is recognized.
    if let Some(pos) = var.rfind('_') {
        let base = &var[..pos];
        let suffix = &var[pos + 1..];
        if RECOGNIZED_VARS.contains(&base)
            && !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return base.to_string();
        }
    }
    var.to_string()
}

/// Find the closest recognized variable name for typo suggestions.
fn suggest_closest(var: &str) -> String {
    let base = strip_profile_suffix(var);
    // Shorter variable names need a tighter threshold to avoid false matches.
    // e.g. VB_FOO (6 chars) at distance 3 could match unrelated names.
    let max_dist = if base.len() < 8 { 2 } else { 3 };
    let mut best: Vec<(usize, &str)> = RECOGNIZED_VARS
        .iter()
        .map(|known| (levenshtein(&base, known), *known))
        .filter(|(d, _)| *d <= max_dist)
        .collect();
    best.sort_by_key(|(d, _)| *d);
    if best.is_empty() {
        "see `vcli init` template".into()
    } else {
        best.iter()
            .take(3)
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Simple Levenshtein distance for typo detection.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Redact sensitive values (tokens, keys) for safe output.
fn sanitize_value(var: &str, val: &str) -> Value {
    if var.contains("TOKEN") || var.contains("KEY") || var.contains("SECRET") {
        if val.is_empty() {
            json!("")
        } else {
            json!(format!("***redacted*** ({} chars)", val.len()))
        }
    } else {
        json!(val)
    }
}

/// Find the .env file that Config::from_env() would load (cwd → parent → …).
fn find_dotenv() -> Option<std::path::PathBuf> {
    let start = std::env::current_dir().ok()?;
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join(".env");
        if candidate.exists() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

/// Validate a .env file: duplicate keys, invalid lines, permissions, BOM.
fn check_dotenv_file(
    path: &std::path::Path,
    warnings: &mut Vec<Value>,
    _errors: &mut Vec<Value>,
) {
    let content = match std::fs::read(path) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(json!({
                "var": ".env",
                "message": format!("Cannot read .env file at {}: {e}", path.display()),
                "fix": "Check file permissions"
            }));
            return;
        }
    };

    // BOM check (UTF-8 BOM = EF BB BF)
    if content.starts_with(&[0xEF, 0xBB, 0xBF]) {
        warnings.push(json!({
            "var": ".env",
            "message": ".env file has a UTF-8 BOM — this can cause the first variable name to be malformed (e.g. '\\u{feff}VB_PORT')",
            "fix": "Save the file as UTF-8 without BOM"
        }));
    }

    let text = String::from_utf8_lossy(&content);
    let mut seen_keys: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (line_num, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip export prefix (common in shell-style .env)
        let line_for_parse = trimmed.strip_prefix("export ").unwrap_or(trimmed);

        // Valid line: KEY=VALUE or KEY="VALUE" or KEY='VALUE'
        if !line_for_parse.contains('=') {
            warnings.push(json!({
                "var": ".env",
                "message": format!(".env line {} is not in KEY=VALUE format: '{}'", line_num + 1, trimmed.chars().take(60).collect::<String>()),
                "fix": "Use KEY=VALUE format, or prefix with # for comments"
            }));
            continue;
        }

        if let Some(eq_pos) = line_for_parse.find('=') {
            let key = line_for_parse[..eq_pos].trim();
            // Validate key format: uppercase letters, digits, underscores
            if !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                warnings.push(json!({
                    "var": ".env",
                    "message": format!(".env line {}: variable name '{}' contains unusual characters (expected UPPER_SNAKE_CASE)", line_num + 1, key),
                    "fix": "Rename to UPPER_SNAKE_CASE format"
                }));
            }
            // Duplicate key detection
            if let Some(prev_line) = seen_keys.get(key) {
                warnings.push(json!({
                    "var": key,
                    "message": format!("Duplicate key '{}' in .env (lines {} and {}) — the later value overrides the earlier one", key, prev_line, line_num + 1),
                    "fix": "Remove the duplicate or combine into one line"
                }));
            } else {
                seen_keys.insert(key.to_string(), line_num + 1);
            }
        }
    }

    // Permission check: if .env contains secrets, it should be 600 (not world-readable)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            let has_secret = text.contains("TOKEN") || text.contains("KEY") || text.contains("SECRET");
            if has_secret && mode & 0o004 != 0 {
                warnings.push(json!({
                    "var": ".env",
                    "message": format!(".env file contains secrets but is world-readable (mode {:o}) — other users on this machine can read your tokens/keys", mode),
                    "fix": "Run: chmod 600 .env"
                }));
            }
        }
    }
}

/// Live connectivity checks: TCP reachability, SSH auth, daemon response.
fn run_connectivity_checks(config: &Value) -> Value {
    use std::net::TcpStream;
    use std::time::Duration;

    let is_remote = config.get("is_remote").and_then(|v| v.as_bool()).unwrap_or(false);
    if !is_remote {
        return json!({
            "status": "skipped",
            "reason": "not in remote mode (VB_REMOTE_HOST not set)"
        });
    }

    let host = config.get("remote_host").and_then(|v| v.as_str()).unwrap_or("");
    let port = config.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
    let ssh_port = config.get("ssh_port").and_then(|v| v.as_u64()).unwrap_or(22);

    let mut checks = Vec::new();

    // 1. TCP port reachability (SSH port)
    let tcp_target = format!("{host}:{ssh_port}");
    let tcp_ok = TcpStream::connect_timeout(
        &tcp_target.parse().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap()),
        Duration::from_secs(5),
    ).is_ok();
    checks.push(json!({
        "check": "tcp_connect",
        "target": tcp_target,
        "ok": tcp_ok,
        "duration_ms": 5000u64,
    }));

    // 2. SSH authentication test (if TCP succeeded)
    let mut ssh_ok = false;
    if tcp_ok {
        let ssh_result = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=accept-new", "-p", &ssh_port.to_string(), host, "true"])
            .output();
        ssh_ok = ssh_result.map(|o| o.status.success()).unwrap_or(false);
        checks.push(json!({
            "check": "ssh_auth",
            "target": host,
            "ok": ssh_ok,
        }));
    } else {
        checks.push(json!({
            "check": "ssh_auth",
            "target": host,
            "ok": false,
            "skipped": "tcp_connect failed",
        }));
    }

    // 3. Daemon response test (if SSH succeeded)
    let mut daemon_ok = false;
    if ssh_ok {
        let daemon_result = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-p", &ssh_port.to_string(), host, &format!("VB_PORT={port} vcli session list --format json")])
            .output();
        if let Ok(output) = daemon_result {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                daemon_ok = stdout.contains("\"status\"") || stdout.contains("sessions");
            }
        }
        checks.push(json!({
            "check": "daemon_response",
            "port": port,
            "ok": daemon_ok,
        }));
    } else {
        checks.push(json!({
            "check": "daemon_response",
            "port": port,
            "ok": false,
            "skipped": "ssh_auth failed",
        }));
    }

    let all_ok = checks.iter().all(|c| c.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    json!({
        "status": if all_ok { "pass" } else { "fail" },
        "host": host,
        "checks": checks,
    })
}

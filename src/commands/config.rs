//! `vcli config check` — validate .env configuration and give actionable feedback.
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
const LOCAL_PATH_VARS: &[&str] = &["VB_SSH_KEY", "VB_SSH_CONFIG", "VB_CADENCE_CSHRC", "VB_SPECTRE_BIN"];

/// Run `vcli config check`.
pub fn run() -> Result<Value> {
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

    // 4. Integer validation
    for (var, min, max) in INTEGER_VARS {
        if let Ok(raw) = env::var(var) {
            if raw.is_empty() {
                continue;
            }
            match raw.parse::<u64>() {
                Ok(v) => {
                    if v < *min || v > *max {
                        errors.push(json!({
                            "var": var,
                            "value": v,
                            "message": format!("{var}={v} is out of range [{min}, {max}]"),
                            "fix": format!("Set {var} to a value between {min} and {max}")
                        }));
                    }
                }
                Err(_) => {
                    errors.push(json!({
                        "var": var,
                        "value": raw,
                        "message": format!("{var}='{raw}' is not a valid integer"),
                        "fix": format!("Set {var} to an integer (e.g. {var}=30)")
                    }));
                }
            }
        }
    }

    // 5. Boolean validation
    for var in BOOLEAN_VARS {
        if let Ok(raw) = env::var(var) {
            if raw.is_empty() {
                continue;
            }
            let lower = raw.to_lowercase();
            if !matches!(lower.as_str(), "1" | "0" | "true" | "false" | "yes" | "no") {
                warnings.push(json!({
                    "var": var,
                    "value": raw,
                    "message": format!("{var}='{raw}' is not a recognized boolean (expected 1/0/true/false)"),
                    "fix": format!("Set {var}=1 or {var}=0")
                }));
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

    // 7. Local path existence (only check when set)
    for var in LOCAL_PATH_VARS {
        if let Ok(path) = env::var(var) {
            if path.is_empty() {
                continue;
            }
            if !std::path::Path::new(&path).exists() {
                warnings.push(json!({
                    "var": var,
                    "value": path,
                    "message": format!("{var} path '{path}' does not exist locally"),
                    "fix": format!("Verify the path is correct and accessible, or unset {var}")
                }));
            }
        }
    }

    // 8. Remote mode consistency
    let has_remote_host = env::var("VB_REMOTE_HOST").map(|v| !v.is_empty()).unwrap_or(false);
    if has_remote_host {
        // If remote host is set but port is default-derived, that's fine — just info
        let port = env::var("VB_PORT").unwrap_or_else(|_| "(default)".into());
        recognized.insert("_effective_port".into(), json!(port));
    } else {
        warnings.push(json!({
            "var": "VB_REMOTE_HOST",
            "message": "VB_REMOTE_HOST is not set — running in local mode (no SSH tunnel)",
            "fix": "Set VB_REMOTE_HOST=<hostname> for remote Virtuoso access"
        }));
    }

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

    // Build Config to confirm it parses (catches any remaining issues)
    let config_status = match Config::from_env() {
        Ok(cfg) => json!({
            "parsed": true,
            "is_remote": cfg.is_remote(),
            "port": cfg.port,
            "ssh_backend": cfg.ssh_backend,
            "timeout": cfg.timeout,
            "read_timeout": cfg.read_timeout
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
    let mut best: Vec<(usize, &str)> = RECOGNIZED_VARS
        .iter()
        .map(|known| (levenshtein(&base, known), *known))
        .filter(|(d, _)| *d <= 3)
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

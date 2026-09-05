//! Integration tests for `vcli config check`.
//!
//! Each test deliberately misconfigures one variable and asserts that
//! `commands::config::run()` produces the expected error or warning.
//! Env vars are scoped per-test via a guard that restores the previous state.

use std::env;
use std::sync::Mutex;

/// Global lock so env-var-mutating tests don't race each other.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Set an env var for the duration of the closure, then restore.
fn with_env_var<F: FnOnce()>(key: &str, val: Option<&str>, f: F) {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = env::var(key).ok();
    match val {
        Some(v) => env::set_var(key, v),
        None => env::remove_var(key),
    }
    f();
    match prev {
        Some(v) => env::set_var(key, v),
        None => env::remove_var(key),
    }
}

/// Set multiple env vars for the duration of the closure, then restore.
/// Avoids nested with_env_var calls which would deadlock on ENV_LOCK.
fn with_env_vars<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
    let _guard = ENV_LOCK.lock().unwrap();
    let prevs: Vec<(&str, Option<String>)> = vars
        .iter()
        .map(|(k, _)| (*k, env::var(k).ok()))
        .collect();
    for (key, val) in vars {
        match val {
            Some(v) => env::set_var(key, v),
            None => env::remove_var(key),
        }
    }
    f();
    for (key, prev) in prevs {
        match prev {
            Some(v) => env::set_var(key, v),
            None => env::remove_var(key),
        }
    }
}

fn run_check() -> serde_json::Value {
    virtuoso_cli::commands::config::run(false).unwrap()
}

fn status_of(result: &serde_json::Value) -> &str {
    result["status"].as_str().unwrap()
}

fn has_error_var(result: &serde_json::Value, var: &str) -> bool {
    result["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["var"].as_str() == Some(var))
}

fn has_warning_var(result: &serde_json::Value, var: &str) -> bool {
    result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["var"].as_str() == Some(var))
}

// ── Happy path ──────────────────────────────────────────────────

#[test]
fn test_clean_config_passes() {
    // Remove all VB_* vars that could interfere, then restore after.
    let _guard = ENV_LOCK.lock().unwrap();
    let vb_keys: Vec<(String, Option<String>)> = env::vars()
        .filter(|(k, _)| k.starts_with("VB_"))
        .map(|(k, v)| (k, Some(v)))
        .collect();
    for (k, _) in &vb_keys {
        env::remove_var(k);
    }
    let result = run_check();
    // Restore
    for (k, v) in &vb_keys {
        if let Some(val) = v {
            env::set_var(k, val);
        }
    }
    // With no VB_* vars, we expect at least a warning about VB_REMOTE_HOST
    assert!(
        status_of(&result) == "pass" || status_of(&result) == "warn",
        "clean config should pass or warn, got: {}",
        result
    );
}

// ── Integer validation ─────────────────────────────────────────

#[test]
fn test_port_non_integer_is_error() {
    with_env_var("VB_PORT", Some("not-a-number"), || {
        let result = run_check();
        assert_eq!(status_of(&result), "fail");
        assert!(has_error_var(&result, "VB_PORT"));
    });
}

#[test]
fn test_port_out_of_range_is_error() {
    with_env_var("VB_PORT", Some("99999"), || {
        let result = run_check();
        assert_eq!(status_of(&result), "fail");
        assert!(has_error_var(&result, "VB_PORT"));
    });
}

#[test]
fn test_port_zero_is_error() {
    with_env_var("VB_PORT", Some("0"), || {
        let result = run_check();
        assert_eq!(status_of(&result), "fail");
        assert!(has_error_var(&result, "VB_PORT"));
    });
}

#[test]
fn test_timeout_non_integer_is_error() {
    with_env_var("VB_TIMEOUT", Some("abc"), || {
        let result = run_check();
        assert!(has_error_var(&result, "VB_TIMEOUT"));
    });
}

#[test]
fn test_valid_port_passes() {
    with_env_var("VB_PORT", Some("33817"), || {
        let result = run_check();
        assert!(!has_error_var(&result, "VB_PORT"));
    });
}

#[test]
fn test_profile_suffix_integer_is_validated() {
    // VB_PORT_prod=abc should be caught as an error, not silently recognized
    with_env_var("VB_PORT_prod", Some("not-a-number"), || {
        let result = run_check();
        assert!(has_error_var(&result, "VB_PORT_prod"));
        // Should also be recognized (not flagged as unrecognized)
        let unrecognized = result["unrecognized_vars"].as_array().unwrap();
        assert!(
            !unrecognized.iter().any(|v| v == "VB_PORT_prod"),
            "profile-suffixed var should be recognized"
        );
    });
}

// ── Unrecognized variable (typo) ───────────────────────────────

#[test]
fn test_typo_var_is_warning() {
    with_env_var("VB_REMOTE_HOS", Some("eda-01"), || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_REMOTE_HOS"));
        // Suggestion should mention the correct name
        let warning = result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["var"].as_str() == Some("VB_REMOTE_HOS"))
            .unwrap();
        let msg = warning["message"].as_str().unwrap();
        assert!(
            msg.contains("VB_REMOTE_HOST") || msg.contains("Did you mean"),
            "typo warning should suggest correct name: {msg}"
        );
    });
}

// ── Deprecated variable ────────────────────────────────────────

#[test]
fn test_deprecated_vb_session_is_warning() {
    with_env_var("VB_SESSION", Some("dean-user1-12345"), || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_SESSION"));
    });
}

// ── Boolean validation ─────────────────────────────────────────

#[test]
fn test_invalid_boolean_is_warning() {
    with_env_var("VB_KEEP_REMOTE_FILES", Some("maybe"), || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_KEEP_REMOTE_FILES"));
    });
}

#[test]
fn test_valid_boolean_passes() {
    with_env_var("VB_KEEP_REMOTE_FILES", Some("true"), || {
        let result = run_check();
        assert!(!has_warning_var(&result, "VB_KEEP_REMOTE_FILES"));
    });
}

// ── SSH backend validation ─────────────────────────────────────

#[test]
fn test_unknown_ssh_backend_is_warning() {
    with_env_var("VB_SSH_BACKEND", Some("libssh"), || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_SSH_BACKEND"));
    });
}

// Note: native-ssh feature test is compile-time gated; we test
// that openssh backend passes regardless.
#[test]
fn test_openssh_backend_passes() {
    with_env_var("VB_SSH_BACKEND", Some("openssh"), || {
        let result = run_check();
        assert!(!has_error_var(&result, "VB_SSH_BACKEND"));
        assert!(!has_warning_var(&result, "VB_SSH_BACKEND"));
    });
}

// ── Path validation ────────────────────────────────────────────

#[test]
fn test_nonexistent_ssh_key_is_warning() {
    with_env_var("VB_SSH_KEY", Some("/nonexistent/path/key.pem"), || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_SSH_KEY"));
    });
}

// ── Spectre args validation ────────────────────────────────────

#[test]
fn test_invalid_spectre_args_is_error() {
    with_env_var("VB_SPECTRE_ARGS", Some("+escargs \"unclosed"), || {
        let result = run_check();
        assert!(has_error_var(&result, "VB_SPECTRE_ARGS"));
    });
}

// ── Remote host missing ────────────────────────────────────────

#[test]
fn test_missing_remote_host_is_warning() {
    with_env_var("VB_REMOTE_HOST", None, || {
        let result = run_check();
        assert!(has_warning_var(&result, "VB_REMOTE_HOST"));
    });
}

// ── Daemon socket without token ────────────────────────────────

#[test]
fn test_daemon_socket_without_token_is_warning() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev_socket = env::var("VB_TRANSPORT_DAEMON_SOCKET").ok();
    let prev_token = env::var("VB_TRANSPORT_DAEMON_TOKEN").ok();
    env::set_var("VB_TRANSPORT_DAEMON_SOCKET", "/tmp/vcli-daemon.sock");
    env::remove_var("VB_TRANSPORT_DAEMON_TOKEN");
    let result = run_check();
    // Restore
    match prev_socket { Some(v) => env::set_var("VB_TRANSPORT_DAEMON_SOCKET", v), None => env::remove_var("VB_TRANSPORT_DAEMON_SOCKET") }
    match prev_token { Some(v) => env::set_var("VB_TRANSPORT_DAEMON_TOKEN", v), None => env::remove_var("VB_TRANSPORT_DAEMON_TOKEN") }
    assert!(has_warning_var(&result, "VB_TRANSPORT_DAEMON_TOKEN"));
}

// ── Token redaction ────────────────────────────────────────────

#[test]
fn test_token_is_redacted_in_output() {
    with_env_var("VB_TRANSPORT_DAEMON_TOKEN", Some("super-secret-token-123"), || {
        let result = run_check();
        let recognized = &result["recognized_vars"];
        let token_val = recognized["VB_TRANSPORT_DAEMON_TOKEN"].as_str().unwrap_or("");
        assert!(
            token_val.contains("redacted") && !token_val.contains("super-secret"),
            "token should be redacted, got: {token_val}"
        );
    });
}

// ── Profile suffix handling ────────────────────────────────────

#[test]
fn test_profile_suffix_var_is_recognized() {
    with_env_var("VB_REMOTE_HOST_prod", Some("eda-prod-01"), || {
        let result = run_check();
        // Should NOT be flagged as unrecognized
        let unrecognized = result["unrecognized_vars"].as_array().unwrap();
        assert!(
            !unrecognized.iter().any(|v| v == "VB_REMOTE_HOST_prod"),
            "profile-suffixed var should be recognized"
        );
    });
}

// ── Multiple errors aggregation ────────────────────────────────

#[test]
fn test_multiple_errors_aggregated() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Set multiple bad values
    env::set_var("VB_PORT", "bad");
    env::set_var("VB_TIMEOUT", "worse");
    env::set_var("VB_SPECTRE_ARGS", "\"unclosed");
    let result = run_check();
    // Clean up
    env::remove_var("VB_PORT");
    env::remove_var("VB_TIMEOUT");
    env::remove_var("VB_SPECTRE_ARGS");

    assert_eq!(status_of(&result), "fail");
    assert!(result["errors_count"].as_i64().unwrap() >= 3);
    assert!(has_error_var(&result, "VB_PORT"));
    assert!(has_error_var(&result, "VB_TIMEOUT"));
    assert!(has_error_var(&result, "VB_SPECTRE_ARGS"));
}

// ── Suggestions field ──────────────────────────────────────────

#[test]
fn test_suggestions_present_for_deprecated() {
    with_env_var("VB_SESSION", Some("x"), || {
        let result = run_check();
        let suggestions = result["suggestions"].as_array().unwrap();
        assert!(
            !suggestions.is_empty(),
            "deprecated var should produce at least one suggestion"
        );
    });
}

// --- Cross-variable consistency checks (P0 enhancement) ---

#[test]
fn test_remote_host_without_explicit_port_is_warning() {
    with_env_vars(&[("VB_REMOTE_HOST", Some("eda-server")), ("VB_PORT", None)], || {
        let result = run_check();
        assert!(
            has_warning_var(&result, "VB_PORT"),
            "remote host without explicit port should warn about default-derived port"
        );
    });
}

#[test]
fn test_jump_host_without_jump_user_is_warning() {
    with_env_vars(&[("VB_JUMP_HOST", Some("bastion.example.com")), ("VB_JUMP_USER", None)], || {
        let result = run_check();
        assert!(
            has_warning_var(&result, "VB_JUMP_USER"),
            "jump host without jump user should warn"
        );
    });
}

#[test]
fn test_timeout_greater_than_read_timeout_is_warning() {
    with_env_vars(&[("VB_TIMEOUT", Some("60")), ("VB_READ_TIMEOUT", Some("30"))], || {
        let result = run_check();
        assert!(
            has_warning_var(&result, "VB_TIMEOUT"),
            "VB_TIMEOUT > VB_READ_TIMEOUT should warn"
        );
    });
}

#[test]
fn test_native_backend_without_tuning_produces_suggestion() {
    with_env_vars(&[
        ("VB_SSH_BACKEND", Some("native")),
        ("VB_SSH_MAX_SESSIONS", None),
        ("VB_SSH_KEEPALIVE_INTERVAL", None),
    ], || {
        let result = run_check();
        let suggestions = result["suggestions"].as_array().unwrap();
        let has_native_suggestion = suggestions.iter().any(|s| {
            s.as_str().unwrap_or("").contains("native")
        });
        assert!(
            has_native_suggestion,
            "native backend without tuning should produce a suggestion"
        );
    });
}

// --- .env file checks (P0 enhancement) ---

/// Run config-check in a temp directory containing a .env file.
fn run_with_dotenv(env_content: &str) -> serde_json::Value {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("vcli_cfg_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let env_path = tmp.join(".env");
    std::fs::write(&env_path, env_content).unwrap();
    let prev_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();
    let result = virtuoso_cli::commands::config::run(false).unwrap();
    std::env::set_current_dir(&prev_dir).unwrap();
    std::fs::remove_dir_all(&tmp).ok();
    result
}

#[test]
fn test_dotenv_duplicate_key_is_warning() {
    let result = run_with_dotenv("VB_PORT=3000\nVB_PORT=3001\n");
    assert!(
        has_warning_var(&result, "VB_PORT"),
        "duplicate key in .env should warn"
    );
}

#[test]
fn test_dotenv_invalid_line_is_warning() {
    let result = run_with_dotenv("VB_PORT=3000\nthis is not valid\n");
    assert!(
        has_warning_var(&result, ".env"),
        "invalid line in .env should warn"
    );
}

#[test]
fn test_dotenv_bom_is_warning() {
    // UTF-8 BOM + content
    let content = [0xEF, 0xBB, 0xBF].iter().chain(b"VB_PORT=3000\n".iter()).copied().collect::<Vec<u8>>();
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("vcli_cfg_bom_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join(".env"), &content).unwrap();
    let prev_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();
    let result = virtuoso_cli::commands::config::run(false).unwrap();
    std::env::set_current_dir(&prev_dir).unwrap();
    std::fs::remove_dir_all(&tmp).ok();
    assert!(
        has_warning_var(&result, ".env"),
        ".env with BOM should warn"
    );
}

#[test]
fn test_dotenv_clean_file_no_warning() {
    let result = run_with_dotenv("VB_PORT=3000\nVB_REMOTE_HOST=eda\n# comment\n\n");
    // A clean .env should not produce .env-specific warnings
    let env_warnings: Vec<_> = result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["var"].as_str() == Some(".env"))
        .collect();
    assert!(
        env_warnings.is_empty(),
        "clean .env should not produce .env warnings, got: {:?}",
        env_warnings
    );
}

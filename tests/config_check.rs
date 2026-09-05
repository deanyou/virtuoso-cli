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

fn run_check() -> serde_json::Value {
    virtuoso_cli::commands::config::run().unwrap()
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

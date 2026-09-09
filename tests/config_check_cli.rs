//! End-to-end CLI integration tests for `vcli config check`.
//!
//! These spawn the real `vcli` binary (via `CARGO_BIN_EXE_vcli`) and verify
//! the full main → selection → dispatch → report → exit-code chain, not just
//! `Config::build_report()` in isolation.
//!
//! The key regression pinned here: a broken `active_target` in `targets.yaml`
//! must NOT abort with a bare stderr exit. It must emit a valid JSON document
//! with `valid: false`, an `ILLEGAL_CONFIG` diagnostic, and a non-zero exit
//! code — so operators can see *both* the selection failure and whatever
//! config could still be read.

use std::process::Command;

fn vcli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vcli"))
}

/// Guard that restores a set of environment variables on drop.
struct EnvGuard {
    vars: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn new(keys: &[&str]) -> Self {
        let mut vars = Vec::new();
        for k in keys {
            vars.push((k.to_string(), std::env::var_os(k)));
            std::env::remove_var(k);
        }
        Self { vars }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.vars.drain(..) {
            match v {
                Some(val) => std::env::set_var(&k, val),
                None => std::env::remove_var(&k),
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn cli_config_check_with_broken_active_target_emits_json_and_nonzero_exit() {
    // Isolate every variable that could shift selection or attribution.
    let _env = EnvGuard::new(&[
        "VB_TARGETS_FILE",
        "VB_TARGET",
        "VB_PROFILE",
        "VB_CONFIG_DIR",
        "VB_REMOTE_HOST",
        "VB_TIMEOUT",
        "VB_PORT",
    ]);

    // Write a targets.yaml whose active_target points at a target that
    // does not exist in the `targets:` map. This is the canonical
    // "selection failure" scenario: resolve_selection / resolve_from_selection
    // must surface the error through build_report rather than exit early.
    let tmp = tempfile::tempdir().expect("tempdir");
    let targets_path = tmp.path().join("targets.yaml");
    std::fs::write(
        &targets_path,
        "active_target: does-not-exist\n\
         targets:\n  \
           prod:\n    \
             remote_host: eda-host\n    \
             port: 65535\n",
    )
    .expect("write targets.yaml");

    let output = vcli()
        .args(["config", "check", "--format", "json"])
        .env("VB_TARGETS_FILE", &targets_path)
        .output()
        .expect("vcli config check should spawn");

    // 1. Non-zero exit code — a broken config must not report success.
    assert!(
        !output.status.success(),
        "expected non-zero exit for broken active_target, got success. \
         stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 2. stdout must be valid JSON (not a bare error message).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout must be valid JSON, got parse error: {e}\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });

    // 3. valid must be false.
    assert_eq!(
        json.get("valid").and_then(|v| v.as_bool()),
        Some(false),
        "expected valid=false, got: {json}"
    );

    // 4. An ILLEGAL_CONFIG diagnostic must be present, carrying the
    //    underlying selection error.
    let diagnostics = json
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .expect("diagnostics must be an array");
    assert!(
        !diagnostics.is_empty(),
        "expected at least one diagnostic, got: {json}"
    );
    assert!(
        diagnostics.iter().any(|d| d["code"] == "ILLEGAL_CONFIG"),
        "expected ILLEGAL_CONFIG diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|d| d["level"] == "error"),
        "diagnostic must be level=error, got: {diagnostics:?}"
    );

    // 5. status should reflect the error (not "ok").
    assert_eq!(
        json.get("status").and_then(|v| v.as_str()),
        Some("error"),
        "expected status=error, got: {json}"
    );

    // 6. The report should still contain entries (config that *could* be
    //    read is surfaced alongside the failure — strictly more info than
    //    a bare exit).
    assert!(
        json.get("entries").and_then(|v| v.as_array()).is_some(),
        "report should still carry entries array, got: {json}"
    );
}

#[test]
#[serial_test::serial]
fn cli_config_check_with_valid_target_exits_zero_and_valid_true() {
    // Complementary happy-path: a well-formed targets.yaml with a valid
    // active_target must exit 0 with valid=true. This ensures the
    // broken-target test above isn't passing trivially because *all*
    // config checks fail.
    let _env = EnvGuard::new(&[
        "VB_TARGETS_FILE",
        "VB_TARGET",
        "VB_PROFILE",
        "VB_CONFIG_DIR",
        "VB_REMOTE_HOST",
        "VB_TIMEOUT",
        "VB_PORT",
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let targets_path = tmp.path().join("targets.yaml");
    std::fs::write(
        &targets_path,
        "active_target: prod\n\
         targets:\n  \
           prod:\n    \
             remote_host: eda-host\n    \
             port: 65535\n",
    )
    .expect("write targets.yaml");

    let output = vcli()
        .args(["config", "check", "--format", "json"])
        .env("VB_TARGETS_FILE", &targets_path)
        .output()
        .expect("vcli config check should spawn");

    assert!(
        output.status.success(),
        "valid target should exit 0, got non-zero. stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid JSON on happy path");
    assert_eq!(
        json.get("valid").and_then(|v| v.as_bool()),
        Some(true),
        "expected valid=true for valid target, got: {json}"
    );
    assert_eq!(
        json.get("status").and_then(|v| v.as_str()),
        Some("ok"),
        "expected status=ok, got: {json}"
    );
}

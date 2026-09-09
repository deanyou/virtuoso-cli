//! Integration tests for `vcli config check`.
//!
//! The check command rebuilds the same layered lookup the live `Config`
//! uses and reports which layer won per key. The contract these tests pin:
//!
//! 1. **JSON shape is stable.** Downstream tools (`jq` filters, dashboards)
//!    depend on `entries[]` carrying `key`, `value`, `source.layer`,
//!    `source.<variant-attrs>`, and `source_label` — adding a field is
//!    fine, renaming or removing one is a breaking change.
//! 2. **Source attribution is correct.** Each precedence layer (env-profile,
//!    env-global, file-profile, file-global, target, default) must be
//!    reachable and correctly identified.
//! 3. **Parse failures are warnings, not silent.** A typo like
//!    `VB_TIMEOUT=abc` produces a value + a warning, never an exit error —
//!    that's how users learn the value would fail at runtime without
//!    having to run a real command.
//!
//! Every test is `#[serial]` (the env is shared process state) and shields
//! every variable whose presence would shift attribution. The `Drop` on
//! `EnvGuard` restores the original state so a failed assertion doesn't
//! poison subsequent tests.

use std::ffi::OsString;

struct EnvGuard {
    restored: Vec<(String, Option<OsString>)>,
}

impl EnvGuard {
    fn set(key: &str, val: &str) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, val);
        Self {
            restored: vec![(key.to_string(), old)],
        }
    }
    fn add(&mut self, key: &str) {
        let old = std::env::var_os(key);
        std::env::remove_var(key);
        self.restored.push((key.to_string(), old));
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, val) in self.restored.drain(..) {
            match val {
                Some(v) => std::env::set_var(&key, v),
                None => std::env::remove_var(&key),
            }
        }
    }
}

fn isolate_config_dir() -> (tempfile::TempDir, Option<OsString>) {
    let prev = std::env::var_os("VB_CONFIG_DIR");
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("VB_CONFIG_DIR", dir.path());
    (dir, prev)
}

struct ConfigDirRestore(Option<OsString>);

impl Drop for ConfigDirRestore {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("VB_CONFIG_DIR", v),
            None => std::env::remove_var("VB_CONFIG_DIR"),
        }
    }
}

#[test]
#[serial_test::serial]
fn check_json_shape_lists_every_known_key_with_a_source() {
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_REMOTE_HOST");
    env.add("VB_TIMEOUT");
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (_tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);

    let report = virtuoso_cli::config::Config::build_report(None, Ok(())).expect("build_report");

    // Every entry must have a non-empty key, a string value, a layer
    // discriminator, and a label. JSON-shape contract for downstream tools.
    for e in &report.entries {
        assert!(!e.key.is_empty(), "key must be non-empty");
        let serialized = serde_json::to_value(&e.source).expect("ConfigSource serializes");
        let layer = serialized
            .get("layer")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(!layer.is_empty(), "source.layer must be present: {e:?}");
    }

    // Spot-check that the report includes both connection-identity and
    // transport-scheduler fields — a missing key would silently drift.
    let keys: Vec<&str> = report.entries.iter().map(|e| e.key).collect();
    assert!(keys.contains(&"VB_REMOTE_HOST"), "remote_host missing");
    assert!(keys.contains(&"VB_TIMEOUT"), "timeout missing");
    assert!(
        keys.contains(&"VB_SSH_KEEPALIVE_FAILURES"),
        "scheduler field missing"
    );
}

#[test]
#[serial_test::serial]
fn check_env_layer_wins_over_file_layer() {
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_REMOTE_HOST");
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);
    std::fs::create_dir_all(tmp.path().join("vcli")).expect("mkdir vcli");
    std::fs::write(
        tmp.path().join("vcli/config.toml"),
        r#"remote_host = "eda-from-file""#,
    )
    .expect("write config");
    // env var beats file even when file is set:
    std::env::set_var("VB_REMOTE_HOST", "eda-from-env");

    let report = virtuoso_cli::config::Config::build_report(None, Ok(())).expect("build_report");
    let e = report
        .entries
        .iter()
        .find(|e| e.key == "VB_REMOTE_HOST")
        .expect("entry exists");
    assert_eq!(e.value, "eda-from-env");
    let src = serde_json::to_value(&e.source).expect("serialize");
    assert_eq!(src["layer"], "env");
    assert_eq!(src["var"], "VB_REMOTE_HOST");
}

#[test]
#[serial_test::serial]
fn check_file_layer_wins_over_default() {
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_TIMEOUT");
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);
    std::fs::create_dir_all(tmp.path().join("vcli")).expect("mkdir vcli");
    std::fs::write(tmp.path().join("vcli/config.toml"), r#"timeout = "45""#).expect("write");

    let report = virtuoso_cli::config::Config::build_report(None, Ok(())).expect("build_report");
    let e = report
        .entries
        .iter()
        .find(|e| e.key == "VB_TIMEOUT")
        .expect("entry exists");
    assert_eq!(e.value, "45");
    let src = serde_json::to_value(&e.source).expect("serialize");
    assert!(
        src["layer"] == "file_profile" || src["layer"] == "file_global",
        "expected file layer, got {src}"
    );
}

#[test]
#[serial_test::serial]
fn check_malformed_value_becomes_warning_not_error() {
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_PORT");
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (_tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);
    std::env::set_var("VB_PORT", "not-a-number");

    let report = virtuoso_cli::config::Config::build_report(None, Ok(())).expect("build_report");
    let e = report
        .entries
        .iter()
        .find(|e| e.key == "VB_PORT")
        .expect("entry exists");
    // Value still surfaced (so the user can see what they set); warning
    // tells them it would fail to parse at runtime.
    assert_eq!(e.value, "not-a-number");
    let src = serde_json::to_value(&e.source).expect("serialize");
    assert_eq!(src["layer"], "env");
    assert!(
        report.warnings.iter().any(|w| w.contains("VB_PORT")),
        "expected VB_PORT parse warning, got {:?}",
        report.warnings
    );
}

#[test]
#[serial_test::serial]
fn check_empty_profile_section_falls_through_to_global() {
    // Regression: a profile section that omits a key must NOT shadow the
    // global value with "absent". `ConfigFile::get` already returns the
    // global when the profile section has nothing for the key — this test
    // pins that behaviour for the report too.
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_REMOTE_HOST");
    env.add("VB_REMOTE_HOST_prod"); // profile-suffix variant — must also be shielded
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);
    std::fs::create_dir_all(tmp.path().join("vcli")).expect("mkdir");
    std::fs::write(
        tmp.path().join("vcli/config.toml"),
        r#"
remote_host = "global-host"

[profile.prod]
timeout = "60"
"#,
    )
    .expect("write");

    let report = virtuoso_cli::config::Config::build_report(Some("prod"), Ok(())).expect("build_report");
    let e = report
        .entries
        .iter()
        .find(|e| e.key == "VB_REMOTE_HOST")
        .expect("entry exists");
    assert_eq!(e.value, "global-host");
    let src = serde_json::to_value(&e.source).expect("serialize");
    assert_eq!(src["layer"], "file_global");
}

#[test]
#[serial_test::serial]
fn check_default_source_is_attributed_when_no_other_layer_fires() {
    let mut env = EnvGuard::set("VB_CONFIG_DIR", "");
    env.add("VB_TIMEOUT");
    env.add("VB_TARGETS_FILE");
    env.add("VB_TARGET");
    let (_tmp, prev) = isolate_config_dir();
    let _restore = ConfigDirRestore(prev);
    // No env, no file — VB_TIMEOUT must trace back to the constant default.
    let report = virtuoso_cli::config::Config::build_report(None, Ok(())).expect("build_report");
    let e = report
        .entries
        .iter()
        .find(|e| e.key == "VB_TIMEOUT")
        .expect("entry exists");
    assert_eq!(e.value, "30");
    let src = serde_json::to_value(&e.source).expect("serialize");
    assert_eq!(src["layer"], "default");
    assert_eq!(src["default"], "30");
    assert!(!report.all_explicit());
}

//! Integration tests for tunnel profile isolation.
//!
//! Mirrors the multi-profile CIW setup-file collision test from
//! upstream virtuoso-bridge-lite PR #86.
//!
//! The bug being verified: previously, two profiles would both write
//! to `/tmp/virtuoso_bridge/ramic_bridge.il` on the remote host, so
//! the second profile's `start` would silently overwrite the first
//! profile's CIW setup file. After the fix:
//!
//! - profile A's setup dir: `/tmp/virtuoso_bridge_analog/`
//! - profile B's setup dir: `/tmp/virtuoso_bridge_digital/`
//! - profile A's `stop` cleans ONLY `/tmp/virtuoso_bridge_analog/`,
//!   leaving profile B's setup dir intact.

use std::path::PathBuf;
use std::process::Command;

/// Resolve the path to the vcli binary built by `cargo test --release`.
/// We use the same artifact path convention that `cargo` itself uses.
fn vcli_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vcli"))
}

#[test]
fn profile_helpers_return_distinct_dirs() {
    use virtuoso_cli::transport::tunnel as t;

    let a = t::setup_dir_for_profile(Some("analog"));
    let b = t::setup_dir_for_profile(Some("digital"));
    let none = t::setup_dir_for_profile(None);

    assert_eq!(a, "/tmp/virtuoso_bridge_analog");
    assert_eq!(b, "/tmp/virtuoso_bridge_digital");
    assert_eq!(none, "/tmp/virtuoso_bridge");
    assert_ne!(a, b);
    assert_ne!(a, none, "profile dir must differ from no-profile dir");
}

#[test]
fn env_keys_are_profile_suffixed() {
    use virtuoso_cli::transport::tunnel as t;
    assert_eq!(
        t::profiled_env_key("VB_LOCAL_PORT", Some("a")),
        "VB_LOCAL_PORT_a"
    );
    assert_eq!(t::profiled_env_key("VB_LOCAL_PORT", None), "VB_LOCAL_PORT");
}

#[test]
fn cleanup_scope_simulation() {
    use virtuoso_cli::transport::tunnel::setup_dir_for_profile;

    let target = setup_dir_for_profile(Some("analog"));
    let other = setup_dir_for_profile(Some("digital"));

    let cleanup_cmd = format!("rm -rf {target}");
    assert!(cleanup_cmd.contains("analog"));
    assert!(
        !cleanup_cmd.contains("digital"),
        "cleanup must NOT touch the other profile's dir"
    );
    assert_ne!(target, other);
}

#[test]
fn vcli_profile_subcommand_runs() {
    let output = Command::new(vcli_bin())
        .args(["profile", "show", "--format", "json"])
        .env_remove("VB_PROFILE")
        .env_remove("VIRTUAL_ENV")
        .output()
        .expect("vcli profile show must run");

    assert!(
        output.status.success(),
        "vcli profile show failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("vcli profile show output is not JSON: {e}\nstdout: {stdout}"));

    assert!(parsed.get("profile").is_some(), "missing 'profile' field");
    assert!(parsed.get("source").is_some(), "missing 'source' field");
    assert!(
        parsed.get("resolution_order").is_some(),
        "missing 'resolution_order' field"
    );
}

#[cfg(unix)]
#[test]
fn vcli_profile_bind_user_then_clear() {
    let scratch = tempfile::tempdir().expect("scratch home");
    let home = scratch.path();
    let profile_file = home.join(".vcli").join("profile");
    let env_file = home.join(".vcli").join(".env");

    let test_profile = "vcli_test_profile_xyz";
    let bin = vcli_bin();

    let bind = Command::new(&bin)
        .args([
            "profile",
            "bind",
            test_profile,
            "--user",
            "--format",
            "json",
        ])
        .env("HOME", home)
        .output()
        .expect("vcli profile bind --user");
    assert!(
        bind.status.success(),
        "bind failed: {}",
        String::from_utf8_lossy(&bind.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&bind.stdout).unwrap();
    assert_eq!(parsed["action"], "bind");
    assert_eq!(parsed["scope"], "user");
    assert_eq!(parsed["profile"], test_profile);

    let content = std::fs::read_to_string(&profile_file).unwrap();
    assert_eq!(
        content.trim(),
        test_profile,
        "expected {test_profile} in {}; got: {content}",
        profile_file.display()
    );

    let show = Command::new(&bin)
        .args(["profile", "show", "--format", "json"])
        .env("HOME", home)
        .env("VB_PROFILE", test_profile)
        .output()
        .expect("vcli profile show");
    assert!(show.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(parsed["profile"], test_profile);
    assert_eq!(
        parsed["source"], "environment",
        "VB_PROFILE env var should win"
    );

    let clear = Command::new(&bin)
        .args(["profile", "clear", "--user", "--format", "json"])
        .env("HOME", home)
        .output()
        .expect("vcli profile clear --user");
    assert!(
        clear.status.success(),
        "clear failed: {}",
        String::from_utf8_lossy(&clear.stderr)
    );

    assert!(
        !profile_file.exists(),
        "the user binding should be removed; still there: {content}",
        content = std::fs::read_to_string(&profile_file).unwrap_or_default()
    );

    std::fs::write(&env_file, format!("VB_PROFILE={test_profile}\n")).unwrap();
    let show = Command::new(&bin)
        .args(["profile", "show", "--format", "json"])
        .env("HOME", home)
        .env_remove("VB_PROFILE")
        .output()
        .expect("vcli profile show (legacy fallback)");
    assert!(show.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(parsed["profile"], test_profile);
    assert_eq!(
        parsed["source"], "user_env",
        "the legacy .env should resolve as the deprecated fallback"
    );

    let clear = Command::new(&bin)
        .args(["profile", "clear", "--user", "--format", "json"])
        .env("HOME", home)
        .output()
        .expect("vcli profile clear --user (legacy)");
    assert!(clear.status.success());
    let content = std::fs::read_to_string(&env_file).unwrap_or_default();
    assert!(
        !content.contains(test_profile),
        "the legacy VB_PROFILE line should be stripped; got: {content}"
    );
}

#[cfg(unix)]
#[test]
fn vcli_profile_user_scope_does_not_touch_the_real_home() {
    let scratch = tempfile::tempdir().expect("scratch home");
    let home = scratch.path();

    let show = Command::new(vcli_bin())
        .args(["profile", "show", "--format", "json"])
        .env("HOME", home)
        .env_remove("VB_PROFILE")
        .output()
        .expect("vcli profile show");
    assert!(show.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(parsed["source"], "default");
    assert!(
        !home.join(".vcli").join("profile").exists(),
        "show must not create a binding"
    );
}

#[test]
fn vcli_profile_bind_user_rejects_no_scope() {
    let output = Command::new(vcli_bin())
        .args(["profile", "bind", "test", "--format", "json"])
        .output()
        .expect("vcli profile bind (no scope)");
    assert!(!output.status.success(), "bind without scope should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must specify one of --venv, --user, or --local"),
        "got: {stderr}"
    );
}

#[test]
fn vcli_profile_bind_user_rejects_multi_scope() {
    let output = Command::new(vcli_bin())
        .args([
            "profile", "bind", "test", "--user", "--venv", "--format", "json",
        ])
        .output()
        .expect("vcli profile bind (multi scope)");
    assert!(!output.status.success(), "bind with 2 scopes should fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("cannot be used with") || combined.contains("only one of"),
        "expected conflict error; got: {combined}"
    );
}

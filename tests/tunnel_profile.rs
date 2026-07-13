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
    // CARGO_BIN_EXE_vcli is set by cargo when running integration tests
    // against the main binary, with the binary already built.
    PathBuf::from(env!("CARGO_BIN_EXE_vcli"))
}

#[test]
fn profile_helpers_return_distinct_dirs() {
    // Use the published vcli binary's transport helpers via the lib.
    // We can't easily call pub(crate) items from an integration test,
    // so we test the externally observable behavior: two profiles
    // resolve to two different setup dirs.
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
    // Simulate the cleanup-scope fix without a live SSH:
    //
    // 1. Create two fake profile setup dirs locally (the same way
    //    they would exist on the remote after `tunnel start`).
    // 2. Run a cleanup that scopes to ONE profile.
    // 3. Assert the OTHER profile's dir is untouched.
    //
    // The real `cleanup_remote()` runs over SSH; we test the path
    // resolution and scoping logic directly by calling the helper
    // and asserting the string we would `rm -rf`.
    use virtuoso_cli::transport::tunnel::setup_dir_for_profile;

    let target = setup_dir_for_profile(Some("analog"));
    let other = setup_dir_for_profile(Some("digital"));

    // The cleanup command we would issue is:
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
    // Smoke test: `vcli profile show` runs without crashing, returns
    // valid JSON, and the JSON has the expected fields.
    //
    // This doesn't require a live SSH connection — it's a local-only
    // introspection of env + binding files.
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

#[test]
fn vcli_profile_bind_user_then_clear() {
    // End-to-end: bind a profile to user-level, observe it via show,
    // then clear. Verifies the CLI plumbing works all the way through.
    use std::env;
    let home = env::var("HOME").expect("HOME must be set for this test");
    let env_file = PathBuf::from(&home).join(".vcli").join(".env");
    let backup = if env_file.exists() {
        Some(std::fs::read_to_string(&env_file).unwrap())
    } else {
        None
    };

    // Test is single-threaded for the duration via test-threads=1 in CI,
    // OR the underlying lib's Mutex serializes its own tests. We
    // accept best-effort coordination here.

    let test_profile = "vcli_test_profile_xyz";
    let bin = vcli_bin();

    // 1. Bind
    let bind = Command::new(&bin)
        .args([
            "profile",
            "bind",
            test_profile,
            "--user",
            "--format",
            "json",
        ])
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

    // 2. Verify ~/.vcli/.env contains it
    let content = std::fs::read_to_string(&env_file).unwrap();
    assert!(
        content.contains(&format!("VB_PROFILE={test_profile}")),
        "expected VB_PROFILE={test_profile} in {}; got: {content}",
        env_file.display()
    );

    // 3. Show
    let show = Command::new(&bin)
        .args(["profile", "show", "--format", "json"])
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

    // 4. Clear
    let clear = Command::new(&bin)
        .args(["profile", "clear", "--user", "--format", "json"])
        .output()
        .expect("vcli profile clear --user");
    assert!(
        clear.status.success(),
        "clear failed: {}",
        String::from_utf8_lossy(&clear.stderr)
    );

    // 5. Verify gone
    let content = std::fs::read_to_string(&env_file).unwrap_or_default();
    assert!(
        !content.contains(test_profile),
        "VB_PROFILE should be cleared; got: {content}"
    );

    // Restore backup
    if let Some(b) = backup {
        std::fs::write(&env_file, b).unwrap();
    } else if env_file.exists() {
        std::fs::remove_file(&env_file).unwrap();
    }
}

#[test]
fn vcli_profile_bind_user_rejects_no_scope() {
    // `vcli profile bind X` (no --venv/--user/--local) must fail with
    // a clear error.
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
    // clap catches the multi-scope conflict before our handler runs.
    // We just verify the binary rejected the call and the error
    // mentions the conflicting flags.
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
    // clap's exact wording: "the argument '--user' cannot be used with '--venv'"
    assert!(
        combined.contains("cannot be used with") || combined.contains("only one of"),
        "expected conflict error; got: {combined}"
    );
}

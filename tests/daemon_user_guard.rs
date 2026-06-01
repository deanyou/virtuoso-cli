//! Integration tests for the daemon-user guard, remote scratch scoping, and
//! stale-daemon hints added in v0.3.18+.
//!
//! These exercise only the Rust code paths that do not require a live Virtuoso
//! daemon. The `vcli session show` end-to-end integration is covered by the
//! loop running in /tmp/daemon_health/.

use std::env;
use std::sync::Mutex;
use virtuoso_cli::client::bridge::{remote_scratch_root, resolve_client_id, sanitize_client_id};
use virtuoso_cli::models::SessionInfo;

// ----------------------------------------------------------------------------
// sanitize_client_id
// ----------------------------------------------------------------------------

#[test]
fn sanitize_passes_alnum_dash_underscore_dot() {
    assert_eq!(sanitize_client_id("ABC-abc_1.2"), "ABC-abc_1.2");
}

#[test]
fn sanitize_replaces_path_separators_and_specials() {
    assert_eq!(sanitize_client_id("a/b\\c:d e\tf"), "a_b_c_d_e_f");
}

#[test]
fn sanitize_drops_unicode_replaces_with_underscore() {
    // 2 non-ASCII chars (each is one Unicode scalar) → 2 underscores
    assert_eq!(sanitize_client_id("meow-主机"), "meow-__");
    // 1 non-ASCII char → 1 underscore
    assert_eq!(sanitize_client_id("xé"), "x_");
}

#[test]
fn sanitize_handles_empty_string() {
    assert_eq!(sanitize_client_id(""), "");
}

#[test]
fn sanitize_preserves_leading_dots_and_dashes() {
    // Pathological but legal as a directory name
    assert_eq!(sanitize_client_id(".hidden"), ".hidden");
    assert_eq!(sanitize_client_id("-dash-prefix"), "-dash-prefix");
}

// ----------------------------------------------------------------------------
// resolve_client_id + remote_scratch_root (with env-var precedence)
// ----------------------------------------------------------------------------

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Clean all env vars that influence client_id resolution.
fn clear_client_id_env() {
    env::remove_var("VB_CLIENT_ID");
    env::remove_var("VB_PROFILE");
    env::remove_var("HOSTNAME");
}

#[test]
fn client_id_uses_explicit_override() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    env::set_var("VB_CLIENT_ID", "explicit-id");
    env::set_var("VB_PROFILE", "profile-x");
    // VB_CLIENT_ID wins over everything
    assert_eq!(resolve_client_id(), "explicit-id");
    assert_eq!(remote_scratch_root(), "/tmp/virtuoso_bridge/explicit-id");
    clear_client_id_env();
}

#[test]
fn client_id_uses_profile_when_no_explicit() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    env::set_var("VB_PROFILE", "profile-y");
    assert_eq!(resolve_client_id(), "profile-y");
    assert_eq!(remote_scratch_root(), "/tmp/virtuoso_bridge/profile-y");
    clear_client_id_env();
}

#[test]
fn client_id_empty_explicit_falls_through() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    env::set_var("VB_CLIENT_ID", "  ");
    env::set_var("VB_PROFILE", "profile-fallback");
    // Empty/whitespace VB_CLIENT_ID falls through to VB_PROFILE
    assert_eq!(resolve_client_id(), "profile-fallback");
    clear_client_id_env();
}

#[test]
fn client_id_falls_back_to_hostname() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    env::set_var("HOSTNAME", "test-host-9");
    // No VB_CLIENT_ID, no VB_PROFILE → use hostname
    assert_eq!(resolve_client_id(), "test-host-9");
    clear_client_id_env();
}

#[test]
fn client_id_sanitizes_hostname() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    env::set_var("HOSTNAME", "weird/host\\name");
    // Hostname with path separators should be sanitized
    assert_eq!(resolve_client_id(), "weird_host_name");
    clear_client_id_env();
}

#[test]
fn client_id_no_env_falls_back_to_default() {
    let _g = ENV_LOCK.lock().unwrap();
    clear_client_id_env();
    // Set HOSTNAME to empty to force the inner None branch
    env::set_var("HOSTNAME", "");
    // Both VB_CLIENT_ID and VB_PROFILE unset, HOSTNAME empty
    // → gethostname() called, but we can't control that in tests.
    // Either it returns "default" (if gethostname fails) or some sanitized
    // real hostname. We just check that the result is a non-empty sanitized
    // string under the same constraints.
    let id = resolve_client_id();
    assert!(!id.is_empty(), "client_id must never be empty");
    assert_eq!(id, sanitize_client_id(&id), "client_id must be sanitized");
    clear_client_id_env();
}

// ----------------------------------------------------------------------------
// SessionInfo JSON round-trip with the new optional daemon_user field
// ----------------------------------------------------------------------------

#[test]
fn session_info_serializes_without_daemon_user_when_none() {
    let s = SessionInfo {
        id: "test-12345".into(),
        port: 12345,
        pid: 9999,
        host: "test-host".into(),
        user: "tester".into(),
        created: "2026-06-01T00:00:00Z".into(),
        daemon_user: None,
    };
    let json = serde_json::to_string(&s).unwrap();
    // daemon_user is None → field should be OMITTED from JSON
    assert!(
        !json.contains("daemon_user"),
        "None daemon_user should be skipped: {json}"
    );
}

#[test]
fn session_info_serializes_with_daemon_user_when_some() {
    let s = SessionInfo {
        id: "test-12345".into(),
        port: 12345,
        pid: 9999,
        host: "test-host".into(),
        user: "tester".into(),
        created: "2026-06-01T00:00:00Z".into(),
        daemon_user: Some("alice".into()),
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"daemon_user\":\"alice\""), "got: {json}");
}

#[test]
fn session_info_deserializes_legacy_json_without_daemon_user() {
    // Older ramic_bridge.il (and older CLI versions) won't write daemon_user
    // at all. The struct must still parse such files via #[serde(default)].
    let legacy_json = r#"{
        "id": "legacy-9999",
        "port": 9999,
        "pid": 8888,
        "host": "old-host",
        "user": "legacy",
        "created": "2026-01-01T00:00:00Z"
    }"#;
    let s: SessionInfo = serde_json::from_str(legacy_json)
        .expect("must deserialize legacy JSON without daemon_user field");
    assert_eq!(s.id, "legacy-9999");
    assert_eq!(s.port, 9999);
    assert!(
        s.daemon_user.is_none(),
        "missing field should default to None"
    );
}

#[test]
fn session_info_deserializes_new_json_with_daemon_user() {
    let new_json = r#"{
        "id": "new-1111",
        "port": 1111,
        "pid": 2222,
        "host": "new-host",
        "user": "newuser",
        "created": "2026-06-01T00:00:00Z",
        "daemon_user": "bob"
    }"#;
    let s: SessionInfo = serde_json::from_str(new_json).unwrap();
    assert_eq!(s.daemon_user.as_deref(), Some("bob"));
}

// ----------------------------------------------------------------------------
// ramic_bridge.il recovery-hint strings (regression check)
// ----------------------------------------------------------------------------

#[test]
fn ramic_bridge_recovery_hint_strings_present() {
    // The "already running" branch of RBStart() must tell users how to recover
    // a stuck daemon. This protects against accidentally removing the hint
    // during future refactors of the SKILL.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ramic_bridge.il");
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    // Find the "is already running" branch
    let already_running_idx = src
        .find("is already running")
        .expect("ramic_bridge.il should still have the 'is already running' branch");
    // We just need a window after the print to look for the recovery hint.
    let after = &src[already_running_idx..];

    assert!(
        after.contains("RBStop()"),
        "recovery hint must mention RBStop() — got: {after}"
    );
    assert!(
        after.contains("RBStopAll()"),
        "recovery hint must mention RBStopAll() for stubborn cases — got: {after}"
    );
    assert!(
        after.contains("load("),
        "recovery hint must mention re-load — got: {after}"
    );
    // Both the bound port and the session id should be shown so the user
    // can confirm this is THEIR daemon.
    assert!(
        after.contains("RBPort") || after.contains("bound port"),
        "recovery hint should echo RBPort / bound port"
    );
    assert!(
        after.contains("RBSessionId") || after.contains("session id"),
        "recovery hint should echo RBSessionId / session id"
    );
}

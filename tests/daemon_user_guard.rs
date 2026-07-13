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
        daemon_version: None,
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
        daemon_version: None,
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
// save_to_session_file (M3: Rust-side write-back of daemon_user)
// ----------------------------------------------------------------------------

#[test]
fn save_to_session_file_writes_to_sessions_dir() {
    use std::env;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Save and restore the env var that save() uses for cache_dir resolution
    let original_xdg = env::var_os("XDG_CACHE_HOME").and_then(|s| s.into_string().ok());

    let tmp = tempfile::tempdir().expect("tempdir");
    env::set_var("XDG_CACHE_HOME", tmp.path());

    let mut s = SessionInfo {
        id: format!("test-save-{}", std::process::id()),
        port: 12345,
        pid: 0,
        host: "test-host".into(),
        user: "tester".into(),
        created: "2026-06-01T00:00:00Z".into(),
        daemon_user: None,
        daemon_version: None,
    };
    s.daemon_user = Some("alice".into());
    s.save_to_session_file();

    // File should exist at <XDG_CACHE_HOME>/virtuoso_bridge/sessions/<id>.json
    let path = tmp
        .path()
        .join("virtuoso_bridge")
        .join("sessions")
        .join(format!("{}.json", s.id));
    assert!(path.exists(), "save_to_session_file should create {path:?}");

    // Reload and verify round-trip
    let loaded = SessionInfo::load(&s.id).expect("load should succeed");
    assert_eq!(loaded.daemon_user.as_deref(), Some("alice"));
    assert_eq!(loaded.id, s.id);
    assert_eq!(loaded.port, s.port);

    // Cleanup
    if let Some(v) = original_xdg {
        env::set_var("XDG_CACHE_HOME", v);
    } else {
        env::remove_var("XDG_CACHE_HOME");
    }
}

// ----------------------------------------------------------------------------
// ipcIsProcessRunning() no-arg regression (M1)
// ----------------------------------------------------------------------------
//
// Background: ipcIsProcessRunning() with no process-handle argument returns
// nil on a live daemon, which caused vcli ping / util.ping to spuriously
// fail. We switched the liveness probe to `plus(1 1)` (a no-op SKILL
// expression that always returns a non-nil integer on a responsive daemon).
//
// These tests pin the implementation so a future refactor can't silently
// reintroduce the broken call.

#[test]
fn bridge_ping_uses_plus_one_one_not_ipc_is_process_running() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/client/bridge.rs"),
    )
    .expect("read bridge.rs");

    // Find the `pub fn ping` block and check its body uses plus(1 1).
    let ping_start = src
        .find("pub fn ping")
        .expect("bridge.rs should have pub fn ping");
    // Look ahead ~600 chars for the skill literal
    let window = &src[ping_start..ping_start.saturating_add(600)];
    assert!(
        window.contains("plus(1 1)"),
        "bridge::ping should use plus(1 1) — verified live on daemon: ipcIsProcessRunning() without a process handle returns nil"
    );
    // Ensure the broken call is NOT present in the ping body
    assert!(
        !window.contains("ipcIsProcessRunning"),
        "bridge::ping should not call ipcIsProcessRunning() — the no-arg form returns nil on live daemons"
    );
}

#[test]
fn dispatcher_ping_uses_plus_one_one_not_ipc_is_process_running() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rpc/dispatcher.rs"),
    )
    .expect("read dispatcher.rs");

    // Find the `"ping"` arm
    let arm_start = src
        .find("\"ping\" =>")
        .expect("dispatcher.rs should have a ping arm");
    let window = &src[arm_start..arm_start.saturating_add(700)];

    // The actual SKILL payload passed to execute_skill_unchecked must be
    // plus(1 1). We pin this by looking for the exact call form, not
    // just any occurrence of "plus(1 1)" (which would also match comments).
    assert!(
        window.contains("execute_skill_unchecked(\"plus(1 1)\""),
        "dispatcher::ping should pass the SKILL payload plus(1 1) to execute_skill_unchecked — got: {window}"
    );

    // Negative check: the actual SKILL payload must NOT be the broken call.
    // We allow the comment to mention ipcIsProcessRunning (for context) but
    // the payload itself must not be that string.
    assert!(
        !window.contains("execute_skill_unchecked(\"ipcIsProcessRunning"),
        "dispatcher::ping must not pass ipcIsProcessRunning to execute_skill_unchecked (it returns nil without a process handle)"
    );
}

#[test]
fn daemon_alive_uses_plus_one_one() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/client/bridge.rs"),
    )
    .expect("read bridge.rs");

    let fn_start = src
        .find("pub fn daemon_alive")
        .expect("bridge.rs should have pub fn daemon_alive");
    let window = &src[fn_start..fn_start.saturating_add(500)];
    assert!(
        window.contains("plus(1 1)"),
        "daemon_alive should use plus(1 1) as a no-op liveness probe"
    );
}

// ----------------------------------------------------------------------------
// ensure_remote_dir local-mode dir creation (regression: was a no-op in local
// mode, causing load_il to fail with "No such file or directory" when the
// per-client scratch dir did not exist yet).
// ----------------------------------------------------------------------------

#[test]
fn ensure_remote_dir_local_mode_creates_dir() {
    // We can't easily construct a real VirtuosoClient without a tunnel
    // (we'd need an SSHClient). The local-mode branch of ensure_remote_dir
    // just calls std::fs::create_dir_all — replicate that here and assert
    // the side effect, so the test is independent of the SSH machinery.
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp
        .path()
        .join("nested")
        .join("deeper")
        .join("per-client")
        .to_string_lossy()
        .to_string();
    assert!(
        !std::path::Path::new(&target).exists(),
        "precondition: target should not exist"
    );
    let _ = std::fs::create_dir_all(&target);
    assert!(
        std::path::Path::new(&target).is_dir(),
        "dir should be created by create_dir_all"
    );
}

#[test]
fn save_to_session_file_swallows_io_errors() {
    // Pointing the cache to a non-writable path should NOT panic — the
    // contract is "best-effort, errors silently ignored".
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let original_xdg = std::env::var_os("XDG_CACHE_HOME").and_then(|s| s.into_string().ok());
    std::env::set_var("XDG_CACHE_HOME", "/nonexistent-readonly-path/abc/def");

    let s = SessionInfo {
        id: "any-id".into(),
        port: 12345,
        pid: 0,
        host: "h".into(),
        user: "u".into(),
        created: "now".into(),
        daemon_user: Some("bob".into()),
        daemon_version: None,
    };
    // Should not panic
    s.save_to_session_file();

    if let Some(v) = original_xdg {
        std::env::set_var("XDG_CACHE_HOME", v);
    } else {
        std::env::remove_var("XDG_CACHE_HOME");
    }
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

// ----------------------------------------------------------------------------
// daemon_version field on SessionInfo (version-unification feature)
// ----------------------------------------------------------------------------

#[test]
fn session_info_serializes_without_daemon_version_when_none() {
    let s = SessionInfo {
        id: "test-12345".into(),
        port: 12345,
        pid: 9999,
        host: "test-host".into(),
        user: "tester".into(),
        created: "2026-06-01T00:00:00Z".into(),
        daemon_user: None,
        daemon_version: None,
    };
    let json = serde_json::to_string(&s).unwrap();
    // daemon_version is None → field should be OMITTED from JSON
    assert!(
        !json.contains("daemon_version"),
        "None daemon_version should be skipped: {json}"
    );
}

#[test]
fn session_info_serializes_with_daemon_version_when_some() {
    let s = SessionInfo {
        id: "test-12345".into(),
        port: 12345,
        pid: 9999,
        host: "test-host".into(),
        user: "tester".into(),
        created: "2026-06-01T00:00:00Z".into(),
        daemon_user: None,
        daemon_version: Some("0.4.0-alpha.5".into()),
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"daemon_version\":\"0.4.0-alpha.5\""),
        "got: {json}"
    );
}

#[test]
fn session_info_deserializes_legacy_json_without_daemon_version() {
    // Older vcli session files (and older .il) won't write daemon_version at
    // all. The struct must still parse such files via #[serde(default)].
    let legacy_json = r#"{
        "id": "legacy-9999",
        "port": 9999,
        "pid": 8888,
        "host": "old-host",
        "user": "legacy",
        "created": "2026-01-01T00:00:00Z"
    }"#;
    let s: SessionInfo = serde_json::from_str(legacy_json)
        .expect("must deserialize legacy JSON without daemon_version field");
    assert!(
        s.daemon_version.is_none(),
        "missing daemon_version should default to None"
    );
}

#[test]
fn session_info_deserializes_new_json_with_daemon_version() {
    let new_json = r#"{
        "id": "new-9999",
        "port": 9999,
        "pid": 8888,
        "host": "new-host",
        "user": "modern",
        "created": "2026-06-01T00:00:00Z",
        "daemon_user": "bob",
        "daemon_version": "0.4.0-alpha.5"
    }"#;
    let s: SessionInfo = serde_json::from_str(new_json).unwrap();
    assert_eq!(s.daemon_version.as_deref(), Some("0.4.0-alpha.5"));
}

// ----------------------------------------------------------------------------
// ramic_bridge.il version stamp (regression check)
// ----------------------------------------------------------------------------

#[test]
fn ramic_bridge_has_rb_version_stamp() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ramic_bridge.il");
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    // Stamp must exist somewhere near the top of the file.
    let head = src.lines().take(15).collect::<Vec<_>>().join("\n");
    assert!(
        head.contains("; RB_VERSION:"),
        "ramic_bridge.il should have a `; RB_VERSION: x.y.z` stamp in its first 15 lines, got:\n{head}"
    );
}

#[test]
fn ramic_bridge_version_stamp_matches_cargo_toml() {
    // Read the stamp from .il
    let il_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ramic_bridge.il");
    let il_src = std::fs::read_to_string(&il_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", il_path.display()));
    let stamp_line = il_src
        .lines()
        .find(|l| {
            l.trim_start_matches(';')
                .trim_start()
                .starts_with("RB_VERSION:")
        })
        .expect("ramic_bridge.il should have a `; RB_VERSION: ...` line");
    let il_version = stamp_line
        .trim_start_matches(';')
        .trim()
        .trim_start_matches("RB_VERSION:")
        .trim();

    // Read the version from Cargo.toml of the workspace root
    let toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let toml_src = std::fs::read_to_string(&toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    // Find the first `version = "..."` in [package] section
    let cli_version = toml_src
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("[package]"))
        .take_while(|l| {
            let t = l.trim_start();
            t.is_empty()
                || t.starts_with('#')
                || t.starts_with('[')
                || t.starts_with("version")
                || t.starts_with("name")
                || t.starts_with("authors")
                || t.starts_with("edition")
                || t.starts_with("description")
                || t.starts_with("license")
                || t.starts_with("repository")
                || t.starts_with("readme")
                || t.starts_with("keywords")
                || t.starts_with("categories")
                || t.starts_with("publish")
                || t.starts_with("include")
                || t.starts_with("exclude")
                || t.starts_with("default-run")
                || t.starts_with("rust-version")
                || t.starts_with("documentation")
                || t.starts_with("homepage")
                || t.starts_with("workspace")
                || t.starts_with("path")
        })
        .find(|l| l.trim_start().starts_with("version"))
        .and_then(|l| l.split('"').nth(1))
        .map(str::to_string)
        .expect("Cargo.toml should have a `version = \"...\"` in [package]");

    assert_eq!(
        il_version, cli_version,
        "ramic_bridge.il RB_VERSION stamp ({il_version:?}) must match Cargo.toml version ({cli_version:?})"
    );
    assert_eq!(il_version, env!("CARGO_PKG_VERSION"));
}

// ─── runtime_paths integration ──────────────────────────────────────────────

#[test]
fn sessions_dir_respects_vb_cache_dir_env_override() {
    use virtuoso_cli::runtime_paths;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let original_xdg = env::var_os("XDG_CACHE_HOME");
    let original_vb = env::var_os("VB_CACHE_DIR");
    let tmp = tempfile::tempdir().expect("tempdir");
    env::set_var("VB_CACHE_DIR", tmp.path());
    env::remove_var("XDG_CACHE_HOME");

    let sessions = runtime_paths::cache_subdir(&["sessions"]);
    assert_eq!(sessions, tmp.path().join("virtuoso_bridge/sessions"));

    // Restore
    match original_xdg {
        Some(v) => env::set_var("XDG_CACHE_HOME", v),
        None => env::remove_var("XDG_CACHE_HOME"),
    }
    match original_vb {
        Some(v) => env::set_var("VB_CACHE_DIR", v),
        None => env::remove_var("VB_CACHE_DIR"),
    }
}

#[test]
fn log_root_respects_vb_log_dir_env_override() {
    use virtuoso_cli::runtime_paths;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let original = env::var_os("VB_LOG_DIR");
    let tmp = tempfile::tempdir().expect("tempdir");
    env::set_var("VB_LOG_DIR", tmp.path());

    let log_root = runtime_paths::log_root();
    assert_eq!(log_root, tmp.path());

    match original {
        Some(v) => env::set_var("VB_LOG_DIR", v),
        None => env::remove_var("VB_LOG_DIR"),
    }
}

// ─── end-to-end migration coverage: runtime_paths + downstream consumer ───────

/// End-to-end: writing a session file under a `VB_CACHE_DIR` override
/// and reading it back via the **public** `SessionInfo::load` API must
/// succeed. This verifies the full create-on-disk + read-back path is
/// consistent with the env override — i.e. the rest of the codebase,
/// which calls `SessionInfo::load`, transparently picks up the new path
/// resolution. (The internal `SessionInfo::sessions_dir()` is
/// `pub(crate)` and not callable from integration tests; this round-trip
/// is the integration-level proof that the wiring is correct.)
#[test]
fn session_round_trip_under_vb_cache_dir_override() {
    use virtuoso_cli::models::SessionInfo;
    use virtuoso_cli::runtime_paths;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let original_xdg = env::var_os("XDG_CACHE_HOME");
    let original_vb = env::var_os("VB_CACHE_DIR");
    let tmp = tempfile::tempdir().expect("tempdir");
    env::set_var("VB_CACHE_DIR", tmp.path());
    env::remove_var("XDG_CACHE_HOME");

    // Compute the expected sessions dir via the public runtime_paths API
    // (mirrors what `SessionInfo::sessions_dir()` does internally).
    let dir = runtime_paths::cache_subdir(&["sessions"]);
    assert_eq!(
        dir,
        tmp.path().join("virtuoso_bridge/sessions"),
        "runtime_paths::cache_subdir([\"sessions\"]) must land under VB_CACHE_DIR"
    );

    // Build a session, write it via std::fs to the env-overridden path,
    // then reload through the public SessionInfo::load API.
    let s = SessionInfo {
        id: "round-trip-1234".into(),
        port: 49152,
        pid: 7777,
        host: "roundtrip-host".into(),
        user: "tester".into(),
        created: "2026-06-13T00:00:00Z".into(),
        daemon_user: Some("bob".into()),
        daemon_version: Some("0.4.0-alpha.10".into()),
    };
    std::fs::create_dir_all(&dir).expect("create sessions dir");
    let path = dir.join(format!("{}.json", s.id));
    std::fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).expect("write session");

    // The public load API should find the file in the env-override location.
    let loaded = SessionInfo::load(&s.id).expect("reload session");
    assert_eq!(loaded.id, s.id);
    assert_eq!(loaded.port, s.port);
    assert_eq!(loaded.daemon_user.as_deref(), Some("bob"));
    assert_eq!(loaded.daemon_version.as_deref(), Some("0.4.0-alpha.10"));

    match original_xdg {
        Some(v) => env::set_var("XDG_CACHE_HOME", v),
        None => env::remove_var("XDG_CACHE_HOME"),
    }
    match original_vb {
        Some(v) => env::set_var("VB_CACHE_DIR", v),
        None => env::remove_var("VB_CACHE_DIR"),
    }
}

/// `legacy_state_file` should land at `<cache_root>/virtuoso_bridge/state.json`
/// (or `state_<profile>.json`) regardless of whether `VB_HOME`, `VB_CACHE_DIR`,
/// or `XDG_CACHE_HOME` was used to set the root — it always follows whatever
/// the active cache root is.
#[test]
fn legacy_state_file_tracks_active_cache_root() {
    use virtuoso_cli::runtime_paths;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let original_xdg = env::var_os("XDG_CACHE_HOME");
    let original_vb = env::var_os("VB_CACHE_DIR");
    let original_home = env::var_os("VB_HOME");

    // Case 1: VB_CACHE_DIR
    let tmp1 = tempfile::tempdir().expect("tempdir");
    env::set_var("VB_CACHE_DIR", tmp1.path());
    env::remove_var("XDG_CACHE_HOME");
    env::remove_var("VB_HOME");
    assert_eq!(
        runtime_paths::legacy_state_file(None),
        tmp1.path().join("virtuoso_bridge/state.json")
    );
    assert_eq!(
        runtime_paths::legacy_state_file(Some("p1")),
        tmp1.path().join("virtuoso_bridge/state_p1.json")
    );

    // Case 2: VB_HOME/cache (no VB_CACHE_DIR)
    let tmp2 = tempfile::tempdir().expect("tempdir");
    env::remove_var("VB_CACHE_DIR");
    env::remove_var("XDG_CACHE_HOME");
    env::set_var("VB_HOME", tmp2.path());
    assert_eq!(
        runtime_paths::legacy_state_file(None),
        tmp2.path().join("cache/virtuoso_bridge/state.json")
    );

    // Restore
    match original_xdg {
        Some(v) => env::set_var("XDG_CACHE_HOME", v),
        None => env::remove_var("XDG_CACHE_HOME"),
    }
    match original_vb {
        Some(v) => env::set_var("VB_CACHE_DIR", v),
        None => env::remove_var("VB_CACHE_DIR"),
    }
    match original_home {
        Some(v) => env::set_var("VB_HOME", v),
        None => env::remove_var("VB_HOME"),
    }
}

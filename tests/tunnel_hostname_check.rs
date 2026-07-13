//! Integration tests for the `vcli tunnel status` JSON output shape.
//!
//! These exercise the `HostnameCheck` wiring end-to-end through the
//! `vcli` binary as a subprocess. They don't require a live Virtuoso
//! daemon — the tests point at unused local ports and verify the JSON
//! shape is correct (daemon.responsive=false, no hostname_check field
//! when no port is reachable).
//!
//! Borrowed pattern: the existing `tunnel_status_*.rs` integration test
//! uses `CARGO_BIN_EXE_vcli` to spawn the binary, the same way
//! `daemon_user_guard.rs` does it.

use std::process::Command;

/// Path to the vcli binary. Cargo sets this env var at build time when
/// the integration test target declares a binary dependency.
fn vcli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vcli"))
}

/// Spawn `vcli tunnel status --format json` with a port that nothing
/// is listening on, then parse the JSON output.
///
/// `tunnel status` does not accept `--port` as a CLI flag — it reads
/// the port from the tunnel state file first, then falls back to
/// `VB_PORT`/`cfg.port`. So we set `VB_PORT` to an unused value.
fn tunnel_status_unreachable(port: u16) -> serde_json::Value {
    let output = vcli()
        .args(["tunnel", "status", "--format", "json"])
        .env("VB_PORT", port.to_string())
        .env("VB_REMOTE_HOST", "compute-eda-42")
        .env_remove("VB_SESSION") // ensure we don't pick up a real session
        .output()
        .expect("vcli tunnel status should run");

    assert!(
        output.status.success(),
        "vcli tunnel status failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).expect("vcli should produce valid JSON")
}

#[test]
fn tunnel_status_json_has_expected_top_level_keys() {
    let json = tunnel_status_unreachable(1);
    assert!(json.get("config").is_some(), "missing `config`: {json}");
    assert!(json.get("tunnel").is_some(), "missing `tunnel`: {json}");
    assert!(json.get("daemon").is_some(), "missing `daemon`: {json}");
}

#[test]
fn tunnel_status_daemon_unreachable_has_no_hostname_check() {
    // When the daemon port isn't reachable, the hostname_check path
    // is short-circuited — the field should be absent (or at least
    // not contain a populated configured/actual pair).
    let json = tunnel_status_unreachable(1);
    let daemon = &json["daemon"];
    assert_eq!(daemon["responsive"], false);

    // hostname_check is only populated when the daemon is reachable.
    // It MAY be present as `{"skipped": "..."}` or absent entirely;
    // both are valid.
    if let Some(hc) = daemon.get("hostname_check") {
        // If present, it must NOT contain a populated mismatch=true result.
        assert!(
            hc.get("mismatch").is_none() || hc.get("mismatch") == Some(&serde_json::json!(false)),
            "hostname_check should not be populated when daemon is unreachable, got: {hc}"
        );
    }
}

#[test]
fn tunnel_status_tunnel_section_reports_not_running() {
    // No tunnel state file should be present in this test (we just
    // pointed at an unused port), so the tunnel section should report
    // running=false.
    let json = tunnel_status_unreachable(1);
    assert_eq!(json["tunnel"]["running"], false);
}

#[test]
fn tunnel_status_config_section_reflects_env_overrides() {
    // The config section should reflect the env vars we passed.
    let json = tunnel_status_unreachable(65530);
    let config = &json["config"];
    assert_eq!(config["port"], 65530);
    // remote_host comes from VB_REMOTE_HOST
    assert_eq!(config["remote_host"], "compute-eda-42");
}

/// Verify the full status() path compiles and the JSON shape is stable
/// across a Table format run. We don't snapshot the full output (it
/// includes pids and timestamps that vary), but we verify the shape
/// has the right structure.
#[test]
fn tunnel_status_table_format_includes_daemon_section() {
    let output = vcli()
        .args(["tunnel", "status", "--format", "table"])
        .env("VB_PORT", "1")
        .env_remove("VB_SESSION")
        .output()
        .expect("vcli tunnel status --format table should run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The header is always present, even when nothing is reachable.
    assert!(
        stdout.contains("=== Virtuoso CLI Status ==="),
        "table output should have header, got: {stdout}"
    );
    assert!(
        stdout.contains("daemon:"),
        "table output should have daemon section, got: {stdout}"
    );
    assert!(
        stdout.contains("responsive: no"),
        "table output should report daemon not responsive, got: {stdout}"
    );
}

/// End-to-end test for the `HostnameCheck::run` early-return path:
/// when `configured` is `None` or empty, the check is skipped and the
/// status() output reflects `{"skipped": "local mode"}` only when the
/// daemon IS reachable. With no daemon reachable, the field is absent.
#[test]
fn tunnel_status_skips_hostname_check_when_no_remote_host() {
    let output = vcli()
        .args(["tunnel", "status", "--format", "json"])
        .env("VB_PORT", "1")
        .env_remove("VB_REMOTE_HOST") // simulate local mode
        .env_remove("VB_SESSION")
        .output()
        .expect("vcli tunnel status should run");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let daemon = &json["daemon"];
    assert_eq!(daemon["responsive"], false);
    // No daemon reachable → hostname_check field should be absent.
    assert!(daemon.get("hostname_check").is_none());
}

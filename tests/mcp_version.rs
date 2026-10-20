//! End-to-end check that the CLI and the MCP server report the same version.
//!
//! Regression: `vcli --version` reported 1.3.5 while the MCP `initialize`
//! handshake advertised `serverInfo.version = "0.3.18"` (a hardcoded literal
//! in `src/mcp/mod.rs`). Both now derive from `CARGO_PKG_VERSION`; this test
//! pins the observable contract — the two numbers must be identical.

use std::io::Write;
use std::process::{Command, Stdio};

fn vcli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vcli"))
}

/// Isolate the CLI from any developer/CI `targets.yaml` so the test never
/// depends on the ambient environment.
fn isolated_targets(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("targets.yaml");
    std::fs::write(
        &path,
        "active_target: prod\n\
         targets:\n  \
           prod:\n    \
             remote_host: eda-host\n    \
             port: 65535\n",
    )
    .expect("write targets.yaml");
    path
}

#[test]
fn cli_version_matches_mcp_server_info_version() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let targets = isolated_targets(tmp.path());

    // 1. The version the CLI prints.
    let out = vcli()
        .arg("--version")
        .env("VB_TARGETS_FILE", &targets)
        .output()
        .expect("vcli --version should spawn");
    let version_out = String::from_utf8_lossy(&out.stdout);
    let cli_version = version_out
        .trim()
        .rsplit(' ')
        .next()
        .expect("version in `vcli <version>` output")
        .to_string();
    assert!(
        !cli_version.is_empty(),
        "could not parse a version out of {version_out:?}"
    );

    // 2. The version the MCP server advertises in its initialize handshake.
    let mut child = vcli()
        .args(["mcp", "serve"])
        .env("VB_TARGETS_FILE", &targets)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("vcli mcp serve should spawn");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
            .expect("write initialize request");
        // Dropping stdin closes the pipe so the server loop exits.
    }
    let out = child.wait_with_output().expect("wait for vcli mcp serve");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_line = stdout.lines().next().expect("a JSON-RPC response");
    let response: serde_json::Value =
        serde_json::from_str(first_line).expect("initialize response must be JSON");
    let mcp_version = response["result"]["serverInfo"]["version"]
        .as_str()
        .expect("serverInfo.version must be a string")
        .to_string();

    assert_eq!(
        cli_version, mcp_version,
        "vcli --version ({cli_version}) and MCP serverInfo.version ({mcp_version}) must match"
    );
}

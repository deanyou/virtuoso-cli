//! End-to-end contract test for the CLI's error channel.
//!
//! Background: during a `verify-virtuoso-cli` proof run, an assertion that
//! only read **stdout** passed vacuously on a failing command — the CLI
//! writes the error document to **stderr** and leaves stdout empty, so a
//! stdout-only check sees "no output" and goes green.
//!
//! The CLI's behaviour here is the established Unix contract (results on
//! stdout, diagnostics on stderr, non-zero exit on failure) and is *not*
//! changed. This test pins that contract so the failure mode cannot silently
//! return:
//!
//!   * a failing command exits non-zero,
//!   * the error text lands on **stderr**,
//!   * stdout stays empty (so stdout-only assertions cannot pass vacuously),
//!   * a succeeding command does the mirror image (result on stdout, stderr
//!     empty, exit 0) — otherwise the failure assertions could pass for the
//!     wrong reason.

use std::process::Command;

fn vcli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vcli"))
}

/// A TCP port on localhost that nothing is listening on.
///
/// Bound and immediately released so the port is free: connecting to it is
/// refused straight away, which keeps the test fast and offline.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// Isolate the CLI from any developer/CI `targets.yaml`.
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
fn failing_command_reports_the_error_on_stderr_and_exits_nonzero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let targets = isolated_targets(tmp.path());
    let port = closed_port();

    let output = vcli()
        .args(["--format", "json", "cell", "open", "--lib", "demo", "--cell", "top"])
        .env("VB_TARGETS_FILE", &targets)
        .env("VB_HOST", "127.0.0.1")
        .env("VB_PORT", port.to_string())
        .env("VB_TIMEOUT", "5")
        .output()
        .expect("vcli cell open should spawn");

    // 1. Non-zero exit — a failure must never look like success.
    assert!(
        !output.status.success(),
        "failing command must exit non-zero, got success. stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 2. The error text must be on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.trim().is_empty(),
        "failing command must write an error to stderr, got nothing"
    );

    // 3. stdout must stay empty — this is the whole point of the contract.
    //    A harness that asserts on stdout alone would otherwise see an empty
    //    document and pass vacuously.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "failing command must not write a result to stdout, got: {stdout}"
    );

    // 4. The stderr payload is a structured error document.
    let error: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr must carry a JSON error document");
    assert!(
        error.get("error").and_then(|v| v.as_str()).is_some(),
        "error document must carry an `error` field, got: {error}"
    );
    assert!(
        error
            .get("message")
            .and_then(|v| v.as_str())
            .is_some_and(|m| !m.is_empty()),
        "error document must carry a non-empty `message`, got: {error}"
    );
}

#[test]
fn succeeding_command_writes_the_result_to_stdout_and_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let targets = isolated_targets(tmp.path());

    let output = vcli()
        .args(["--format", "json", "schema", "--all"])
        .env("VB_TARGETS_FILE", &targets)
        .output()
        .expect("vcli schema --all should spawn");

    // Mirror image of the failure contract.
    assert!(
        output.status.success(),
        "succeeding command must exit 0, got {:?}. stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "succeeding command must write its result to stdout"
    );
    let result: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must carry a JSON result document");
    assert!(
        result.get("commands").is_some(),
        "result document must carry the command schema, got: {result}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "succeeding command must not write to stderr, got: {stderr}"
    );
}

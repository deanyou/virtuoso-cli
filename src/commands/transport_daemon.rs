//! Internal entry point for the native transport daemon (`__transport-daemon`).
//!
//! Compiled only with the `native-ssh` feature, because the design requires the
//! subcommand to be **absent** from builds without it — not present-and-failing.
//!
//! It is spawned by `vcli` itself, never typed by a user. The daemon proper
//! ships with the `russh` dependency; until that lands this refuses to serve
//! through the ordinary CLI error path (non-zero exit, structured error) rather
//! than exiting successfully and leaving the parent waiting on an IPC endpoint
//! nobody answers.

use crate::error::{Result, VirtuosoError};
use serde_json::Value;

pub fn run(_ipc_endpoint: &str, _token_path: &str, _daemon_nonce: &str) -> Result<Value> {
    Err(VirtuosoError::Config(
        "native transport daemon is not available in this build: the native client \
         (and its `russh` dependency) has not landed yet"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_serve_with_a_structured_error() {
        let err = run("/run/vb.sock", "/run/vb.token", "n0nce").unwrap_err();
        match err {
            VirtuosoError::Config(m) => {
                assert!(m.contains("russh"), "message should name the blocker: {m}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[test]
    fn refuses_regardless_of_arguments() {
        // The parent cannot make it serve by passing different arguments.
        assert!(run("", "", "").is_err());
    }
}

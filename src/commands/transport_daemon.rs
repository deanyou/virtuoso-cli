//! Internal entry point for the native transport daemon (`__transport-daemon`).
//!
//! Compiled only with the `native-ssh` feature, because the design requires
//! the subcommand to be **absent** from builds without it — not
//! present-and-failing.
//!
//! It is spawned by `vcli` itself, never typed by a user. This module wires
//! the `__transport-daemon` hidden subcommand to the production IPC server in
//! [`crate::transport::ipc::server`]. Step 6 of the design (channel pool,
//! reconnect, lifecycle) is consumed here: the daemon owns an
//! [`EndpointPool`], resolves one [`EndpointKey`] from the shared config, and
//! serves every request through [`server::run_with_pool`] — so all four roles
//! that name the same host share one transport, and a connection-level
//! failure evicts the pooled connection and reconnects on the next request.
//!
//! [Step 2]: docs/superpowers/specs/2026-08-29-native-remote-transport-design.md
//!
//! [NativeTransport]: crate::transport::native::NativeTransport

use crate::error::{Result, VirtuosoError};
use serde_json::Value;

#[cfg(all(unix, feature = "native-ssh"))]
use std::path::Path;
#[cfg(all(unix, feature = "native-ssh"))]
use std::sync::Arc;

#[cfg(all(unix, feature = "native-ssh"))]
use crate::transport::contract::RemoteTransport;
#[cfg(all(unix, feature = "native-ssh"))]
use crate::transport::contract::TransportError;
#[cfg(all(unix, feature = "native-ssh"))]
use crate::transport::pool::{EndpointKey, EndpointPool};
#[cfg(all(unix, feature = "native-ssh"))]
use crate::transport::scheduler::SchedulerLimits;

/// The "this build has no daemon" answer.
///
/// Compiled only when `native-ssh` is **off**: with the feature on, the real
/// [`run_with`] takes over and this placeholder would be dead code. The
/// subcommand itself is gated out of the CLI in feature-off builds, so this
/// exists for the tests below and for library callers, which must get a
/// structured answer rather than a working-looking daemon.
///
/// Arguments are the ones `tunnel start` passes verbatim: `ipc_endpoint` is
/// the Unix domain socket path (mode `0600` after bind), `token_path` is the
/// file holding the auth token (mode `0600`), and `daemon_nonce` is the
/// per-instance nonce the parent wrote to the state file before spawning.
#[cfg(not(feature = "native-ssh"))]
pub fn run(_ipc_endpoint: &str, _token_path: &str, _daemon_nonce: &str) -> Result<Value> {
    Err(VirtuosoError::Config(
        "native transport daemon is not available in this build: the daemon subcommand \
         is gated on the `native-ssh` feature"
            .into(),
    ))
}

/// Production body of the daemon. Only compiled when `native-ssh` is on, so
/// the subcommand stays absent from feature-stripped builds (the design's
/// hard requirement).
#[cfg(all(unix, feature = "native-ssh"))]
pub fn run_with(ipc_endpoint: &str, token_path: &str, daemon_nonce: &str) -> Result<Value> {
    use crate::transport::backend::open_transport_for_daemon;
    use crate::transport::ipc::server;
    use crate::transport::lifecycle::ShutdownCoordinator;

    let config = crate::config::Config::from_env()?;

    // The daemon constructs its SSH backend directly via `open_transport_for_daemon`,
    // NOT `open_transport`. The latter routes native traffic over IPC to a
    // running daemon — using it here would deadlock on first startup (the
    // daemon can't connect to itself before it starts listening).
    //
    // Configuration errors surface here, before the socket is bound, with the
    // same fail-loudly semantics as the pre-pool daemon: a backend that cannot
    // be constructed (missing `VB_REMOTE_HOST`, missing `VB_SSH_KEY`, invalid
    // capacity) must fail startup, not the first request. The pooled daemon
    // then re-runs this factory only when a connection needs (re)building.
    open_transport_for_daemon(&config).map_err(transport_to_virtuoso)?;

    // One endpoint key for the whole daemon: the daemon serves the config's
    // single remote host, so all roles collapse onto one pooled connection.
    let host = config.remote_host.as_deref().unwrap_or("");
    let key = EndpointKey::from_config(&config, host);
    let limits = SchedulerLimits::from_config(&config)?;
    let pool = Arc::new(EndpointPool::new());
    let factory: Arc<dyn Fn() -> Result<Arc<dyn RemoteTransport>, TransportError> + Send + Sync> =
        Arc::new(move || open_transport_for_daemon(&config));

    let token = std::fs::read_to_string(Path::new(token_path)).map_err(|e| {
        VirtuosoError::Io(std::io::Error::other(format!(
            "read auth token from {token_path}: {e}"
        )))
    })?;
    // Trim a trailing newline that `cargo run -- ...` style launchers often
    // add — the token file is single-line.
    let token = token.trim().to_string();

    // Grace period for `Operation::Shutdown` (VB_TRANSPORT_SHUTDOWN_GRACE).
    let shutdown = ShutdownCoordinator::from_config(&config);

    let socket = Path::new(ipc_endpoint);
    server::run_with_pool(
        socket,
        pool,
        key,
        limits,
        factory,
        &token,
        daemon_nonce,
        shutdown,
    )
    .map_err(|e| {
        VirtuosoError::Io(std::io::Error::other(format!(
            "transport daemon exited unexpectedly: {e}"
        )))
    })?;
    Ok(Value::Null)
}

/// Non-Unix counterpart of [`run_with`].
///
/// [`crate::transport::ipc::server`] is Unix-only — it binds a Unix domain
/// socket — so a `native-ssh` build elsewhere has no daemon to run. `main.rs`
/// still dispatches the subcommand whenever the feature is on, so this has to
/// answer with a structured Config error rather than be absent: the same
/// contract `run` upholds for feature-off builds.
#[cfg(all(feature = "native-ssh", not(unix)))]
pub fn run_with(_ipc_endpoint: &str, _token_path: &str, _daemon_nonce: &str) -> Result<Value> {
    Err(VirtuosoError::Config(
        "native transport daemon requires a Unix domain socket: no daemon is \
         available on this platform"
            .into(),
    ))
}

/// Map a transport error onto the daemon's CLI error path. Config and
/// unsupported-backend failures are usage errors (exit code per the design);
/// every other failure stays a general error so the parent process learns
/// the daemon failed to start without parsing message text.
#[cfg(all(unix, feature = "native-ssh"))]
fn transport_to_virtuoso(e: TransportError) -> VirtuosoError {
    use crate::transport::contract::TransportError as T;
    match e {
        T::Configuration(m) => VirtuosoError::Config(format!("daemon transport: {m}")),
        T::UnsupportedBackend => VirtuosoError::Config(
            "native transport daemon requires the `native-ssh` feature".into(),
        ),
        other => VirtuosoError::Io(std::io::Error::other(format!("daemon transport: {other}"))),
    }
}

#[cfg(all(test, not(feature = "native-ssh")))]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_serve_when_feature_is_disabled() {
        // When the `native-ssh` feature is off, `run` is the public entry
        // point and must report a structured Config error that names the
        // missing feature. This is the gate that prevents the placeholder
        // from ever being silently mistaken for a working daemon.
        let err = run("/run/vb.sock", "/run/vb.token", "n0nce").unwrap_err();
        match err {
            VirtuosoError::Config(m) => {
                assert!(
                    m.contains("native-ssh"),
                    "message should name the gating feature: {m}"
                );
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

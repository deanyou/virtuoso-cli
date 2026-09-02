//! Two-tier liveness check for a recorded daemon (stop / crash recovery).
//!
//! Implements the design's [Stop and crash recovery] contract: termination and
//! stale-state removal require proof, in this order.
//!
//! **Tier 1 — the daemon answers.** An IPC nonce challenge proves the process on
//! the other end is the recorded daemon. No platform-specific code.
//!
//! **Tier 2 — the daemon does not answer.** A hung daemon cannot complete a
//! challenge, and that is exactly when an operator needs to force termination.
//! Falling back to the PID alone would reintroduce the risk the nonce exists to
//! remove, so Tier 2 requires an OS identity match on all three recorded
//! attributes (executable path, PID, start identity) before anything is
//! signalled. Where identity cannot be established, this module refuses.
//!
//! Startup removes stale state only after proving the recorded daemon is no
//! longer valid, using the same two tiers in the same order.
//!
//! This module is additive: nothing calls it yet. Wiring it into
//! `tunnel stop`/`status` (which currently trusts the PID on non-Unix) is a
//! separate increment because it changes existing behaviour.
//!
//! [Stop and crash recovery]: ../../../docs/superpowers/specs/2026-08-29-native-remote-transport-design.md

// Consumed when the daemon lifecycle lands; mirrors `contract.rs`.
#![allow(dead_code)]

use std::path::PathBuf;

use crate::models::TunnelState;
use crate::transport::identity::{self, ProcessIdentity, Refusal};

/// Tier-1 liveness probe: connect to the recorded IPC endpoint, complete the
/// `Hello` handshake, send `Operation::Challenge`, and confirm the daemon
/// answered with the nonce the state file recorded.
///
/// Returns `true` only when every step succeeds *and* the daemon's echoed
/// nonce equals `state.daemon_nonce`. Any failure — missing fields, refused
/// connection, token mismatch, nonce mismatch — returns `false`, which lets
/// `assess` fall through to Tier 2 (the OS identity check) instead of
/// claiming the daemon is alive when it may not be.
///
/// Gated to the native backend on Unix: an OpenSSH tunnel has no daemon to
/// challenge, and a Windows native build will reach this through the named
/// pipe path in a later increment. The fallback to Tier 2 is the same on
/// every platform — refuse, do not guess.
#[cfg(all(unix, feature = "native-ssh"))]
pub fn challenge_via_ipc(state: &TunnelState) -> bool {
    use crate::transport::ipc::daemon::NativeTransportClient;

    let endpoint = match state.ipc_endpoint.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let token_path = match state.token_path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let expected_nonce = match state.daemon_nonce.as_deref() {
        Some(n) if !n.is_empty() => n,
        _ => return false,
    };

    // Read the auth token the daemon was given at startup. Trimming mirrors
    // `commands::transport_daemon::run_with` on the daemon side.
    let token = match std::fs::read_to_string(token_path) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return false,
    };
    let profile = state.profile.as_deref().unwrap_or("");

    let client =
        match NativeTransportClient::connect(std::path::Path::new(endpoint), profile, &token) {
            Ok(c) => c,
            Err(_) => return false,
        };

    match client.challenge() {
        Ok(ack) => ack.daemon_nonce == expected_nonce,
        Err(_) => false,
    }
}

/// Ask the recorded daemon to shut down cooperatively over IPC.
///
/// Reached from `tunnel stop` only after Tier 1 proved the daemon is ours —
/// and the proof is repeated on *this* connection: the nonce challenge runs
/// first, and the shutdown request is sent on the same authenticated channel
/// only when the echoed nonce matches. The stop decision may have been
/// reached through an earlier connection, and a Unix socket path is
/// re-bindable, so trusting that decision here would widen the proof gap.
///
/// Returns `true` when the daemon acked — and because the daemon fires its
/// shutdown token *before* writing the ack (step 6c-3), observing the ack
/// proves admission has already stopped. The daemon finishes in-flight work
/// within `VB_TRANSPORT_SHUTDOWN_GRACE` and exits on its own; the caller
/// bounds its wait and falls back to signalling otherwise.
///
/// Gated identically to [`challenge_via_ipc`].
#[cfg(all(unix, feature = "native-ssh"))]
pub fn shutdown_via_ipc(state: &TunnelState) -> bool {
    use crate::transport::ipc::daemon::NativeTransportClient;

    let endpoint = match state.ipc_endpoint.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let token_path = match state.token_path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let expected_nonce = match state.daemon_nonce.as_deref() {
        Some(n) if !n.is_empty() => n,
        _ => return false,
    };
    // Read the auth token the daemon was given at startup. Trimming mirrors
    // `commands::transport_daemon::run_with` on the daemon side.
    let token = match std::fs::read_to_string(token_path) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return false,
    };
    let profile = state.profile.as_deref().unwrap_or("");

    let client =
        match NativeTransportClient::connect(std::path::Path::new(endpoint), profile, &token) {
            Ok(c) => c,
            Err(_) => return false,
        };

    match client.challenge() {
        Ok(ack) if ack.daemon_nonce == expected_nonce => {}
        _ => return false,
    }

    client.request_shutdown().is_ok()
}

/// What the recorded state still describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Tier 1: the daemon answered the nonce challenge correctly.
    Alive,
    /// Tier 1 failed, but Tier 2 proved the live process is the recorded one —
    /// a forced signal is authorized.
    UnresponsiveButIdentified,
    /// Proven gone (process absent, or the PID now belongs to something else).
    /// Safe to clear the state file.
    Stale,
    /// Identity could not be established. Never signal, never auto-clear.
    Unverifiable(String),
}

impl Verdict {
    /// Whether a forced termination is authorized.
    pub fn may_signal(&self) -> bool {
        matches!(self, Verdict::UnresponsiveButIdentified)
    }

    /// Whether the state file may be removed as stale.
    ///
    /// Deliberately false for [`Verdict::Unverifiable`]: the design requires
    /// proof before discarding state, and an unreadable identity is not proof.
    pub fn may_clear_state(&self) -> bool {
        matches!(self, Verdict::Stale)
    }
}

/// Rebuild the OS identity triple the state file records, if it has one.
///
/// `None` for v1 files and for OpenSSH-written state (no identity recorded),
/// which is why callers must treat "no identity" as unverifiable rather than
/// as proof that the daemon is gone.
pub fn recorded_identity(state: &TunnelState) -> Option<ProcessIdentity> {
    let start_identity = state.start_identity?;
    let executable_path = state.executable_path.as_deref()?;
    Some(ProcessIdentity {
        executable_path: PathBuf::from(executable_path),
        pid: state.pid,
        start_identity,
    })
}

/// Record a daemon's OS identity into the state file.
///
/// Call this when the daemon starts, so a later Tier 2 check has something to
/// match against.
pub fn record_identity(state: &mut TunnelState, identity: &ProcessIdentity) {
    state.executable_path = Some(identity.executable_path.to_string_lossy().into_owned());
    state.pid = identity.pid;
    state.start_identity = Some(identity.start_identity);
}

/// Assess a recorded daemon, Tier 1 then Tier 2.
///
/// `challenge` performs Tier 1: given the recorded nonce, return `true` only if
/// the daemon answered correctly. It is injected rather than performed here so
/// this decision is testable without IPC.
pub fn assess(state: &TunnelState, challenge: impl FnOnce(&str) -> bool) -> Verdict {
    // Tier 1.
    if let Some(nonce) = state.daemon_nonce.as_deref() {
        if challenge(nonce) {
            return Verdict::Alive;
        }
    }

    // Tier 2: the daemon is not answering, so require an OS identity match.
    let Some(recorded) = recorded_identity(state) else {
        return Verdict::Unverifiable(
            "state records no process identity (v1 or OpenSSH state)".into(),
        );
    };

    match identity::authorize_signal(&recorded) {
        Ok(()) => Verdict::UnresponsiveButIdentified,
        // The process is gone, or the PID now belongs to a different one —
        // either way the recorded daemon no longer exists.
        Err(Refusal::ProcessGone(_)) | Err(Refusal::Mismatch { .. }) => Verdict::Stale,
        Err(Refusal::Unverifiable(e)) => Verdict::Unverifiable(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> TunnelState {
        TunnelState {
            version: 2,
            port: 40567,
            pid: 999_999,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: None,
            backend: Some("native".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: None,
            attached_remote_port: None,
            attached_session_id: None,
        }
    }

    /// A state describing *this* running test process, which stands in for a
    /// live daemon. Only used where the platform can read an identity.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn state_for_current_process() -> TunnelState {
        let me = ProcessIdentity::current().expect("identity of the test process");
        let mut s = state();
        record_identity(&mut s, &me);
        s
    }

    #[test]
    fn tier1_answer_short_circuits_tier2() {
        // Even with a nonexistent pid, a correct challenge means Alive and
        // Tier 2 is never consulted.
        let mut s = state();
        s.daemon_nonce = Some("nonce-abc".into());
        s.pid = 999_999;
        let v = assess(&s, |nonce| nonce == "nonce-abc");
        assert_eq!(v, Verdict::Alive);
    }

    #[test]
    fn tier1_wrong_nonce_falls_through_to_tier2() {
        let mut s = state();
        s.daemon_nonce = Some("nonce-abc".into());
        // Wrong nonce → not Alive; with no recorded identity it is unverifiable.
        let v = assess(&s, |nonce| nonce == "different");
        assert!(matches!(v, Verdict::Unverifiable(_)), "got {v:?}");
    }

    #[test]
    fn no_recorded_identity_is_unverifiable_not_stale() {
        // The important negative: absence of identity is not proof of absence.
        let v = assess(&state(), |_| false);
        assert!(matches!(v, Verdict::Unverifiable(_)));
        assert!(!v.may_clear_state());
        assert!(!v.may_signal());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn live_matching_process_is_unresponsive_but_identified() {
        let s = state_for_current_process();
        let v = assess(&s, |_| false);
        assert_eq!(v, Verdict::UnresponsiveButIdentified);
        assert!(v.may_signal());
        // Not Alive, so it must not be treated as answering.
        assert!(!matches!(v, Verdict::Alive));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn dead_process_is_stale() {
        let mut s = state_for_current_process();
        s.pid = 999_999; // nothing is running here
        let v = assess(&s, |_| false);
        assert_eq!(v, Verdict::Stale);
        assert!(v.may_clear_state());
        assert!(!v.may_signal());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn pid_reuse_is_stale() {
        // Same pid as a live process, but a start identity that does not match:
        // the recorded daemon is gone even though the pid is in use.
        let s = state_for_current_process();
        let mut reused = s.clone();
        reused.start_identity = Some(s.start_identity.unwrap().wrapping_add(1));
        let v = assess(&reused, |_| false);
        assert_eq!(v, Verdict::Stale);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn different_executable_is_stale() {
        let mut s = state_for_current_process();
        s.executable_path = Some("/definitely/not/this/binary".into());
        let v = assess(&s, |_| false);
        assert_eq!(v, Verdict::Stale);
    }

    #[test]
    fn record_identity_round_trips() {
        let mut s = state();
        let id = ProcessIdentity {
            executable_path: PathBuf::from("/usr/bin/vcli"),
            pid: 4242,
            start_identity: 1767225600,
        };
        record_identity(&mut s, &id);
        assert_eq!(recorded_identity(&s), Some(id));
    }

    #[test]
    fn recorded_identity_is_none_when_any_attribute_is_missing() {
        let mut s = state();
        s.pid = 42;
        s.executable_path = Some("/usr/bin/vcli".into());
        assert_eq!(recorded_identity(&s), None, "start_identity missing");

        s.start_identity = Some(7);
        s.executable_path = None;
        assert_eq!(recorded_identity(&s), None, "executable_path missing");
    }

    #[test]
    fn verdict_predicates_are_exclusive() {
        // Only Stale may clear state; only UnresponsiveButIdentified may signal.
        for v in [
            Verdict::Alive,
            Verdict::UnresponsiveButIdentified,
            Verdict::Stale,
            Verdict::Unverifiable("x".into()),
        ] {
            let signals = v.may_signal();
            let clears = v.may_clear_state();
            assert!(!(signals && clears), "{v:?} must not allow both");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier-1 challenge tests: only meaningful on Unix with the `native-ssh`
// feature, because the helper talks to a Unix domain socket owned by the
// daemon subcommand. On every other platform the helper is absent and Tier 1
// is a structural no-op that falls through to Tier 2.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(all(unix, feature = "native-ssh"))]
#[cfg(test)]
mod challenge_tests {
    use super::*;
    use crate::transport::ipc::server::{self, ChallengeAck};
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Spawn a one-shot daemon that accepts exactly one connection, completes
    /// the Hello handshake with `server_nonce`, and answers one `Challenge`
    /// request with the same nonce. Returns the socket path and a token file
    /// path the parent can hand to `challenge_via_ipc`.
    ///
    /// The socket lives under `/tmp` (not `std::env::temp_dir()`, which on
    /// macOS resolves under `/var/folders/...` and easily exceeds the 104-byte
    /// `sun_path` limit the kernel enforces on `bind`). The token file uses
    /// the same rule.
    fn spawn_challenge_daemon(
        server_nonce: &str,
        token: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let socket = std::path::PathBuf::from(format!("/tmp/vcli-c-{tag}.sock"));
        let token_path = std::path::PathBuf::from(format!("/tmp/vcli-c-{tag}.tok"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
        let listener = UnixListener::bind(&socket).expect("bind");
        listener.set_nonblocking(false).expect("blocking listener");

        // Write the token to a single-line file with mode 0600.
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&token_path)
                .expect("create token");
            f.write_all(token.as_bytes()).expect("write token");
        }

        // Serve one connection. The transport is irrelevant — Challenge
        // doesn't touch it — but the server API requires one.
        let listener_for_thread = listener.try_clone().expect("clone listener");
        let token_owned = token.to_string();
        let nonce_owned = server_nonce.to_string();
        thread::spawn(move || {
            if let Ok((stream, _)) = listener_for_thread.accept() {
                let transport: Arc<dyn crate::transport::contract::RemoteTransport> =
                    Arc::new(crate::transport::contract::test_support::FakeTransport::ok());
                server::serve_one(stream, transport, &token_owned, &nonce_owned);
            }
        });

        (socket, token_path)
    }

    fn state_with(nonce: &str, endpoint: Option<&str>, token_path: Option<&str>) -> TunnelState {
        TunnelState {
            version: 2,
            port: 40567,
            pid: 999_999,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: None,
            backend: Some("native".into()),
            daemon_nonce: Some(nonce.into()),
            executable_path: None,
            start_identity: None,
            ipc_endpoint: endpoint.map(String::from),
            token_path: token_path.map(String::from),
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: None,
            attached_remote_port: None,
            attached_session_id: None,
        }
    }

    #[test]
    fn challenge_answers_true_when_daemon_returns_recorded_nonce() {
        let (socket, token_path) = spawn_challenge_daemon("recorded-nonce", "secret-token");
        let st = state_with(
            "recorded-nonce",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        assert!(challenge_via_ipc(&st));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    #[test]
    fn challenge_answers_false_on_nonce_mismatch() {
        // The daemon answers its *own* nonce; the state file recorded
        // something else, so this must fail.
        let (socket, token_path) = spawn_challenge_daemon("real-nonce", "secret-token");
        let st = state_with(
            "WRONG-nonce",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        assert!(!challenge_via_ipc(&st));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    #[test]
    fn challenge_answers_false_when_auth_token_is_wrong() {
        let (socket, token_path) = spawn_challenge_daemon("recorded-nonce", "good-token");
        // Rewrite the token file with a different token — the daemon will
        // reject the Hello, and the challenge returns false.
        std::fs::write(&token_path, "different-token\n").unwrap();
        let st = state_with(
            "recorded-nonce",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        assert!(!challenge_via_ipc(&st));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    #[test]
    fn challenge_returns_false_when_recorded_endpoint_is_missing() {
        let st = state_with("recorded-nonce", None, None);
        assert!(!challenge_via_ipc(&st));
    }

    #[test]
    fn challenge_returns_false_when_recorded_nonce_is_missing() {
        let (_socket, token_path) = spawn_challenge_daemon("x", "secret-token");
        let st = state_with(
            "",
            Some("/tmp/whatever.sock"),
            Some(&token_path.to_string_lossy()),
        );
        assert!(!challenge_via_ipc(&st));
        let _ = std::fs::remove_file(&token_path);
    }

    #[test]
    fn challenge_returns_false_when_endpoint_does_not_exist() {
        // A path that is syntactically valid but no daemon is bound there.
        let st = state_with("x", Some("/tmp/vcli-does-not-exist-1234567890.sock"), None);
        assert!(!challenge_via_ipc(&st));
    }

    #[test]
    fn challenge_endpoint_uses_a_short_deadline_so_stuck_daemon_does_not_hang_stop() {
        // A daemon that never replies must not block `tunnel stop` longer
        // than the IPC challenge deadline. The deadline is encoded in
        // `NativeTransportClient::challenge` and is bounded to a couple of
        // seconds; this test guards against accidentally removing it.
        //
        // We exercise it by pointing at a socket whose accept loop never
        // writes back: a listener that accepts and then idles.
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let socket = std::path::PathBuf::from(format!("/tmp/vcli-ch-{tag}.sock"));
        let token_path = std::path::PathBuf::from(format!("/tmp/vcli-ch-{tag}.tok"));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("bind");
        let listener_for_thread = listener.try_clone().expect("clone listener");
        thread::spawn(move || {
            // Accept and hold the stream open without responding.
            if let Ok((stream, _)) = listener_for_thread.accept() {
                std::thread::sleep(Duration::from_secs(30));
                drop(stream);
            }
        });
        // Drop our listener so the accept loop terminates.
        drop(listener);

        std::fs::write(&token_path, "t").unwrap();

        let st = state_with(
            "x",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        let start = std::time::Instant::now();
        assert!(!challenge_via_ipc(&st));
        // Generous bound — a stuck daemon must not block stop beyond a few
        // seconds; we allow up to 10s for slow CI, far below the 30s sleep.
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "challenge took too long: {:?}",
            start.elapsed()
        );

        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    /// Cooperative shutdown acks when the daemon echoes the recorded nonce —
    /// the same Tier-1 proof `challenge_via_ipc` requires, then the Shutdown
    /// request on the same channel.
    #[test]
    fn shutdown_acks_when_daemon_matches_the_recorded_nonce() {
        let (socket, token_path) = spawn_challenge_daemon("recorded-nonce", "secret-token");
        let st = state_with(
            "recorded-nonce",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        assert!(shutdown_via_ipc(&st));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    /// A nonce mismatch must never reach the shutdown request: the daemon is
    /// not proven ours, so the call refuses before sending `Shutdown`.
    #[test]
    fn shutdown_refuses_a_nonce_mismatch() {
        let (socket, token_path) = spawn_challenge_daemon("real-nonce", "secret-token");
        let st = state_with(
            "WRONG-nonce",
            Some(socket.to_str().unwrap()),
            Some(&token_path.to_string_lossy()),
        );
        assert!(!shutdown_via_ipc(&st));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&token_path);
    }

    // Silence the unused-import lint for `ChallengeAck` — re-exported so
    // downstream callers can name the payload type without reaching into
    // `server::*`.
    #[allow(dead_code)]
    fn _ack_alias(_: ChallengeAck) {}
}

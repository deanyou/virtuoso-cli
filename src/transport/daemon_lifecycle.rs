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

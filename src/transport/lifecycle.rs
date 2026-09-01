//! Lifecycle primitives for the native transport: reconnect policy,
//! circuit breaker, cancellation, and shutdown coordination.
//!
//! These are the *rules* of step 6, kept free of I/O so they can be tested
//! exhaustively. The native transport (and, later, `tunnel reconnect`) apply
//! them; nothing here opens a socket.
//!
//! The governing invariants from the design document:
//!
//! - An operation already sent to the server is **never replayed**. A lost
//!   command returns `OutcomeUnknown`; a lost transfer returns
//!   `TransferInterrupted`. Reconnection re-establishes the path; it does not
//!   re-issue work.
//! - Host-key changes, rejected authentication, unsupported security policy,
//!   and proxy policy failures are **permanent until user action**.
//! - Repeated transient failures eventually open a circuit breaker and set
//!   the endpoint to `Degraded`; `vcli tunnel reconnect` explicitly resets it.
//! - `tunnel stop` stops admission, grants running work a bounded grace
//!   period, then cancels the remainder.

// Consumed when the native transport wires these in (step 6b/6c); mirrors
// `contract.rs` and `scheduler.rs`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::transport::contract::TransportError;

/// How a failure should influence the connection lifecycle.
///
/// The distinction is the design's, not a heuristic: the four families the
/// design names "permanent until user action" map to [`FailureClass::Permanent`],
/// network-path failures map to [`FailureClass::Transient`], and everything
/// that belongs to a single request rather than the connection maps to
/// [`FailureClass::RequestLevel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Network-path failure. The connection may be re-established with
    /// backoff; the failed operation itself is never replayed.
    Transient,
    /// Permanent until user action. Never retried, never fed to the circuit
    /// breaker: hammering the server after an auth rejection would only make
    /// things worse, and a host-key change is a security event, not noise.
    Permanent,
    /// The connection is healthy; the failure belongs to one request (queue
    /// timeout, remote exit, cancellation, unknown outcome). Not a reconnect
    /// trigger and not replayable.
    RequestLevel,
}

impl FailureClass {
    /// Classify a transport error for lifecycle purposes.
    pub fn of(err: &TransportError) -> Self {
        match err {
            // Configuration and capability errors are user action territory.
            TransportError::Configuration(_)
            | TransportError::UnsupportedOperation(_)
            | TransportError::UnsupportedBackend
            | TransportError::ProtocolMismatch { .. }
            | TransportError::RestartRequired(_) => FailureClass::Permanent,

            // Security and policy events: the design is explicit that these
            // are permanent until the user intervenes.
            TransportError::HostKeyUnknown { .. }
            | TransportError::HostKeyChanged { .. }
            | TransportError::HostKeyPolicyUnsupported(_) => FailureClass::Permanent,

            // Credentials: rejection will not improve with retries, and
            // prompting for interaction is user action by definition.
            TransportError::AuthenticationFailed(_) | TransportError::InteractionRequired => {
                FailureClass::Permanent
            }

            // Proxy policy failures are permanent per the design; the SOCKS
            // route itself is a configured path, not a flaky network.
            TransportError::ProxyFailed(_) => FailureClass::Permanent,

            // Network-path failures worth re-establishing the connection for.
            TransportError::ConnectionFailed(_)
            | TransportError::DaemonUnavailable
            | TransportError::JumpFailed(_)
            | TransportError::LocalIo(_) => FailureClass::Transient,

            // Everything else belongs to one request. The connection may be
            // perfectly fine; classifying these as connection failures would
            // tear down a healthy path because a remote command exited 1.
            TransportError::QueueTimeout { .. }
            | TransportError::ExecutionTimeout { .. }
            | TransportError::RemoteExit { .. }
            | TransportError::OutcomeUnknown { .. }
            | TransportError::TransferInterrupted { .. }
            | TransportError::IntegrityMismatch { .. }
            | TransportError::RemoteIo(_)
            | TransportError::Cancelled { .. } => FailureClass::RequestLevel,
        }
    }
}

/// Exponential backoff with jitter for reconnection attempts.
///
/// `delay_for_attempt(n)` grows geometrically from [`Self::base`] and is
/// capped at `max_delay` (the design's `VB_SSH_RECONNECT_MAX_DELAY`, 30 s by
/// default). Jitter spreads simultaneous clients so a server that blipped is
/// not hit by a thundering herd at exactly the same instant.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Total attempts before giving up and degrading the endpoint
    /// (`VB_SSH_RECONNECT_MAX_ATTEMPTS`, default 8).
    pub max_attempts: u32,
    /// Upper bound on a single wait (`VB_SSH_RECONNECT_MAX_DELAY`, default 30 s).
    pub max_delay: Duration,
    /// First wait, before any growth. Not env-configurable in the design;
    /// one second keeps the first retry snappy without hot-looping.
    pub base: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            max_delay: Duration::from_secs(30),
            base: Duration::from_secs(1),
        }
    }
}

impl ReconnectPolicy {
    /// Whether `attempt` (1-based) may still be made.
    pub fn may_retry(&self, attempt: u32) -> bool {
        attempt <= self.max_attempts
    }

    /// Wait before `attempt` (1-based), without jitter. Attempt 1 waits the
    /// base delay; each subsequent attempt doubles, capped at `max_delay`.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(30);
        let raw = self.base.saturating_mul(1u32 << exp);
        raw.min(self.max_delay)
    }

    /// Wait before `attempt`, with jitter derived from `seed`.
    ///
    /// The jitter is ±25% of the base delay so that tests are deterministic
    /// given a seed, while callers feed entropy (e.g. from the request ID or
    /// the clock) in production.
    pub fn jittered_delay(&self, attempt: u32, seed: u64) -> Duration {
        let base = self.delay_for_attempt(attempt);
        // Spread the attempt across ±25% of the base delay. At the cap the
        // jitter floor shrinks so `max_delay` is never exceeded.
        let spread = self.base.as_millis() as u64 / 4;
        let offset = (splitmix64(seed) % (2 * spread + 1)) as i64 - spread as i64;
        let millis = (base.as_millis() as i64 + offset).max(0) as u64;
        Duration::from_millis(millis.min(self.max_delay.as_millis() as u64))
    }
}

/// Small deterministic hash so jitter is reproducible in tests without
/// pulling in a RNG crate.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Circuit breaker over consecutive transient failures.
///
/// The design's failure model: repeated transient failures eventually set the
/// endpoint to `Degraded`, and only `vcli tunnel reconnect` clears it. This
/// is deliberately *not* a half-open state machine — a degraded endpoint
/// stays degraded until an operator acts, because the design treats silent
/// self-healing as the failure mode it is trying to prevent (an endpoint that
/// flips between healthy and broken without anyone noticing).
#[derive(Debug)]
pub struct CircuitBreaker {
    threshold: u32,
    state: Mutex<BreakerState>,
}

#[derive(Debug)]
struct BreakerState {
    degraded: bool,
    consecutive_failures: u32,
}

impl CircuitBreaker {
    /// Open the breaker after `threshold` consecutive transient failures.
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            state: Mutex::new(BreakerState {
                degraded: false,
                consecutive_failures: 0,
            }),
        }
    }

    /// Whether the endpoint is `Degraded` and must refuse new work until an
    /// explicit reset.
    pub fn is_degraded(&self) -> bool {
        self.state.lock().unwrap().degraded
    }

    /// Record a transient failure. Returns `true` exactly when this failure
    /// *tripped* the breaker (the caller logs the Degraded transition once).
    pub fn record_failure(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.degraded {
            return false;
        }
        s.consecutive_failures += 1;
        if s.consecutive_failures >= self.threshold {
            s.degraded = true;
            true
        } else {
            false
        }
    }

    /// Record a success. Clears the consecutive-failure count; cannot clear
    /// `Degraded` (only [`Self::reset`] can), per the design.
    pub fn record_success(&self) {
        let mut s = self.state.lock().unwrap();
        s.consecutive_failures = 0;
    }

    /// Explicit reset, driven by `vcli tunnel reconnect`.
    pub fn reset(&self) {
        let mut s = self.state.lock().unwrap();
        s.degraded = false;
        s.consecutive_failures = 0;
    }
}

/// Cooperative cancellation flag for sync code.
///
/// Operations poll [`Self::is_cancelled`] between units of work (channel
/// reads, file chunks) and return `TransportError::Cancelled`. There is no
/// preemption: the token cannot interrupt a blocked syscall, which is why
/// socket operations additionally carry deadlines.
#[derive(Debug, Default)]
pub struct CancellationToken {
    cancelled: AtomicBool,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Three-phase shutdown with a bounded grace period, as the design prescribes
/// for `tunnel stop`: stop admission, let running work finish within the
/// grace, then cancel whatever remains.
///
/// The grace comes from `VB_TRANSPORT_SHUTDOWN_GRACE` (default 10 s). The
/// cancel phase always runs — even when the grace expires with work still
/// active — because "stop" means stop.
#[derive(Debug, Clone, Copy)]
pub struct ShutdownCoordinator {
    grace: Duration,
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(10),
        }
    }
}

impl ShutdownCoordinator {
    pub fn new(grace: Duration) -> Self {
        Self { grace }
    }

    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Execute the three phases against caller-provided hooks:
    ///
    /// 1. `stop_admission` — refuse new work (scheduler, listener, IPC accept).
    /// 2. wait until `all_done()` reports quiescence, bounded by the grace.
    ///    Polls every 10 ms; the poll interval is an implementation detail.
    /// 3. `cancel_remaining` — always runs, including after the grace expires.
    pub fn execute(
        &self,
        stop_admission: impl FnOnce(),
        all_done: impl Fn() -> bool,
        cancel_remaining: impl FnOnce(),
    ) {
        stop_admission();
        let deadline = std::time::Instant::now() + self.grace;
        while std::time::Instant::now() < deadline && !all_done() {
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel_remaining();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::RequestId;

    // ── FailureClass ──

    #[test]
    fn security_and_policy_errors_are_permanent() {
        let cases: Vec<TransportError> = vec![
            TransportError::HostKeyChanged { host: "h".into() },
            TransportError::HostKeyUnknown {
                host: "h".into(),
                fingerprint: "fp".into(),
            },
            TransportError::HostKeyPolicyUnsupported("strict".into()),
            TransportError::AuthenticationFailed("bad key".into()),
            TransportError::InteractionRequired,
            TransportError::ProxyFailed("socks refused".into()),
            TransportError::Configuration("bad".into()),
            TransportError::UnsupportedBackend,
            TransportError::ProtocolMismatch {
                expected: "1".into(),
                actual: "2".into(),
            },
        ];
        for e in &cases {
            assert_eq!(
                FailureClass::of(e),
                FailureClass::Permanent,
                "{e:?} must be permanent"
            );
        }
    }

    #[test]
    fn network_path_errors_are_transient() {
        let cases: Vec<TransportError> = vec![
            TransportError::ConnectionFailed("refused".into()),
            TransportError::DaemonUnavailable,
            TransportError::JumpFailed("jump down".into()),
            TransportError::LocalIo("socket closed".into()),
        ];
        for e in &cases {
            assert_eq!(
                FailureClass::of(e),
                FailureClass::Transient,
                "{e:?} must be transient"
            );
        }
    }

    #[test]
    fn request_level_errors_never_trigger_reconnect() {
        let cases: Vec<TransportError> = vec![
            TransportError::QueueTimeout {
                request: RequestId::new(),
                after_secs: 1,
            },
            TransportError::RemoteExit {
                status: 1,
                stderr: String::new(),
            },
            TransportError::OutcomeUnknown {
                request: RequestId::new(),
                reason: "lost".into(),
            },
            TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: "cut".into(),
            },
            TransportError::Cancelled {
                request: RequestId::new(),
            },
            TransportError::RemoteIo("disk full".into()),
            TransportError::IntegrityMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            TransportError::ExecutionTimeout {
                request: RequestId::new(),
                after_secs: 2,
                remote_terminated: true,
            },
        ];
        for e in &cases {
            assert_eq!(
                FailureClass::of(e),
                FailureClass::RequestLevel,
                "{e:?} must be request-level"
            );
        }
    }

    #[test]
    fn execution_timeout_with_unknown_outcome_is_still_request_level() {
        // The request failed, not the connection. Reconnect policy must not
        // tear down the path; the caller decides about resubmission, and the
        // contract already forbids replay.
        let e = TransportError::ExecutionTimeout {
            request: RequestId::new(),
            after_secs: 3,
            remote_terminated: false,
        };
        assert_eq!(FailureClass::of(&e), FailureClass::RequestLevel);
    }

    // ── ReconnectPolicy ──

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy {
            max_attempts: 3,
            max_delay: Duration::from_secs(30),
            base: Duration::from_secs(1),
        }
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = ReconnectPolicy {
            max_attempts: 8,
            max_delay: Duration::from_secs(6),
            base: Duration::from_secs(1),
        };
        assert_eq!(p.delay_for_attempt(1), Duration::from_secs(1));
        assert_eq!(p.delay_for_attempt(2), Duration::from_secs(2));
        assert_eq!(p.delay_for_attempt(3), Duration::from_secs(4));
        // Cap: attempt 4 would be 8s raw, clamped to 6s; attempts beyond stay
        // at the cap and never exceed it.
        assert_eq!(p.delay_for_attempt(4), Duration::from_secs(6));
        assert_eq!(p.delay_for_attempt(9), Duration::from_secs(6));
        assert_eq!(p.delay_for_attempt(100), Duration::from_secs(6));
    }

    #[test]
    fn attempts_beyond_max_are_refused() {
        let p = policy();
        assert!(p.may_retry(1));
        assert!(p.may_retry(3));
        assert!(!p.may_retry(4));
    }

    #[test]
    fn jitter_stays_within_quarter_of_the_base_delay() {
        let p = policy();
        for seed in 0..1000u64 {
            let d = p.jittered_delay(1, seed);
            // base is 1s; jitter is ±250ms.
            assert!(
                d >= Duration::from_millis(750) && d <= Duration::from_millis(1250),
                "seed {seed} produced {d:?}"
            );
        }
    }

    #[test]
    fn jitter_never_exceeds_max_delay() {
        let p = ReconnectPolicy {
            max_attempts: 8,
            max_delay: Duration::from_secs(30),
            base: Duration::from_secs(30),
        };
        for seed in 0..1000u64 {
            assert!(p.jittered_delay(5, seed) <= Duration::from_secs(30));
        }
    }

    #[test]
    fn jitter_is_deterministic_for_a_seed() {
        let p = policy();
        assert_eq!(p.jittered_delay(2, 42), p.jittered_delay(2, 42));
    }

    // ── CircuitBreaker ──

    #[test]
    fn breaker_opens_after_threshold_consecutive_failures() {
        let b = CircuitBreaker::new(3);
        assert!(!b.is_degraded());
        assert!(!b.record_failure());
        assert!(!b.record_failure());
        // The third failure is the transition.
        assert!(b.record_failure());
        assert!(b.is_degraded());
        // Further failures are not new transitions.
        assert!(!b.record_failure());
    }

    #[test]
    fn success_resets_the_streak_but_not_degraded() {
        let b = CircuitBreaker::new(2);
        b.record_failure();
        b.record_success();
        assert!(!b.record_failure(), "streak was reset");
        assert!(b.record_failure(), "second consecutive failure trips");
        assert!(b.is_degraded());
        // Success cannot self-heal a degraded endpoint.
        b.record_success();
        assert!(b.is_degraded());
    }

    #[test]
    fn only_an_explicit_reset_clears_degraded() {
        let b = CircuitBreaker::new(1);
        b.record_failure();
        assert!(b.is_degraded());
        b.record_success();
        assert!(b.is_degraded());
        b.reset();
        assert!(!b.is_degraded());
        // And the failure count restarts from zero: with threshold 1 the very
        // next failure trips again, which proves the count did not carry over.
        assert!(b.record_failure());
        assert!(b.is_degraded());
    }

    #[test]
    fn threshold_is_at_least_one() {
        let b = CircuitBreaker::new(0);
        assert!(
            b.record_failure(),
            "a zero threshold must not disable the breaker"
        );
    }

    // ── CancellationToken ──

    #[test]
    fn cancellation_is_idempotent_and_visible() {
        let t = CancellationToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn concurrent_waiter_observes_cancellation_promptly() {
        let t = std::sync::Arc::new(CancellationToken::new());
        let t2 = std::sync::Arc::clone(&t);
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !t2.is_cancelled() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            t2.is_cancelled()
        });
        std::thread::sleep(Duration::from_millis(50));
        let start = std::time::Instant::now();
        t.cancel();
        assert!(handle.join().unwrap());
        // The waiter must have observed cancellation almost immediately.
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    // ── ShutdownCoordinator ──

    #[test]
    fn shutdown_runs_all_three_phases_in_order() {
        let c = ShutdownCoordinator::new(Duration::from_millis(50));
        let log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let l1 = std::sync::Arc::clone(&log);
        let l2 = std::sync::Arc::clone(&log);
        let l3 = std::sync::Arc::clone(&log);
        c.execute(
            move || l1.lock().unwrap().push("admission"),
            || false, // work never finishes: grace must expire
            move || l3.lock().unwrap().push("cancel"),
        );
        // (l2 unused when all_done is a constant; keep the clone honest.)
        drop(l2);
        assert_eq!(*log.lock().unwrap(), vec!["admission", "cancel"]);
    }

    #[test]
    fn shutdown_cancels_as_soon_as_work_quiesces() {
        let c = ShutdownCoordinator::new(Duration::from_secs(30));
        // `all_done` returns false on the first poll and true afterwards,
        // simulating work that finishes right after admission stops. The
        // coordinator must not sit out the (30 s) grace.
        let polls = std::sync::Arc::new(AtomicBool::new(false));
        let log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let l = std::sync::Arc::clone(&log);
        let start = std::time::Instant::now();
        c.execute(
            || {},
            {
                let polls = std::sync::Arc::clone(&polls);
                move || {
                    // False exactly once, then true: simulates work that
                    // finishes right after admission stops.
                    !polls.swap(true, Ordering::SeqCst)
                }
            },
            move || l.lock().unwrap().push("cancel"),
        );
        assert_eq!(*log.lock().unwrap(), vec!["cancel"]);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "must not wait the full grace: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn grace_default_matches_the_design() {
        assert_eq!(
            ShutdownCoordinator::default().grace(),
            Duration::from_secs(10)
        );
    }
}

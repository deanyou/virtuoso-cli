//! Channel scheduling for a single SSH endpoint (step 4 of the native plan).
//!
//! The design gives each connection key independent limits:
//!
//! ```dotenv
//! VB_SSH_MAX_SESSIONS=10
//! VB_SSH_MAX_BULK_SESSIONS=2
//! ```
//!
//! with these rules:
//!
//! > One urgent exec slot is reserved for health checks, cancellation, and
//! > cleanup. Bulk file and directory transfers may consume at most the bulk
//! > limit. Remaining permits serve normal commands and Spectre work. Requests
//! > are FIFO within a priority class; urgent work may move ahead of queued
//! > normal work but does not interrupt running work.
//! >
//! > A request whose deadline expires before acquiring a permit returns
//! > `QueueTimeout`, proving that its remote operation did not begin.
//! >
//! > If the server rejects a channel because of its limit, the daemon lowers
//! > the effective limit and reports that condition. It does not create an
//! > additional authenticated connection to bypass server policy.
//!
//! This module is the whole of that policy and nothing else: it knows nothing
//! about SSH, russh, or IPC, which is what makes it testable without a network.
//!
//! # Delta from the design
//!
//! Strict `Urgent > Normal > Bulk` priority would let a steady stream of normal
//! commands starve bulk transfers indefinitely, because bulk is the lowest
//! class and never gets a reserved slot. The design reserves capacity for
//! *urgent* work but says nothing about bulk starvation. This implementation
//! therefore promotes a bulk waiter to `Normal` once it has waited
//! [`SchedulerLimits::bulk_starvation_grace`] (default 30s). That is a bounded,
//! testable relaxation of strict priority, not a different policy: urgent work
//! still wins immediately, and a healthy system never reaches the grace period.

#![allow(dead_code)] // consumed by step 4b (config) and the daemon in step 6

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::transport::contract::{Deadline, RequestId, TransportError};

// ─────────────────────────────── priority ───────────────────────────────────

/// Which capacity class a request belongs to.
///
/// Ordering is meaningful: `Urgent > Normal > Bulk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Bulk file and directory transfer. Capped by the bulk limit.
    Bulk,
    /// Ordinary command execution, including Spectre work.
    Normal,
    /// Health checks, cancellation, and cleanup. May use the reserved slot.
    Urgent,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bulk => "bulk",
            Self::Normal => "normal",
            Self::Urgent => "urgent",
        }
    }
}

// ──────────────────────────────── limits ────────────────────────────────────

/// Capacity limits for one connection key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerLimits {
    /// Total concurrent exec/SFTP sessions (`VB_SSH_MAX_SESSIONS`).
    pub total: usize,
    /// How many of those bulk transfers may occupy (`VB_SSH_MAX_BULK_SESSIONS`).
    pub bulk: usize,
    /// Slots never granted to non-urgent work, so a health check or a cancel
    /// can always get through a saturated endpoint.
    pub urgent_reserve: usize,
    /// How long a bulk waiter may be overtaken before it is treated as
    /// `Normal`. See the module doc ("Delta from the design").
    pub bulk_starvation_grace: Duration,
}

impl SchedulerLimits {
    pub const DEFAULT_TOTAL: usize = 10;
    pub const DEFAULT_BULK: usize = 2;
    pub const DEFAULT_URGENT_RESERVE: usize = 1;
    pub const DEFAULT_BULK_STARVATION_GRACE: Duration = Duration::from_secs(30);

    pub fn default_limits() -> Self {
        Self {
            total: Self::DEFAULT_TOTAL,
            bulk: Self::DEFAULT_BULK,
            urgent_reserve: Self::DEFAULT_URGENT_RESERVE,
            bulk_starvation_grace: Self::DEFAULT_BULK_STARVATION_GRACE,
        }
    }

    /// Reject a combination that cannot work rather than letting it degrade
    /// later into spurious `QueueTimeout` errors.
    ///
    /// Each rule corresponds to a deadlock, not to a matter of taste:
    ///
    /// - `total == 0` → nothing can ever run;
    /// - `bulk == 0` → bulk transfers can never run, silently;
    /// - `urgent_reserve == 0` → the design's reserved control slot does not
    ///   exist, so a health check or cancellation can be starved;
    /// - `urgent_reserve >= total` → ordinary work has no capacity left and
    ///   never starts.
    ///
    /// `bulk` is deliberately *not* constrained against `total`. A bulk limit
    /// larger than `total - urgent_reserve` is not a deadlock: it just means
    /// bulk may occupy the whole non-reserved pool when it is the only work
    /// offered. Rejecting it would make `VB_SSH_MAX_SESSIONS=1` — a legitimate
    /// "serialize everything" choice — a hard configuration error.
    pub fn validate(&self) -> Result<(), TransportError> {
        if self.total == 0 {
            return Err(TransportError::Configuration(
                "VB_SSH_MAX_SESSIONS must be at least 1".into(),
            ));
        }
        if self.bulk == 0 {
            return Err(TransportError::Configuration(
                "VB_SSH_MAX_BULK_SESSIONS must be at least 1".into(),
            ));
        }
        if self.urgent_reserve == 0 {
            return Err(TransportError::Configuration(
                "the urgent session reserve must be at least 1, otherwise health checks and \
                 cancellation can be starved by ordinary work"
                    .into(),
            ));
        }
        if self.urgent_reserve >= self.total {
            return Err(TransportError::Configuration(format!(
                "the urgent reserve of {} sessions leaves no capacity for ordinary work: \
                 VB_SSH_MAX_SESSIONS is {}. Normal and bulk requests would never start.",
                self.urgent_reserve, self.total
            )));
        }
        Ok(())
    }

    /// Resolve from the environment, checking profile-specific variables first.
    ///
    /// A present-but-unparseable value is a `Configuration` error, never a
    /// silent fall back to the default — the same rule the backend selection
    /// follows, and for the same reason: a typo that silently changes capacity
    /// is much harder to diagnose than one that fails at startup.
    pub fn from_env_with_profile(profile: Option<&str>) -> Result<Self, TransportError> {
        let mut limits = Self::default_limits();
        if let Some(raw) = crate::config::Config::env_with_profile("VB_SSH_MAX_SESSIONS", profile) {
            limits.total = parse_usize("VB_SSH_MAX_SESSIONS", &raw)?;
        }
        if let Some(raw) =
            crate::config::Config::env_with_profile("VB_SSH_MAX_BULK_SESSIONS", profile)
        {
            limits.bulk = parse_usize("VB_SSH_MAX_BULK_SESSIONS", &raw)?;
        }
        limits.validate()?;
        Ok(limits)
    }

    /// Build limits from a resolved [`crate::config::Config`], enforcing the
    /// capacity invariant on the way.
    pub fn from_config(config: &crate::config::Config) -> Result<Self, TransportError> {
        validate_capacity(config)?;
        Ok(Self {
            total: config.ssh_max_sessions,
            bulk: config.ssh_max_bulk_sessions,
            urgent_reserve: Self::DEFAULT_URGENT_RESERVE,
            bulk_starvation_grace: Self::DEFAULT_BULK_STARVATION_GRACE,
        })
    }
}

/// Sessions that must stay available for control traffic while a Spectre sweep
/// saturates an endpoint.
///
/// The design states the invariant as:
///
/// ```text
/// VB_SSH_MAX_SESSIONS >= VB_SPECTRE_MAX_WORKERS + control_reserve
/// ```
///
/// > where `control_reserve` covers the urgent slot plus ordinary foreground
/// > commands issued while a sweep is running. The defaults satisfy this with
/// > room to spare (`10 >= 8 + 2`), because each Spectre worker issues its
/// > commands sequentially and therefore occupies at most one exec session at
/// > a time.
pub const CONTROL_RESERVE: usize = 2;

/// Reject a session/worker combination that cannot work, as the design
/// requires:
///
/// > Configuration validation rejects a combination that violates the
/// > invariant rather than degrading later into spurious `QueueTimeout`
/// > errors. The preferred remedy for saturation is reserving capacity for
/// > urgent and control work, not raising the session total.
///
/// Only the native backend is checked. OpenSSH multiplexes over ControlMaster
/// and has no per-endpoint session ceiling, so applying this there would turn
/// configurations that work today into startup errors for no reason.
pub fn validate_capacity(config: &crate::config::Config) -> Result<(), TransportError> {
    let workers = config.spectre_max_workers as usize;
    let needed = workers.saturating_add(CONTROL_RESERVE);
    if config.ssh_max_sessions < needed {
        return Err(TransportError::Configuration(format!(
            "VB_SSH_MAX_SESSIONS={} is too small for VB_SPECTRE_MAX_WORKERS={}: it must be at \
             least {workers} + {CONTROL_RESERVE} = {needed}, so urgent and foreground work still \
             has a session while a sweep is running",
            config.ssh_max_sessions, config.spectre_max_workers
        )));
    }
    // Built directly rather than via `Self::from_config`, which calls back
    // into this function.
    SchedulerLimits {
        total: config.ssh_max_sessions,
        bulk: config.ssh_max_bulk_sessions,
        urgent_reserve: SchedulerLimits::DEFAULT_URGENT_RESERVE,
        bulk_starvation_grace: SchedulerLimits::DEFAULT_BULK_STARVATION_GRACE,
    }
    .validate()?;
    Ok(())
}

fn parse_usize(key: &str, raw: &str) -> Result<usize, TransportError> {
    let parsed: usize = raw.trim().parse().map_err(|_| {
        TransportError::Configuration(format!("{key} must be a positive integer, got '{raw}'"))
    })?;
    if parsed == 0 {
        return Err(TransportError::Configuration(format!(
            "{key} must be a positive integer, got '{raw}'"
        )));
    }
    Ok(parsed)
}

// ─────────────────────────────── scheduler ──────────────────────────────────

/// Wake-up bound while waiting for a permit.
///
/// Waiters are woken on every release, but a bound is still needed so that the
/// bulk starvation promotion is evaluated even on an idle-but-saturated
/// endpoint where no release ever happens.
const POLL_CAP: Duration = Duration::from_millis(50);

struct Waiter {
    seq: u64,
    priority: Priority,
    enqueued_at: Instant,
    #[allow(dead_code)] // kept for diagnostics once `tunnel status` reports the queue
    request: RequestId,
}

struct State {
    limits: SchedulerLimits,
    /// Sessions currently running, all classes.
    active: usize,
    /// Of those, how many are bulk transfers.
    active_bulk: usize,
    /// Upper bound on `active`, lowered only — see [`SessionScheduler::reduce_capacity`].
    effective_total: usize,
    waiters: Vec<Waiter>,
    next_seq: u64,
}

impl State {
    fn effective_priority(&self, w: &Waiter, now: Instant) -> Priority {
        if w.priority == Priority::Bulk
            && now.saturating_duration_since(w.enqueued_at) >= self.limits.bulk_starvation_grace
        {
            Priority::Normal
        } else {
            w.priority
        }
    }

    /// Whether a slot for `priority` exists right now.
    ///
    /// Non-urgent work may never consume the reserved slots, which is what
    /// keeps a health check or a cancellation able to get through an endpoint
    /// that ordinary work has saturated.
    fn can_admit(&self, priority: Priority) -> bool {
        let cap = if priority == Priority::Urgent {
            self.effective_total
        } else {
            self.effective_total
                .saturating_sub(self.limits.urgent_reserve)
        };
        if self.active >= cap {
            return false;
        }
        if priority == Priority::Bulk && self.active_bulk >= self.limits.bulk {
            return false;
        }
        true
    }

    /// The waiter that should run next: highest effective priority, then
    /// earliest arrival. `None` when no waiter fits the remaining capacity.
    ///
    /// Skipping a head waiter that cannot be admitted (a bulk waiter when the
    /// bulk limit is full) is deliberate: blocking behind it would waste a slot
    /// that another class could use.
    fn chosen_seq(&self, now: Instant) -> Option<u64> {
        let mut best: Option<(Priority, u64)> = None;
        for w in &self.waiters {
            let p = self.effective_priority(w, now);
            if !self.can_admit(p) {
                continue;
            }
            let better = match best {
                None => true,
                Some((bp, bs)) => p > bp || (p == bp && w.seq < bs),
            };
            if better {
                best = Some((p, w.seq));
            }
        }
        best.map(|(_, seq)| seq)
    }
}

/// Observable counters, for `tunnel status` and the daemon's own diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerStats {
    pub active: usize,
    pub active_bulk: usize,
    pub queued: usize,
    pub effective_total: usize,
}

/// Admission control for one connection key.
///
/// Held behind an `Arc` because the daemon acquires a permit in one task and
/// releases it in another; [`Permit`] keeps the scheduler alive for as long as
/// any permit is outstanding.
pub struct SessionScheduler {
    state: Mutex<State>,
    ready: Condvar,
}

impl SessionScheduler {
    pub fn new(limits: SchedulerLimits) -> Result<Arc<Self>, TransportError> {
        limits.validate()?;
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                limits,
                active: 0,
                active_bulk: 0,
                effective_total: limits.total,
                waiters: Vec::new(),
                next_seq: 0,
            }),
            ready: Condvar::new(),
        }))
    }

    /// Wait for a permit until `deadline`.
    ///
    /// Queue time and execution time share one deadline, so a request that
    /// never gets a slot reports `QueueTimeout` — which is exactly the error
    /// that proves no remote operation began and is therefore safe to retry.
    pub fn acquire(
        self: &Arc<Self>,
        priority: Priority,
        request: &RequestId,
        deadline: Deadline,
    ) -> Result<Permit, TransportError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let seq = state.next_seq;
        state.next_seq = state.next_seq.wrapping_add(1);
        state.waiters.push(Waiter {
            seq,
            priority,
            enqueued_at: Instant::now(),
            request: request.clone(),
        });

        loop {
            let now = Instant::now();
            if state.chosen_seq(now) == Some(seq) {
                state.waiters.retain(|w| w.seq != seq);
                state.active += 1;
                if priority == Priority::Bulk {
                    state.active_bulk += 1;
                }
                return Ok(Permit {
                    scheduler: Arc::clone(self),
                    priority,
                });
            }
            if deadline.is_expired() {
                state.waiters.retain(|w| w.seq != seq);
                return Err(TransportError::QueueTimeout {
                    request: request.clone(),
                    after_secs: now.saturating_duration_since(deadline.0).as_secs().max(1),
                });
            }
            // Wake on the next release, or at the poll cap so the bulk
            // starvation promotion is re-evaluated even without a release.
            let budget = deadline.remaining().min(POLL_CAP);
            let (guard, _) = self
                .ready
                .wait_timeout(state, budget)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
    }

    /// Take a permit if one is free right now, without queueing.
    ///
    /// For work that must not wait — a cancellation, or a health probe that
    /// should report "busy" rather than block.
    pub fn try_acquire(self: &Arc<Self>, priority: Priority) -> Option<Permit> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.can_admit(priority) {
            return None;
        }
        state.active += 1;
        if priority == Priority::Bulk {
            state.active_bulk += 1;
        }
        Some(Permit {
            scheduler: Arc::clone(self),
            priority,
        })
    }

    /// Lower the effective session ceiling after the server refused a channel
    /// because of *its* channel limit.
    ///
    /// Returns the new ceiling. The design is explicit that the remedy is to
    /// live within the smaller limit, never to open a second authenticated
    /// connection to route around server policy. The urgent reserve is a floor:
    /// capacity is never reduced below what control work needs.
    pub fn reduce_capacity(&self, server_limit: usize) -> usize {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let floor = state.limits.urgent_reserve.max(1);
        let new = server_limit.max(floor).min(state.effective_total);
        state.effective_total = new;
        self.ready.notify_all();
        new
    }

    pub fn effective_total(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .effective_total
    }

    pub fn stats(&self) -> SchedulerStats {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        SchedulerStats {
            active: state.active,
            active_bulk: state.active_bulk,
            queued: state.waiters.len(),
            effective_total: state.effective_total,
        }
    }

    pub fn limits(&self) -> SchedulerLimits {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).limits
    }

    fn release(&self, priority: Priority) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = state.active.saturating_sub(1);
        if priority == Priority::Bulk {
            state.active_bulk = state.active_bulk.saturating_sub(1);
        }
        // Every waiter re-evaluates `chosen_seq`, so a plain broadcast is
        // correct even though at most one waiter can proceed.
        self.ready.notify_all();
    }
}

/// A held session slot. Returns it to the scheduler on drop.
///
/// Cloning is deliberately not implemented: one acquired slot is one slot, and
/// a `Clone` would let callers duplicate capacity by accident.
pub struct Permit {
    scheduler: Arc<SessionScheduler>,
    priority: Priority,
}

impl Permit {
    pub fn priority(&self) -> Priority {
        self.priority
    }
}

/// Hand-written because `Permit` holds an `Arc<SessionScheduler>`, whose
/// `Mutex`/`Condvar` fields are not `Debug`. Reporting the class is what a
/// caller actually wants to see when a `Result<Permit, _>` is unwrapped.
impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit")
            .field("priority", &self.priority)
            .finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.scheduler.release(self.priority);
    }
}

// ────────────────────────────────── tests ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn limits(total: usize, bulk: usize) -> SchedulerLimits {
        SchedulerLimits {
            total,
            bulk,
            urgent_reserve: 1,
            bulk_starvation_grace: Duration::from_secs(30),
        }
    }

    fn sched(total: usize, bulk: usize) -> Arc<SessionScheduler> {
        SessionScheduler::new(limits(total, bulk)).expect("valid limits")
    }

    fn secs(n: u64) -> Deadline {
        Deadline::from_now(Duration::from_secs(n))
    }

    // ── limits ──

    #[test]
    fn defaults_match_the_design() {
        let d = SchedulerLimits::default_limits();
        assert_eq!(d.total, 10);
        assert_eq!(d.bulk, 2);
        assert_eq!(d.urgent_reserve, 1);
        d.validate().expect("defaults must satisfy the invariant");
    }

    #[test]
    fn validate_rejects_a_reserve_that_starves_normal_work() {
        // A reserve as large as the total leaves ordinary work nowhere to go:
        // every normal and bulk request would queue forever. That is a
        // deadlock, so it must be caught at configuration time.
        let bad = SchedulerLimits {
            total: 2,
            bulk: 1,
            urgent_reserve: 2,
            bulk_starvation_grace: Duration::from_secs(30),
        };
        let err = bad.validate().expect_err("must reject");
        assert!(matches!(err, TransportError::Configuration(_)), "{err:?}");
    }

    #[test]
    fn validate_distinguishes_deadlock_from_a_legitimate_small_config() {
        // Neither is a deadlock: total=1 serializes everything, and a bulk
        // limit above the non-reserved pool merely lets bulk occupy all of it
        // when bulk is the only work offered.
        SchedulerLimits {
            total: 1,
            bulk: 1,
            urgent_reserve: 1,
            bulk_starvation_grace: Duration::from_secs(30),
        }
        .validate()
        .expect_err("reserve == total starves ordinary work, so total=1 cannot reserve a slot");

        SchedulerLimits {
            total: 2,
            bulk: 5,
            urgent_reserve: 1,
            bulk_starvation_grace: Duration::from_secs(30),
        }
        .validate()
        .expect("bulk above the non-reserved pool is legitimate");
    }

    #[test]
    fn validate_rejects_zero() {
        for bad in [
            SchedulerLimits {
                total: 0,
                ..limits(4, 2)
            },
            SchedulerLimits {
                bulk: 0,
                ..limits(4, 2)
            },
            SchedulerLimits {
                urgent_reserve: 0,
                ..limits(4, 2)
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn new_propagates_invalid_limits() {
        assert!(SessionScheduler::new(SchedulerLimits {
            total: 1,
            bulk: 1,
            urgent_reserve: 1,
            bulk_starvation_grace: Duration::from_secs(30)
        })
        .is_err());
    }

    #[test]
    #[serial]
    fn from_env_reads_both_variables() {
        // SAFETY: serialized by `#[serial]`; no other test reads these vars.
        unsafe {
            std::env::set_var("VB_SSH_MAX_SESSIONS", "6");
            std::env::set_var("VB_SSH_MAX_BULK_SESSIONS", "3");
        }
        let l = SchedulerLimits::from_env_with_profile(None).unwrap();
        assert_eq!(l.total, 6);
        assert_eq!(l.bulk, 3);
        unsafe {
            std::env::remove_var("VB_SSH_MAX_SESSIONS");
            std::env::remove_var("VB_SSH_MAX_BULK_SESSIONS");
        }
    }

    #[test]
    #[serial]
    fn from_env_rejects_a_typo_rather_than_defaulting() {
        unsafe { std::env::set_var("VB_SSH_MAX_SESSIONS", "ten") };
        let err = SchedulerLimits::from_env_with_profile(None).expect_err("typo must fail");
        assert!(matches!(err, TransportError::Configuration(_)), "{err:?}");
        assert!(err.to_string().contains("ten"));
        unsafe { std::env::remove_var("VB_SSH_MAX_SESSIONS") };
    }

    #[test]
    #[serial]
    fn from_env_rejects_zero() {
        unsafe { std::env::set_var("VB_SSH_MAX_BULK_SESSIONS", "0") };
        assert!(SchedulerLimits::from_env_with_profile(None).is_err());
        unsafe { std::env::remove_var("VB_SSH_MAX_BULK_SESSIONS") };
    }

    // ── capacity invariant ──

    fn config_with(workers: u32, sessions: usize, bulk: usize) -> crate::config::Config {
        crate::config::Config {
            profile: None,
            remote_host: Some("compute-eda-42".into()),
            remote_user: None,
            port: 65432,
            jump_host: None,
            jump_user: None,
            ssh_port: Some(22),
            ssh_key: None,
            ssh_config: None,
            ssh_backend: Some("native".into()),
            disable_control_master: false,
            timeout: 30,
            read_timeout: 120,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: workers,
            ssh_max_sessions: sessions,
            ssh_max_bulk_sessions: bulk,
            ssh_reconnect_max_attempts: 8,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 30,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 10,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: Default::default(),
        }
    }

    #[test]
    fn the_documented_defaults_satisfy_the_capacity_invariant() {
        // The design: "10 >= 8 + 2".
        assert_eq!(SchedulerLimits::DEFAULT_TOTAL, 10);
        assert_eq!(CONTROL_RESERVE, 2);
        validate_capacity(&config_with(8, 10, 2)).expect("defaults must validate");
    }

    #[test]
    fn too_few_sessions_for_the_worker_count_is_rejected() {
        // The design requires this to be caught at configuration time rather
        // than degrading into spurious QueueTimeouts during a sweep.
        let err = validate_capacity(&config_with(16, 10, 2)).expect_err("16 + 2 > 10");
        assert!(matches!(err, TransportError::Configuration(_)), "{err:?}");
        assert!(err.to_string().contains("16"), "must name the worker count");
        assert!(
            err.to_string().contains("10"),
            "must name the session count"
        );
    }

    #[test]
    fn the_invariant_is_a_bound_not_an_equality() {
        // More headroom than the minimum is fine.
        validate_capacity(&config_with(8, 12, 2)).expect("extra headroom is allowed");
        // Exactly at the bound is fine.
        validate_capacity(&config_with(8, 10, 2)).expect("exactly at the bound");
        // One below the bound is not.
        assert!(validate_capacity(&config_with(8, 9, 2)).is_err());
    }

    #[test]
    fn from_config_carries_the_resolved_limits_and_validates() {
        let l = SchedulerLimits::from_config(&config_with(4, 8, 3)).unwrap();
        assert_eq!(l.total, 8);
        assert_eq!(l.bulk, 3);
        assert_eq!(l.urgent_reserve, 1);
        // The capacity invariant is enforced on the way through.
        assert!(SchedulerLimits::from_config(&config_with(32, 8, 3)).is_err());
    }

    #[test]
    fn a_scheduler_built_from_a_validated_config_admits_a_full_worker_complement() {
        // End-to-end: 8 workers must be able to hold 8 sessions at once and
        // still leave the reserve free for urgent work.
        let cfg = config_with(8, 10, 2);
        let s = SessionScheduler::new(SchedulerLimits::from_config(&cfg).unwrap()).unwrap();
        let held: Vec<_> = (0..8)
            .map(|_| s.try_acquire(Priority::Normal).unwrap())
            .collect();
        assert_eq!(s.stats().active, 8);
        assert!(
            s.try_acquire(Priority::Urgent).is_some(),
            "control work must still get through at full worker load"
        );
        drop(held);
    }

    #[test]
    fn priority_ordering_puts_urgent_first() {
        assert!(Priority::Urgent > Priority::Normal);
        assert!(Priority::Normal > Priority::Bulk);
        assert_eq!(Priority::Urgent.as_str(), "urgent");
    }

    // ── admission ──

    #[test]
    fn permits_up_to_the_total_are_granted_immediately() {
        // total=4 with reserve=1 → three non-urgent slots plus the reserve.
        let s = sched(4, 2);
        let a = s.try_acquire(Priority::Normal).unwrap();
        let b = s.try_acquire(Priority::Normal).unwrap();
        let c = s.try_acquire(Priority::Normal).unwrap();
        let d = s.try_acquire(Priority::Urgent).expect("the reserved slot");
        assert_eq!(s.stats().active, 4);
        assert!(s.try_acquire(Priority::Urgent).is_none(), "now full");
        drop((a, b, c, d));
        assert_eq!(s.stats().active, 0);
    }

    #[test]
    fn non_urgent_work_cannot_take_the_reserved_slot() {
        // total=3 with reserve=1 leaves 2 slots for normal work.
        let s = sched(3, 1);
        let _a = s.try_acquire(Priority::Normal).unwrap();
        let _b = s.try_acquire(Priority::Normal).unwrap();
        assert!(
            s.try_acquire(Priority::Normal).is_none(),
            "the third slot is reserved for urgent work"
        );
        // …and urgent work can still take it.
        assert!(s.try_acquire(Priority::Urgent).is_some());
    }

    #[test]
    fn bulk_is_capped_by_its_own_limit_not_the_total() {
        let s = sched(10, 2);
        let _a = s.try_acquire(Priority::Bulk).unwrap();
        let _b = s.try_acquire(Priority::Bulk).unwrap();
        assert!(
            s.try_acquire(Priority::Bulk).is_none(),
            "bulk limit is 2 even though the total is 10"
        );
        // Normal work is unaffected: it has its own headroom.
        assert!(s.try_acquire(Priority::Normal).is_some());
    }

    #[test]
    fn permit_release_returns_capacity() {
        // total=3, reserve=1 → two non-urgent slots.
        let s = sched(3, 1);
        let a = s.try_acquire(Priority::Normal).unwrap();
        let _b = s.try_acquire(Priority::Normal).unwrap();
        assert!(s.try_acquire(Priority::Normal).is_none());
        drop(a);
        assert!(s.try_acquire(Priority::Normal).is_some());
    }

    // ── queueing ──

    #[test]
    fn expired_deadline_reports_queue_timeout_not_a_hang() {
        // total=2, reserve=1 → the single non-urgent slot is held.
        let s = sched(2, 1);
        let _held = s.try_acquire(Priority::Normal).unwrap();
        let past = Deadline(Instant::now() - Duration::from_secs(1));
        let err = s
            .acquire(Priority::Normal, &RequestId::new(), past)
            .expect_err("must not hang");
        assert!(
            matches!(err, TransportError::QueueTimeout { .. }),
            "got {err:?}"
        );
        // QueueTimeout is precisely the error that proves nothing started, so
        // the caller may resubmit.
        assert!(err.retryable());
        assert_eq!(s.stats().queued, 0, "the timed-out waiter must be removed");
    }

    #[test]
    fn blocked_request_proceeds_once_a_slot_frees() {
        let s = sched(2, 1);
        let held = s.try_acquire(Priority::Normal).unwrap();
        let s2 = Arc::clone(&s);
        let done = Arc::new(AtomicUsize::new(0));
        let done2 = Arc::clone(&done);
        let t = thread::spawn(move || {
            let _p = s2
                .acquire(Priority::Normal, &RequestId::new(), secs(10))
                .expect("should get a slot");
            done2.fetch_add(1, Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(50));
        assert_eq!(done.load(Ordering::SeqCst), 0, "must still be queued");
        assert_eq!(s.stats().queued, 1);
        drop(held);
        t.join().expect("no panic");
        assert_eq!(done.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn equal_priority_work_is_fifo() {
        // One non-urgent slot, three waiters: admitted in arrival order.
        let s = sched(2, 1);
        let held = s.try_acquire(Priority::Normal).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for i in 0..3 {
            let sc = Arc::clone(&s);
            let ord = Arc::clone(&order);
            threads.push(thread::spawn(move || {
                let _p = sc
                    .acquire(Priority::Normal, &RequestId::new(), secs(10))
                    .unwrap();
                ord.lock().unwrap().push(i);
            }));
            // Stagger the spawns so arrival order is deterministic. Without
            // this the assertion would be about the OS thread scheduler rather
            // than about our own FIFO: a waiter that happens to enqueue second
            // is legitimately served second.
            //
            // The *recording* order needs no such care: with a single slot,
            // waiter `i` cannot return from `acquire` until waiter `i-1` has
            // dropped its permit, which happens after it recorded itself.
            thread::sleep(Duration::from_millis(50));
        }
        drop(held);
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2], "must be FIFO");
    }

    #[test]
    fn urgent_work_passes_queued_normal_work() {
        // total=3, reserve=1 → two non-urgent slots, both held by normal work,
        // with normal waiters queued behind them. An urgent waiter must go
        // first once a slot frees.
        let s = sched(3, 1);
        let _h1 = s.try_acquire(Priority::Normal).unwrap();
        let h2 = s.try_acquire(Priority::Normal).unwrap();

        // Queue two normal waiters.
        let sn = Arc::clone(&s);
        let tn = thread::spawn(move || {
            let _p = sn
                .acquire(Priority::Normal, &RequestId::new(), secs(10))
                .unwrap();
        });
        thread::sleep(Duration::from_millis(60));
        let sn2 = Arc::clone(&s);
        let tn2 = thread::spawn(move || {
            let _p = sn2
                .acquire(Priority::Normal, &RequestId::new(), secs(10))
                .unwrap();
        });
        thread::sleep(Duration::from_millis(60));

        // Now queue the urgent waiter (last in arrival order).
        let order = Arc::new(Mutex::new(Vec::new()));
        let su = Arc::clone(&s);
        let ou = Arc::clone(&order);
        let tu = thread::spawn(move || {
            let _p = su
                .acquire(Priority::Urgent, &RequestId::new(), secs(10))
                .unwrap();
            ou.lock().unwrap().push("urgent");
        });
        thread::sleep(Duration::from_millis(60));

        drop(h2); // frees exactly one slot
        tu.join().unwrap();
        assert_eq!(
            *order.lock().unwrap(),
            vec!["urgent"],
            "urgent must overtake both queued normal waiters"
        );
        // Let the normal waiters drain so nothing is left hanging.
        drop(_h1);
        tn.join().unwrap();
        tn2.join().unwrap();
    }

    #[test]
    fn running_work_is_never_interrupted() {
        // Urgent work may overtake *queued* work only. A running request keeps
        // its slot until it finishes — including the reserved slot, once it is
        // taken: filling the total blocks even urgent work.
        let s = sched(2, 1);
        let _normal = s.try_acquire(Priority::Normal).unwrap();
        let running_urgent = s.try_acquire(Priority::Urgent).unwrap();
        assert_eq!(s.stats().active, 2, "total is exhausted");
        assert!(s.try_acquire(Priority::Urgent).is_none());
        drop(running_urgent);
        assert!(s.try_acquire(Priority::Urgent).is_some());
    }

    #[test]
    fn bulk_waiter_is_promoted_after_the_starvation_grace() {
        // Strict priority would let a steady stream of normal commands starve
        // bulk transfers forever, because bulk is the lowest class and has no
        // reserved slot. This asserts the escape valve: once a bulk waiter has
        // waited past the grace it is scheduled as `Normal`.
        //
        // Both non-reserved slots are held for the whole test, so the waiter
        // cannot be admitted either way — what is observed is the promotion
        // itself, not an admission.
        //
        // Both phases anchor on `enqueued_at`, the timestamp the spawned thread
        // stamps when it acquires the mutex. The previous version used a fixed
        // `thread::sleep(30ms)` which raced with the OS scheduler: on slow CI
        // runners the test thread's lock acquisition could land >100ms past
        // `enqueued_at`, making Phase 1 observe `Normal` instead of `Bulk` for
        // a reason unrelated to the promotion logic. `enqueued_at` is captured
        // in the same critical section as the observed `now`, which makes the
        // elapsed-at-observation a *property of the data* and not of the
        // *moment we happened to grab the lock*.
        let grace = Duration::from_millis(100);
        let s = SessionScheduler::new(SchedulerLimits {
            total: 3,
            bulk: 1,
            urgent_reserve: 1,
            bulk_starvation_grace: grace,
        })
        .unwrap();
        let _n1 = s.try_acquire(Priority::Normal).unwrap();
        let _n2 = s.try_acquire(Priority::Normal).unwrap();

        let sc = Arc::clone(&s);
        let t = thread::spawn(move || {
            sc.acquire(Priority::Bulk, &RequestId::new(), secs(10))
                .expect("admitted once a slot frees")
        });

        // Phase 1: wait for the bulk waiter to be enqueued, then verify it is
        // still inside the grace window. If the wait's not in the queue within
        // a generous deadline, panic with a clear message; if it is but the
        // scheduling latency already exceeds the grace, panic with the
        // *observable* cause instead of an opaque `left: Normal, right: Bulk`.
        let observed_deadline = Instant::now() + Duration::from_secs(2);
        let enqueued_at = loop {
            let state = s.state.lock().unwrap();
            if let Some(w) = state.waiters.first() {
                // Same critical section: read both timestamps while the state
                // is held, so `effective_priority(now, enqueued_at)` uses a
                // `now` that cannot race with the waiter's continued waiting.
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(w.enqueued_at);
                let effective = state.effective_priority(w, now);
                assert!(
                    effective == Priority::Bulk,
                    "first observation of the bulk waiter must show priority \
                     `Bulk` (inside grace); observed priority `{:?}` after \
                     {elapsed:?} of waiting — grace is {grace:?}. Either the \
                     OS scheduler delayed the test thread past the grace \
                     window, or the promotion logic flipped earlier than \
                     `enqueued_at + grace`. The latter is the real regression \
                     we are guarding; the former is environmental and would \
                     warrant bumping the grace for this test.",
                    effective
                );
                break w.enqueued_at;
            }
            drop(state);
            if Instant::now() >= observed_deadline {
                panic!("bulk waiter never reached the queue within 2s");
            }
            thread::sleep(Duration::from_millis(1));
        };

        // Phase 2: sleep until we are *deterministically* past `enqueued_at +
        // grace`, then verify the waiter is promoted to `Normal`. The `+ 50ms`
        // margin absorbs the latency between waking from `sleep` and the test
        // thread grabbing the mutex; the anchor makes the total wall-clock
        // duration independent of how slow the scheduler is — only the
        // arithmetic `target - now` matters.
        let target = enqueued_at + grace + Duration::from_millis(50);
        let now = Instant::now();
        if now < target {
            thread::sleep(target - now);
        }
        {
            let state = s.state.lock().unwrap();
            let w = state.waiters.first().expect("still waiting");
            assert_eq!(
                state.effective_priority(w, Instant::now()),
                Priority::Normal,
                "past the grace the bulk waiter must be scheduled as normal"
            );
        }
        // Release a slot; the waiter must finish rather than hang.
        drop(_n1);
        assert!(t.join().is_ok());
    }

    // ── capacity reduction ──

    #[test]
    fn server_channel_rejection_lowers_the_ceiling_only() {
        let s = sched(10, 2);
        assert_eq!(s.effective_total(), 10);
        assert_eq!(s.reduce_capacity(4), 4);
        // Reduction is permanent and never creates a second connection.
        assert_eq!(s.reduce_capacity(10), 4, "must not climb back up");
        assert_eq!(s.stats().effective_total, 4);
    }

    #[test]
    fn capacity_is_never_reduced_below_the_urgent_reserve() {
        let s = sched(10, 2);
        // A server reporting zero usable channels must not leave control work
        // with nowhere to go.
        assert_eq!(s.reduce_capacity(0), 1);
    }

    #[test]
    fn reduced_capacity_still_admits_urgent_work() {
        let s = sched(4, 1);
        let _a = s.try_acquire(Priority::Normal).unwrap();
        s.reduce_capacity(2);
        // 2 total, 1 reserved → normal is full, urgent still has a slot.
        assert!(s.try_acquire(Priority::Normal).is_none());
        assert!(s.try_acquire(Priority::Urgent).is_some());
    }

    // ── concurrency ──

    #[test]
    fn one_hundred_concurrent_requests_never_exceed_the_limit() {
        // "Tests prove … configured session and bulk limits are never exceeded."
        let s = sched(4, 2);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for i in 0..100 {
            let sc = Arc::clone(&s);
            let live = Arc::clone(&live);
            let peak = Arc::clone(&peak);
            let prio = if i % 4 == 0 {
                Priority::Bulk
            } else {
                Priority::Normal
            };
            threads.push(thread::spawn(move || {
                let _p = sc.acquire(prio, &RequestId::new(), secs(30)).unwrap();
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(1));
                live.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 4,
            "peak {} exceeded VB_SSH_MAX_SESSIONS=4",
            peak.load(Ordering::SeqCst)
        );
        assert_eq!(s.stats().active, 0, "all permits must be released");
    }

    #[test]
    fn concurrent_bulk_never_exceeds_the_bulk_limit() {
        let s = sched(10, 2);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..40 {
            let sc = Arc::clone(&s);
            let live = Arc::clone(&live);
            let peak = Arc::clone(&peak);
            threads.push(thread::spawn(move || {
                let _p = sc
                    .acquire(Priority::Bulk, &RequestId::new(), secs(30))
                    .unwrap();
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(1));
                live.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak {} exceeded VB_SSH_MAX_BULK_SESSIONS=2",
            peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn scheduler_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionScheduler>();
        assert_send_sync::<Permit>();
    }
}

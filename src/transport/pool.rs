//! Endpoint identity and connection pooling (step 4 of the native plan).
//!
//! The design fixes what one pooled connection *is*:
//!
//! > Connections use an immutable key composed of:
//! > `host + port + user + complete jump route + SOCKS route + per-hop
//! > identities + per-hop HostKeyAlias values + security-relevant SSH options`
//! >
//! > GUI, deploy, daemon, and Spectre roles that resolve to the same key share
//! > one authenticated SSH Transport. Different endpoints use separate
//! > Transports. Profiles are always isolated, even if their endpoint keys are
//! > otherwise identical.
//! >
//! > Endpoint aliases are not guessed to be equivalent. Two differently
//! > resolved keys create two connections unless configuration resolution
//! > produces the same canonical key.
//!
//! [`EndpointKey`] is that composition, [`EndpointPool`] is the sharing, and
//! [`Endpoint`] carries the per-connection channel scheduler from
//! [`crate::transport::scheduler`] alongside the transport itself.
//!
//! # What is not here yet
//!
//! SOCKS routing and multi-hop jump routing are step 5, so `socks_route` and
//! all but the first `jump_route` hop are currently always empty. The fields
//! exist now because they belong to the key: adding them later is a change to
//! how the key is *built*, not to the key's shape, and no pooled connection
//! can outlive a change to its own identity.

#![allow(dead_code)] // consumed on Unix by the pooled transport daemon (commands::transport_daemon → server::run_with_pool). Windows builds have no daemon yet, so the pool stays un-referenced there.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Monotonically increasing counter stamped onto every newly-created
/// [`Endpoint`].
///
/// `EndpointPool::get_or_create` bumps this once per successful factory call
/// and writes the new value into the freshly-stored `Endpoint`. A stale
/// eviction callback captured the generation of the connection it ran on;
/// when it later asks the pool to drop that connection, the pool compares
/// the captured generation against the slot's current one. They differ when
/// a concurrent caller has already replaced the failed connection with a
/// fresh one — in which case the stale callback must not drop the
/// replacement.
static NEXT_ENDPOINT_GENERATION: AtomicU64 = AtomicU64::new(1);

use crate::config::Config;
use crate::transport::contract::{Deadline, RemoteTransport, RequestId, TransportError};
use crate::transport::scheduler::{Permit, Priority, SchedulerLimits, SessionScheduler};

// ─────────────────────────────── endpoint key ───────────────────────────────

/// Immutable identity of one authenticated SSH connection.
///
/// Every field is part of the identity in the sense that changing it can
/// change *which host you reach* or *under whose authority* — so two
/// connections differing in any field are genuinely two connections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointKey {
    /// Profiles never share, even with an otherwise identical key.
    pub profile: Option<String>,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    /// Complete jump route, nearest hop first. Step 5 populates beyond hop 0.
    pub jump_route: Vec<String>,
    /// SOCKS proxy route. Step 5.
    pub socks_route: Option<String>,
    /// Identity files, per hop in `jump_route` order.
    pub identities: Vec<String>,
    /// `HostKeyAlias` values, per hop. Empty until step 5 models them.
    pub host_key_aliases: Vec<String>,
    /// Security-relevant SSH options — which backend carries the traffic and
    /// which config file supplied the policy. `VB_DISABLE_CONTROL_MASTER` is
    /// deliberately absent: it is a path workaround, not a security switch,
    /// and treating it as identity would needlessly split a connection.
    pub security_options: Vec<String>,
}

impl EndpointKey {
    /// Build the key for an already role-resolved `host`.
    ///
    /// Role resolution lives in [`crate::config::RemoteRoles`]; callers pass
    /// the result (for example `roles.spectre_host(config.remote_host.as_deref())`)
    /// so that the four roles collapse onto one connection exactly when they
    /// name the same host — not because this function guessed that two
    /// different strings are aliases.
    pub fn from_config(config: &Config, host: &str) -> Self {
        let mut jump_route = Vec::new();
        if let Some(jump) = config.jump_host.as_deref().filter(|s| !s.is_empty()) {
            jump_route.push(match config.jump_user.as_deref() {
                Some(u) if !u.is_empty() => format!("{u}@{jump}"),
                _ => jump.to_string(),
            });
        }
        let identities = config
            .ssh_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|k| vec![k.to_string()])
            .unwrap_or_default();

        let mut security_options = vec![format!(
            "backend={}",
            config.ssh_backend.as_deref().unwrap_or("openssh")
        )];
        if let Some(cfg_path) = config.ssh_config.as_deref().filter(|s| !s.is_empty()) {
            security_options.push(format!("ssh_config={cfg_path}"));
        }

        Self {
            profile: config.profile.clone(),
            host: host.to_string(),
            port: config.ssh_port.unwrap_or(22),
            user: config.remote_user.clone().filter(|u| !u.is_empty()),
            jump_route,
            socks_route: None,
            identities,
            host_key_aliases: Vec::new(),
            security_options,
        }
    }

    /// Short, stable digest for logs.
    ///
    /// The design allows logs to record sanitized endpoints but never to leak
    /// identity paths or options, so diagnostics carry this rather than the
    /// full key.
    pub fn digest(&self) -> String {
        let canonical = format!("{self:?}");
        let h = <sha2::Sha256 as sha2::Digest>::digest(canonical.as_bytes());
        hex::encode(h)[..12].to_string()
    }

    /// Human-readable form that excludes identities and options.
    pub fn summary(&self) -> String {
        let profile = self
            .profile
            .as_deref()
            .map(|p| format!("profile={p} "))
            .unwrap_or_default();
        format!(
            "{profile}{}@{}:{}",
            self.user.as_deref().unwrap_or("-"),
            self.host,
            self.port
        )
    }
}

// ───────────────────────────────── endpoint ─────────────────────────────────

/// One pooled connection: a transport plus the scheduler that meters it.
pub struct Endpoint {
    pub key: EndpointKey,
    pub transport: Arc<dyn RemoteTransport>,
    pub scheduler: Arc<SessionScheduler>,
    /// Generation stamped when this `Endpoint` was stored in its slot. Used
    /// to make eviction conditional: a late callback from a request that
    /// already failed must not drop a replacement built concurrently.
    ///
    /// Held by `Arc` so callers can capture the generation cheaply (one
    /// `Arc::clone`) and the [`EndpointPool`] can read it without borrowing
    /// the whole `Endpoint` while it is evicting.
    pub generation: Arc<AtomicU64>,
}

/// Hand-written because neither `dyn RemoteTransport` nor
/// `SessionScheduler` (which holds a `Condvar`) is `Debug`. Reporting the
/// key digest and scheduler state is what a caller wants when a
/// `Result<Arc<Endpoint>, _>` is unwrapped.
impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("key", &self.key.summary())
            .field("digest", &self.key.digest())
            .field("scheduler", &self.scheduler.stats())
            .finish()
    }
}

impl Endpoint {
    /// Take a channel permit, bounded by the request deadline.
    pub fn acquire(
        &self,
        priority: Priority,
        request: &RequestId,
        deadline: Deadline,
    ) -> Result<Permit, TransportError> {
        self.scheduler.acquire(priority, request, deadline)
    }

    /// Read the generation stamped on this endpoint.
    ///
    /// Pair with [`EndpointPool::remove_if_matches`] to drop the connection
    /// only when the captured generation still describes the slot's current
    /// endpoint.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Creation state for a single key.
///
/// The mutex is what makes "one authentication" true: concurrent callers for
/// the same key share this slot, so exactly one of them runs the factory and
/// the rest block and then observe its result. Different keys have different
/// slots and therefore connect in parallel.
struct EndpointSlot {
    endpoint: Mutex<Option<Arc<Endpoint>>>,
    /// The scheduler outlives the individual endpoint.
    ///
    /// It is created once, when the slot is, and every endpoint built for
    /// this key shares that one instance. Reconnecting therefore keeps a
    /// single set of channel limits: an in-flight request holding a permit
    /// and a request served by the freshly-built replacement draw on the
    /// same budget instead of two independent ones.
    scheduler: Arc<SessionScheduler>,
}

impl EndpointSlot {
    fn new(limits: SchedulerLimits) -> Result<Self, TransportError> {
        Ok(Self {
            endpoint: Mutex::new(None),
            scheduler: SessionScheduler::new(limits)?,
        })
    }

    fn get_or_create<F>(
        &self,
        key: EndpointKey,
        factory: F,
    ) -> Result<Arc<Endpoint>, TransportError>
    where
        F: FnOnce() -> Result<Arc<dyn RemoteTransport>, TransportError>,
    {
        let mut guard = self.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard.as_ref() {
            return Ok(Arc::clone(existing));
        }
        let transport = factory()?;
        // Bump the global counter once per successful factory call. Reading
        // the previous value and storing the next gives every newly-built
        // `Endpoint` a strictly larger generation than its predecessor.
        let generation = Arc::new(AtomicU64::new(
            NEXT_ENDPOINT_GENERATION.fetch_add(1, Ordering::Relaxed),
        ));
        let endpoint = Arc::new(Endpoint {
            key,
            transport,
            scheduler: Arc::clone(&self.scheduler),
            generation,
        });
        *guard = Some(Arc::clone(&endpoint));
        Ok(endpoint)
    }

    /// Drop the connection but keep the slot — and therefore the scheduler.
    ///
    /// This is what conditional eviction does. Removing the whole slot would
    /// take its scheduler with it, and the next `get_or_create` would build a
    /// fresh one, splitting the key's channel budget across two schedulers.
    fn clear(&self) {
        *self.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn has_endpoint(&self) -> bool {
        self.endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    ///
    /// Used by [`EndpointPool::remove_if_matches`] and by the regression
    /// test that proves a stale eviction callback does not drop a
    /// replacement.
    fn current_generation(&self) -> Option<u64> {
        self.endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|ep| ep.generation())
    }
}

// ─────────────────────────────────── pool ───────────────────────────────────

/// Shared connections, keyed by [`EndpointKey`].
pub struct EndpointPool {
    slots: Mutex<HashMap<EndpointKey, Arc<EndpointSlot>>>,
}

impl EndpointPool {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Return the pooled connection for `key`, creating it at most once.
    ///
    /// `factory` is called only when no connection exists for this key, and
    /// never by two callers at once. A failed creation removes the slot so the
    /// next caller retries rather than caching the failure forever — a
    /// transient network error on first connect must not poison the profile.
    pub fn get_or_create<F>(
        &self,
        key: EndpointKey,
        limits: SchedulerLimits,
        factory: F,
    ) -> Result<Arc<Endpoint>, TransportError>
    where
        F: FnOnce() -> Result<Arc<dyn RemoteTransport>, TransportError>,
    {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = slots.get(&key) {
                Arc::clone(existing)
            } else {
                let created = Arc::new(EndpointSlot::new(limits)?);
                slots.insert(key.clone(), Arc::clone(&created));
                created
            }
        };
        match slot.get_or_create(key.clone(), factory) {
            Ok(endpoint) => Ok(endpoint),
            Err(e) => {
                let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                // Drop the empty slot, but only if it is still the one we
                // created — a concurrent success may have replaced it.
                let still_empty = slots
                    .get(&key)
                    .and_then(|s| s.endpoint.lock().ok())
                    .map(|g| g.is_none())
                    .unwrap_or(false);
                if still_empty {
                    slots.remove(&key);
                }
                Err(e)
            }
        }
    }

    /// The pooled connection for `key`, if one exists.
    pub fn get(&self, key: &EndpointKey) -> Option<Arc<Endpoint>> {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .and_then(|s| s.endpoint.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Drop a connection without stopping the others. Used by `tunnel stop`
    /// and by reconnect in step 6.
    pub fn remove(&self, key: &EndpointKey) -> bool {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
            .is_some()
    }

    /// Conditionally drop the connection for `key`, only if its current
    /// generation matches `expected_gen`.
    ///
    /// A request captured `expected_gen` when it began. If the connection
    /// failed and was already replaced by a fresh one before the eviction
    /// callback ran, the slot's current generation is newer and this call
    /// eviction is a no-op: the slot keeps its scheduler, so the next
    /// `get_or_create` reconnects onto the same channel budget instead of
    /// a reset one.
    ///
    /// This is the contract the IPC server relies on. An unconditional
    /// [`remove`] is for `tunnel stop` only — it would otherwise race with
    /// reconnect in step 6.
    pub fn remove_if_matches(&self, key: &EndpointKey, expected_gen: u64) -> bool {
        // One lock for the whole check-then-clear. `EndpointPool::slots` is a
        // plain `Mutex`: a second `lock()` on this thread would deadlock
        // because the first guard is still alive, so the comparison and the
        // eviction must happen inside a single critical section.
        let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let matches = slots
            .get(key)
            .map(|slot| slot.current_generation() == Some(expected_gen))
            .unwrap_or(false);
        if !matches {
            return false;
        }
        let Some(slot) = slots.get(key) else {
            return false;
        };
        slot.clear();
        true
    }

    /// Number of keys with a live connection.
    ///
    /// An evicted key keeps its slot (and scheduler) so a reconnect reuses
    /// the same budget, so this counts slots that currently hold an
    /// endpoint rather than every slot in the map.
    pub fn len(&self) -> usize {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|s| s.has_endpoint())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keys that currently hold a live connection.
    pub fn keys(&self) -> Vec<EndpointKey> {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, s)| s.has_endpoint())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Drop every connection. `tunnel stop` uses this after the graceful
    /// window closes.
    pub fn clear(&self) {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl Default for EndpointPool {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────── tests ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::FakeTransport;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn config_for(host: &str) -> Config {
        Config {
            profile: None,
            remote_host: Some(host.into()),
            remote_user: None,
            port: 65432,
            port_explicit: false,
            jump_host: None,
            jump_user: None,
            ssh_port: Some(22),
            ssh_key: None,
            ssh_config: None,
            ssh_backend: None,
            disable_control_master: false,
            timeout: 30,
            read_timeout: 120,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: 8,
            ssh_max_sessions: 10,
            ssh_max_bulk_sessions: 2,
            ssh_reconnect_max_attempts: 8,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 30,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 10,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: Default::default(),
            transport_daemon_socket: None,
            transport_daemon_token: None,
            allow_cross_user_daemon: false,
        }
    }

    fn key(host: &str) -> EndpointKey {
        EndpointKey::from_config(&config_for(host), host)
    }

    /// A factory that always succeeds. Returns a `Result` because that is what
    /// [`EndpointPool::get_or_create`] accepts — a real factory performs an
    /// authenticated handshake here.
    fn ok_transport() -> Result<Arc<dyn RemoteTransport>, TransportError> {
        Ok(Arc::new(FakeTransport::ok()))
    }

    // ── key composition ──

    #[test]
    fn identical_resolutions_produce_one_key() {
        // "GUI, deploy, daemon, and Spectre roles that resolve to the same key
        // share one authenticated SSH Transport."
        let cfg = config_for("eda-1");
        let fb = cfg.remote_host.as_deref();
        let keys = [
            EndpointKey::from_config(&cfg, &cfg.roles.gui_host(fb)),
            EndpointKey::from_config(&cfg, &cfg.roles.deploy_host(fb)),
            EndpointKey::from_config(&cfg, &cfg.roles.daemon_host(fb)),
            EndpointKey::from_config(&cfg, &cfg.roles.spectre_host(fb)),
        ];
        for k in &keys[1..] {
            assert_eq!(*k, keys[0], "all four roles resolve to eda-1");
        }
    }

    #[test]
    fn a_different_role_host_is_a_different_endpoint() {
        let mut cfg = config_for("eda-1");
        cfg.roles.spectre_host = Some("hpc-7".into());
        let fb = cfg.remote_host.as_deref();
        assert_ne!(
            EndpointKey::from_config(&cfg, &cfg.roles.gui_host(fb)),
            EndpointKey::from_config(&cfg, &cfg.roles.spectre_host(fb))
        );
    }

    #[test]
    fn aliases_are_not_guessed_to_be_equivalent() {
        // "Endpoint aliases are not guessed to be equivalent."
        assert_ne!(key("eda-1"), key("eda-1.corp.example.com"));
        assert_ne!(key("eda-1"), key("10.0.0.7"));
    }

    #[test]
    fn every_identity_component_changes_the_key() {
        let base = key("eda-1");
        let mut port = config_for("eda-1");
        port.ssh_port = Some(2222);
        assert_ne!(EndpointKey::from_config(&port, "eda-1"), base);

        let mut user = config_for("eda-1");
        user.remote_user = Some("cadence".into());
        assert_ne!(EndpointKey::from_config(&user, "eda-1"), base);

        let mut jump = config_for("eda-1");
        jump.jump_host = Some("bastion".into());
        assert_ne!(EndpointKey::from_config(&jump, "eda-1"), base);

        let mut ident = config_for("eda-1");
        ident.ssh_key = Some("/home/u/.ssh/id_ed25519".into());
        assert_ne!(EndpointKey::from_config(&ident, "eda-1"), base);

        let mut backend = config_for("eda-1");
        backend.ssh_backend = Some("native".into());
        assert_ne!(EndpointKey::from_config(&backend, "eda-1"), base);
    }

    #[test]
    fn profiles_never_share_an_endpoint() {
        // "Profiles are always isolated, even if their endpoint keys are
        // otherwise identical."
        let mut a = config_for("eda-1");
        a.profile = Some("tapeout".into());
        let mut b = config_for("eda-1");
        b.profile = Some("dev".into());
        assert_ne!(
            EndpointKey::from_config(&a, "eda-1"),
            EndpointKey::from_config(&b, "eda-1")
        );
        // And a profiled config never shares with an unprofiled one.
        let plain = config_for("eda-1");
        assert_ne!(
            EndpointKey::from_config(&a, "eda-1"),
            EndpointKey::from_config(&plain, "eda-1")
        );
    }

    #[test]
    fn control_master_is_not_part_of_the_identity() {
        // It is a path workaround, not a security switch: splitting a pooled
        // connection over it would be pure waste.
        let mut cfg = config_for("eda-1");
        cfg.disable_control_master = true;
        assert_eq!(EndpointKey::from_config(&cfg, "eda-1"), key("eda-1"));
    }

    #[test]
    fn jump_route_records_user_and_host_together() {
        let mut cfg = config_for("eda-1");
        cfg.jump_host = Some("bastion".into());
        cfg.jump_user = Some("jumpuser".into());
        let k = EndpointKey::from_config(&cfg, "eda-1");
        assert_eq!(k.jump_route, vec!["jumpuser@bastion".to_string()]);
    }

    #[test]
    fn digest_and_summary_avoid_leaking_identity_paths() {
        let mut cfg = config_for("eda-1");
        cfg.ssh_key = Some("/home/u/.ssh/secret_key".into());
        let k = EndpointKey::from_config(&cfg, "eda-1");
        assert!(!k.summary().contains("secret_key"));
        assert!(!k.digest().contains("secret_key"));
        assert!(k.summary().contains("eda-1"));
        assert_eq!(k.digest().len(), 12);
        // Stable across calls, and different per key.
        assert_eq!(k.digest(), k.digest());
        assert_ne!(k.digest(), key("eda-2").digest());
    }

    // ── pooling ──

    #[test]
    fn the_same_key_returns_the_same_connection() {
        let pool = EndpointPool::new();
        let a = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let b = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b), "must be one pooled connection");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn different_keys_get_different_connections() {
        let pool = EndpointPool::new();
        let a = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let b = pool
            .get_or_create(
                key("eda-2"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn concurrent_callers_authenticate_once() {
        // "Tests prove that 100 concurrent short commands use one
        // authentication." The factory is the stand-in for that handshake.
        let pool = Arc::new(EndpointPool::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..32 {
            let p = Arc::clone(&pool);
            let c = Arc::clone(&calls);
            threads.push(thread::spawn(move || {
                let ep = p
                    .get_or_create(key("eda-1"), SchedulerLimits::default_limits(), || {
                        c.fetch_add(1, Ordering::SeqCst);
                        ok_transport()
                    })
                    .unwrap();
                Arc::as_ptr(&ep) as usize
            }));
        }
        let pointers: Vec<usize> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one handshake");
        let first = pointers[0];
        assert!(
            pointers.iter().all(|p| *p == first),
            "every caller must get the same connection"
        );
    }

    #[test]
    fn a_failed_creation_is_not_cached() {
        let pool = EndpointPool::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let err = pool
            .get_or_create(key("eda-1"), SchedulerLimits::default_limits(), || {
                a.fetch_add(1, Ordering::SeqCst);
                Err(TransportError::ConnectionFailed("first attempt".into()))
            })
            .expect_err("first attempt fails");
        assert!(matches!(err, TransportError::ConnectionFailed(_)));
        // The slot must be gone, so the next caller really retries.
        assert_eq!(pool.len(), 0);
        let ep = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&ep, &pool.get(&key("eda-1")).unwrap()));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_invalid_scheduler_limit_fails_without_caching() {
        let pool = EndpointPool::new();
        let bad = SchedulerLimits {
            total: 1,
            bulk: 1,
            urgent_reserve: 1,
            bulk_starvation_grace: SchedulerLimits::DEFAULT_BULK_STARVATION_GRACE,
        };
        assert!(pool.get_or_create(key("eda-1"), bad, ok_transport).is_err());
        assert_eq!(pool.len(), 0, "a half-built endpoint must not be pooled");
    }

    #[test]
    fn remove_and_clear_drop_connections() {
        let pool = EndpointPool::new();
        pool.get_or_create(
            key("eda-1"),
            SchedulerLimits::default_limits(),
            ok_transport,
        )
        .unwrap();
        pool.get_or_create(
            key("eda-2"),
            SchedulerLimits::default_limits(),
            ok_transport,
        )
        .unwrap();
        assert_eq!(pool.len(), 2);
        assert!(pool.remove(&key("eda-1")));
        assert!(!pool.remove(&key("eda-1")), "already gone");
        assert_eq!(pool.len(), 1);
        assert!(pool.get(&key("eda-1")).is_none());
        pool.clear();
        assert!(pool.is_empty());
    }

    #[test]
    fn each_endpoint_carries_its_own_scheduler() {
        let pool = EndpointPool::new();
        let a = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let b = pool
            .get_or_create(
                key("eda-2"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        // Acquiring on one endpoint must not consume the other's capacity.
        let _held: Vec<_> = (0..3)
            .map(|_| {
                a.acquire(
                    Priority::Normal,
                    &RequestId::new(),
                    Deadline::from_now(std::time::Duration::from_secs(5)),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(a.scheduler.stats().active, 3);
        assert_eq!(b.scheduler.stats().active, 0);
    }

    #[test]
    fn pool_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EndpointPool>();
        assert_send_sync::<EndpointKey>();
    }

    // ── generation-based eviction ──

    #[test]
    fn every_endpoint_carries_a_strictly_increasing_generation() {
        // The generation is the safety net against late eviction callbacks,
        // so the only invariant that matters here is "newer endpoint ⇒ larger
        // generation" across calls.
        let pool = EndpointPool::new();
        let a = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let g_a = a.generation();
        pool.remove(&key("eda-1"));
        let b = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        assert!(
            b.generation() > g_a,
            "each rebuild must bump the generation"
        );
    }

    #[test]
    fn a_late_eviction_does_not_drop_a_concurrent_replacement() {
        // A request captured `gen_a` from the endpoint it was using. By the
        // time the connection-error callback runs, a concurrent request has
        // replaced that endpoint with `gen_b`. The callback's remove must be a
        // no-op so the next caller still has a usable connection.
        let pool = EndpointPool::new();
        let first = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let gen_a = first.generation();

        // Simulate the rebuild path: drop the slot, then re-create. The
        // exact same effect would happen when a real factory errors and the
        // pool re-tries on the next request.
        pool.remove(&key("eda-1"));
        let replacement = pool
            .get_or_create(
                key("eda-1"),
                SchedulerLimits::default_limits(),
                ok_transport,
            )
            .unwrap();
        let gen_b = replacement.generation();
        assert!(gen_b > gen_a);

        // The late callback fires with the stale generation. It must NOT
        // remove the replacement; otherwise the next request would retry the
        // factory instead of reusing the freshly-built connection.
        assert!(
            !pool.remove_if_matches(&key("eda-1"), gen_a),
            "stale generation must not evict the replacement"
        );
        assert_eq!(pool.len(), 1, "the replacement must still be pooled");
        let observed = pool.get(&key("eda-1")).unwrap();
        assert!(
            Arc::ptr_eq(&observed, &replacement),
            "the survivor must be the freshly-built endpoint"
        );
        assert_eq!(observed.generation(), gen_b);

        // The current generation evicts as expected.
        assert!(pool.remove_if_matches(&key("eda-1"), gen_b));
        assert!(pool.is_empty());
    }

    #[test]
    fn remove_if_matches_returns_false_when_the_slot_is_empty() {
        // No slot at all: the conditional remove must be a no-op, never panic
        // on a missing entry, and never synthesise a slot.
        let pool = EndpointPool::new();
        assert!(!pool.remove_if_matches(&key("eda-1"), 42));
        assert!(pool.is_empty());
    }

    #[test]
    fn evicting_an_endpoint_keeps_its_scheduler_and_budget() {
        // #79 ③: cleaning up a failed instance must not rebuild the
        // endpoint's scheduler. The scheduler lives on the slot, so the
        // endpoint built after an eviction shares one channel budget with
        // requests that are still in flight — never two independent ones.
        let pool = EndpointPool::new();
        let limits = SchedulerLimits::default_limits();
        let first = pool
            .get_or_create(key("eda-1"), limits, ok_transport)
            .unwrap();
        // Stand in for an in-flight request: one permit is checked out and
        // will only be returned after the eviction.
        let permit = first
            .acquire(
                Priority::Normal,
                &RequestId::new(),
                Deadline::from_now(std::time::Duration::from_secs(5)),
            )
            .unwrap();
        let first_scheduler = Arc::clone(&first.scheduler);
        let gen_a = first.generation();
        assert_eq!(first_scheduler.stats().active, 1);

        // Conditional eviction (the path the IPC server takes on a
        // connection-level failure) drops the instance but keeps the slot.
        assert!(
            pool.remove_if_matches(&key("eda-1"), gen_a),
            "the current generation must evict"
        );
        assert!(pool.is_empty(), "no live endpoint survives");

        let second = pool
            .get_or_create(key("eda-1"), limits, ok_transport)
            .unwrap();

        assert!(
            Arc::ptr_eq(&first_scheduler, &second.scheduler),
            "the reconnect must reuse the same scheduler, not build a new one"
        );
        assert!(
            second.generation() > gen_a,
            "the replacement is a new instance"
        );
        assert_eq!(
            second.scheduler.stats().active,
            1,
            "the in-flight permit is charged to the shared budget — it must not reset"
        );

        // Releasing the old permit returns capacity to the one scheduler
        // both endpoints share.
        drop(permit);
        assert_eq!(first_scheduler.stats().active, 0);
        assert_eq!(second.scheduler.stats().active, 0);
    }
}

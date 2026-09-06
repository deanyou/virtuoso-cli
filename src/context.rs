//! CommandContext: the single resolved configuration for one invocation.
//!
//! P0-A: migrated command families receive their [`Config`] here instead of
//! re-reading the environment (`Config::from_env()`). `main()` resolves the
//! effective selection exactly once via `target::resolve`, builds a
//! `CommandContext`, and hands the same immutable context to every migrated
//! command — so host/port/backend/etc. are parsed once per process and there is
//! no second, potentially divergent env re-read.
//!
//! The context also carries the target identity (`target_id`) and a
//! deterministic [`Config::digest`] so downstream layers (tunnel status drift
//! detection, daemon Hello validation, F05) can verify they are talking to the
//! intended endpoint rather than trusting parsed values alone.

use crate::config::Config;
use crate::error::{Result, VirtuosoError};

/// Immutable resolved configuration plus target identity for one invocation.
#[derive(Debug, Clone)]
pub struct CommandContext {
    config: Config,
    target_id: Option<String>,
    config_digest: String,
}

impl CommandContext {
    /// Build a context from a resolved target (name + config).
    pub fn from_resolved(resolved: &crate::target::resolve::ResolvedTarget) -> Result<Self> {
        Self::new(resolved.config.clone(), resolved.name.clone())
    }

    /// Build a context from a config and an optional target name.
    pub fn new(config: Config, target_id: Option<String>) -> Result<Self> {
        let config_digest = config.digest();
        Ok(Self {
            config,
            target_id,
            config_digest,
        })
    }

    /// The resolved configuration. Migrated commands must use this instead of
    /// `Config::from_env()`.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The selected target name, if the selection was target-backed.
    pub fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    /// Deterministic digest of the resolved config (identity check, F05).
    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }

    /// Validate that a session belongs to this context's target.
    ///
    /// Single source of truth for target-ownership validation (report F05).
    /// Distinguishes both the host and the remote bridge port, so two targets
    /// on the same compute node with different bridge ports are not conflated.
    /// Target-less (legacy env) selections skip the check entirely.
    pub fn validate_session_ownership(&self, session: &crate::models::SessionInfo) -> Result<()> {
        self.validate_endpoint_ownership("session", &session.id, &session.host, session.port)
    }

    /// Validate that a tunnel state belongs to this context's target.
    /// `state.port` is the local forward port; the ownership discriminator is
    /// the *remote* bridge port actually used: `remote_bridge_port` (the
    /// discovered daemon port, recorded by attach) or `attached_remote_port`,
    /// falling back to `port` only for legacy state files. The bridge-port arm
    /// only fires when the target's port is an explicit constraint.
    pub fn validate_tunnel_ownership(&self, state: &crate::models::TunnelState) -> Result<()> {
        let remote_port = state
            .remote_bridge_port
            .or(state.attached_remote_port)
            .unwrap_or(state.port);
        self.validate_endpoint_ownership(
            "tunnel",
            &state.port.to_string(),
            &state.remote_host,
            remote_port,
        )
    }

    /// Shared host + bridge-port ownership rule.
    ///
    /// The bridge-port arm applies ONLY when the target's port is an explicit
    /// user constraint (`Config::port_explicit`). A default (hash-of-USER)
    /// port is not an endpoint constraint — daemons bind OS-assigned ports —
    /// so it must never reject a discovered session or tunnel. Host identity is
    /// always enforced in target mode; target-less (legacy env) selections
    /// skip the whole check.
    fn validate_endpoint_ownership(
        &self,
        kind: &str,
        id: &str,
        host: &str,
        port: u16,
    ) -> Result<()> {
        if self.target_id.is_none() {
            return Ok(());
        }
        let target_host = self.config.remote_host.as_deref().unwrap_or("");
        if !target_host.is_empty() && host != target_host {
            return Err(VirtuosoError::Config(format!(
                "{kind} '{id}' belongs to host '{host}' but target '{}' resolves to '{target_host}'; \
                 refusing to reuse the wrong {kind} (run `vcli session list`)",
                self.target_id().unwrap_or("unknown")
            )));
        }
        // Same-host discrimination: two targets on one compute node differ by
        // their remote bridge port, so an explicitly configured port mismatch
        // is an ownership violation even when the host matches. A non-explicit
        // (default) port is not a constraint and is never checked here.
        if self.config.port_explicit && self.config.port != 0 && port != self.config.port {
            return Err(VirtuosoError::Config(format!(
                "{kind} '{id}' is on bridge port {port} but target '{}' uses port {}; \
                 refusing to reuse the wrong {kind}",
                self.target_id().unwrap_or("unknown"),
                self.config.port
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionInfo;
    use serial_test::serial;

    fn cfg_with(host: &str, port: u16) -> Config {
        // Build via env_with_profile with scoped vars, then strip profile.
        std::env::set_var("VB_REMOTE_HOST_targettest", host);
        std::env::set_var("VB_PORT_targettest", port.to_string());
        let mut c = Config::from_env_with_profile(Some("targettest")).unwrap();
        std::env::remove_var("VB_REMOTE_HOST_targettest");
        std::env::remove_var("VB_PORT_targettest");
        c.profile = None;
        c
    }

    fn session_with(host: &str, port: u16) -> SessionInfo {
        SessionInfo {
            id: "sess-test".into(),
            port,
            pid: 42,
            host: host.into(),
            user: "user1".into(),
            created: "2026-01-01T00:00:00Z".into(),
            daemon_user: None,
            daemon_version: None,
        }
    }

    #[serial]
    #[test]
    fn digest_is_stable_and_identifies_host() {
        let c1 = cfg_with("compute-a", 30001);
        let c2 = cfg_with("compute-b", 30001);
        let c1b = cfg_with("compute-a", 30001);
        assert_eq!(c1.digest(), c1b.digest());
        assert_ne!(c1.digest(), c2.digest());
    }

    #[serial]
    #[test]
    fn digest_changes_when_bridge_port_changes() {
        let c1 = cfg_with("compute-eda-42", 30001);
        let c2 = cfg_with("compute-eda-42", 30002);
        assert_ne!(c1.digest(), c2.digest());
    }

    #[serial]
    #[test]
    fn ownership_check_passes_on_matching_host_and_port() {
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        assert!(ctx
            .validate_session_ownership(&session_with("compute-eda-42", 30001))
            .is_ok());
    }

    #[serial]
    #[test]
    fn ownership_check_rejects_wrong_host_session() {
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let err = ctx
            .validate_session_ownership(&session_with("compute-eda-99", 30001))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("refusing to reuse the wrong session"));
    }

    #[serial]
    #[test]
    fn ownership_check_rejects_same_host_different_bridge_port() {
        // prod/test share a compute node; only the bridge port distinguishes them.
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let err = ctx
            .validate_session_ownership(&session_with("compute-eda-42", 30002))
            .unwrap_err();
        assert!(err.to_string().contains("is on bridge port 30002"));
    }

    #[serial]
    #[test]
    fn tunnel_ownership_check_rejects_same_host_different_bridge_port() {
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let state = crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: 40001,
            pid: 0,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: Some("prod".into()),
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: Some(crate::models::TUNNEL_MODE_ATTACHED.into()),
            attached_remote_port: Some(30002),
            remote_bridge_port: Some(30002),
            attached_session_id: None,
        };
        let err = ctx.validate_tunnel_ownership(&state).unwrap_err();
        assert!(err.to_string().contains("is on bridge port 30002"));
    }

    #[serial]
    #[test]
    fn tunnel_ownership_pass_with_local_port_fallback_remote_fixed() {
        // `tunnel start` may fall back to a *local* forward port when the
        // configured bridge port is taken locally, while the remote endpoint
        // stays fixed at the target's bridge port. Ownership must be judged on
        // the remote port, so this freshly-created state is NOT self-rejected.
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let state = crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: 40001, // local fallback — differs from the bridge port
            pid: 0,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: Some("prod".into()),
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: Some(crate::models::TUNNEL_MODE_DEPLOYED.into()),
            attached_remote_port: None,
            remote_bridge_port: Some(30001), // fixed remote bridge port
            attached_session_id: None,
        };
        assert!(ctx.validate_tunnel_ownership(&state).is_ok());
    }

    #[serial]
    #[test]
    fn tunnel_ownership_rejects_local_fallback_on_wrong_remote_port() {
        // Same as above but the recorded remote bridge port belongs to another
        // target — must be rejected even though the local port matches nothing
        // meaningful.
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let state = crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: 30001,
            pid: 0,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: Some("prod".into()),
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: Some(crate::models::TUNNEL_MODE_DEPLOYED.into()),
            attached_remote_port: None,
            remote_bridge_port: Some(30002),
            attached_session_id: None,
        };
        let err = ctx.validate_tunnel_ownership(&state).unwrap_err();
        assert!(err.to_string().contains("is on bridge port 30002"));
    }

    #[serial]
    #[test]
    fn tunnel_ownership_legacy_state_uses_port_when_no_remote_port() {
        // v1/v2 legacy state files have neither remote_bridge_port nor
        // attached_remote_port; the remote port is then identical to `port`.
        let ctx =
            CommandContext::new(cfg_with("compute-eda-42", 30001), Some("prod".into())).unwrap();
        let state = crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: 30001,
            pid: 0,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: Some("prod".into()),
            backend: None,
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
            remote_bridge_port: None,
            attached_session_id: None,
        };
        assert!(ctx.validate_tunnel_ownership(&state).is_ok());
    }

    #[serial]
    #[test]
    fn ownership_check_skipped_without_target() {
        let ctx = CommandContext::new(cfg_with("compute-eda-42", 30001), None).unwrap();
        assert!(ctx
            .validate_session_ownership(&session_with("compute-eda-99", 30001))
            .is_ok());
    }

    /// A target config whose port is NOT an explicit constraint (mirrors a
    /// target whose `port:` field is absent → hash-of-USER default). Built as
    /// a literal rather than via env so ambient `VB_PORT` cannot leak in.
    fn cfg_with_default_port(host: &str) -> Config {
        let mut c = cfg_with(host, 65013);
        c.port_explicit = false;
        c
    }

    #[serial]
    #[test]
    fn ownership_default_port_is_not_a_constraint_same_host() {
        // The daemon binds an OS-assigned port; a default cfg.port must never
        // reject a discovered session that happens to differ from it.
        let ctx = CommandContext::new(cfg_with_default_port("compute-eda-42"), Some("prod".into()))
            .unwrap();
        assert!(ctx
            .validate_session_ownership(&session_with("compute-eda-42", 41234))
            .is_ok());
    }

    #[serial]
    #[test]
    fn ownership_default_port_still_enforces_host() {
        let ctx = CommandContext::new(cfg_with_default_port("compute-eda-42"), Some("prod".into()))
            .unwrap();
        let err = ctx
            .validate_session_ownership(&session_with("compute-eda-99", 41234))
            .unwrap_err();
        assert!(err.to_string().contains("belongs to host 'compute-eda-99'"));
    }

    #[serial]
    #[test]
    fn tunnel_ownership_default_port_is_not_a_constraint() {
        let ctx = CommandContext::new(cfg_with_default_port("compute-eda-42"), Some("prod".into()))
            .unwrap();
        let state = crate::models::TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: 41234,
            pid: 0,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: Some("prod".into()),
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: Some(crate::models::TUNNEL_MODE_ATTACHED.into()),
            attached_remote_port: Some(41234),
            remote_bridge_port: Some(41234),
            attached_session_id: None,
        };
        assert!(ctx.validate_tunnel_ownership(&state).is_ok());
    }
}

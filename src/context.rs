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
    /// When a target is selected, a session recorded against a different host
    /// is an ownership violation — switching targets must not silently reuse
    /// the wrong session (report F05: "切换目标不复用错误 session").
    /// Target-less (legacy env) selections skip the check.
    pub fn validate_session_ownership(&self, session: &crate::models::SessionInfo) -> Result<()> {
        if self.target_id.is_none() {
            return Ok(());
        }
        let target_host = self.config.remote_host.as_deref().unwrap_or("");
        if !target_host.is_empty() && session.host != target_host {
            return Err(VirtuosoError::Config(format!(
                "session '{}' belongs to host '{}' but target '{}' resolves to '{}'; \
                 refusing to reuse the wrong session (run `vcli session list`)",
                session.id,
                session.host,
                self.target_id().unwrap_or("unknown"),
                target_host
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

    fn cfg_with_host(host: &str) -> Config {
        // Build via env_with_profile with a scoped var, then strip profile.
        std::env::set_var("VB_REMOTE_HOST_targettest", host);
        let mut c = Config::from_env_with_profile(Some("targettest")).unwrap();
        std::env::remove_var("VB_REMOTE_HOST_targettest");
        c.profile = None;
        c
    }

    fn session(host: &str) -> SessionInfo {
        SessionInfo {
            id: "sess-test".into(),
            port: 30001,
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
        let c1 = cfg_with_host("compute-a");
        let c2 = cfg_with_host("compute-b");
        let c1b = cfg_with_host("compute-a");
        assert_eq!(c1.digest(), c1b.digest());
        assert_ne!(c1.digest(), c2.digest());
    }

    #[serial]
    #[test]
    fn ownership_check_passes_on_matching_host() {
        let ctx =
            CommandContext::new(cfg_with_host("compute-eda-42"), Some("prod".into())).unwrap();
        assert!(ctx
            .validate_session_ownership(&session("compute-eda-42"))
            .is_ok());
    }

    #[serial]
    #[test]
    fn ownership_check_rejects_wrong_host_session() {
        let ctx =
            CommandContext::new(cfg_with_host("compute-eda-42"), Some("prod".into())).unwrap();
        let err = ctx
            .validate_session_ownership(&session("compute-eda-99"))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("refusing to reuse the wrong session"));
    }

    #[serial]
    #[test]
    fn ownership_check_skipped_without_target() {
        let ctx = CommandContext::new(cfg_with_host("compute-eda-42"), None).unwrap();
        assert!(ctx
            .validate_session_ownership(&session("compute-eda-99"))
            .is_ok());
    }
}

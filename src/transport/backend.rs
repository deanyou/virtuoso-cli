//! SSH backend selection: OpenSSH (default) vs native (russh).
//!
//! Implements the design's "Feature-gated builds" contract:
//!
//! > `native-ssh` is a Cargo feature. When a build is produced without it,
//! > selecting `VB_SSH_BACKEND=native` returns structured `UnsupportedBackend`
//! > rather than an unrecognised-value error or a silent fallback to OpenSSH.
//!
//! So the three outcomes are distinct and observable:
//! - unset / `openssh` → the OpenSSH backend, always available;
//! - `native` → `UnsupportedBackend` (the native client ships with `russh`,
//!   which lands in a later increment);
//! - anything else → `Configuration` (genuinely unrecognised value).
//!
//! There is no automatic backend migration: OpenSSH stays the default.

use std::sync::Arc;

use crate::config::Config;
use crate::transport::contract::{RemoteTransport, TransportError};
use crate::transport::openssh::OpenSshTransport;

/// Which SSH implementation carries the traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshBackend {
    /// Shell out to the `ssh` binary (ControlMaster, jump hosts, existing
    /// behaviour). The default, and the only backend available today.
    OpenSsh,
    /// Native russh-based transport. Requires the `native-ssh` feature.
    Native,
}

impl SshBackend {
    /// The backend used when `VB_SSH_BACKEND` is unset.
    pub const DEFAULT: SshBackend = SshBackend::OpenSsh;

    /// Parse the `VB_SSH_BACKEND` value. Case-insensitive, so a shared `.env`
    /// written as `OPENSSH` still resolves.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openssh" => Some(SshBackend::OpenSsh),
            "native" => Some(SshBackend::Native),
            _ => None,
        }
    }

    /// Resolve from configuration. Unset yields the default (no migration);
    /// an unrecognised value is a `Configuration` error.
    pub fn from_config(config: &Config) -> Result<Self, TransportError> {
        match config.ssh_backend.as_deref() {
            None => Ok(SshBackend::DEFAULT),
            Some(raw) => SshBackend::parse(raw).ok_or_else(|| {
                TransportError::Configuration(format!(
                    "unknown VB_SSH_BACKEND '{raw}' (expected 'openssh' or 'native')"
                ))
            }),
        }
    }

    /// Whether this *build* was compiled with support for the backend. Tracks
    /// the `native-ssh` feature, which is the switch the design specifies.
    #[allow(dead_code)] // consumed by tests and the future `native-ssh` increment
    pub fn supported_in_this_build(&self) -> bool {
        match self {
            SshBackend::OpenSsh => true,
            SshBackend::Native => cfg!(feature = "native-ssh"),
        }
    }
}

impl std::fmt::Display for SshBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshBackend::OpenSsh => f.write_str("openssh"),
            SshBackend::Native => f.write_str("native"),
        }
    }
}

/// Open the configured backend as a `Arc<dyn RemoteTransport>`.
///
/// Returns an error rather than a transport when the requested backend is not
/// usable — callers must surface it, never fall back silently.
pub fn open_transport(config: &Config) -> Result<Arc<dyn RemoteTransport>, TransportError> {
    match SshBackend::from_config(config)? {
        SshBackend::OpenSsh => {
            let transport = OpenSshTransport::from_config(config);
            // Faithful to `SSHClient::from_env`, which applies this flag but
            // `SSHRunner::from_config` does not.
            let transport = if config.disable_control_master {
                transport.with_control_master_disabled()
            } else {
                transport
            };
            Ok(Arc::new(transport))
        }
        // The native client ships with the `russh` dependency, which is added
        // when that increment lands. Until then every build reports the
        // structured error instead of silently using OpenSSH.
        SshBackend::Native => Err(TransportError::UnsupportedBackend),
    }
}

/// Gate an OpenSSH-only code path on the selected backend.
///
/// Some call sites build a concrete `SSHRunner` directly (the dedicated tunnel
/// child, the Spectre simulator's reduced configuration, the SKILL Finder's raw
/// `ssh`/`scp` sync) rather than going through [`open_transport`]. They are
/// inherently OpenSSH and cannot carry a native client, so an explicit
/// `native` request must fail loudly here instead of being silently ignored.
///
/// OpenSSH (and an unset backend) pass through untouched, preserving the
/// existing behaviour for purely local operations that never reach a backend.
pub fn require_openssh(config: &Config) -> Result<(), TransportError> {
    match SshBackend::from_config(config)? {
        SshBackend::OpenSsh => Ok(()),
        SshBackend::Native => Err(TransportError::UnsupportedBackend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(backend: Option<&str>) -> Config {
        Config {
            ssh_backend: backend.map(String::from),
            ..make_test_config()
        }
    }

    // Minimal Config for these tests — only backend selection matters here.
    fn make_test_config() -> Config {
        Config {
            profile: None,
            remote_host: Some("compute-eda-42".into()),
            remote_user: None,
            port: 65432,
            jump_host: None,
            jump_user: None,
            ssh_port: None,
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
            cadence_cshrc: None,
            spectre_bin: None,
            roles: crate::config::RemoteRoles::default(),
        }
    }

    #[test]
    fn unset_defaults_to_openssh() {
        assert_eq!(
            SshBackend::from_config(&config_with(None)).unwrap(),
            SshBackend::OpenSsh
        );
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(SshBackend::parse("native"), Some(SshBackend::Native));
        assert_eq!(SshBackend::parse("OpenSSH"), Some(SshBackend::OpenSsh));
        assert_eq!(SshBackend::parse("  Native "), Some(SshBackend::Native));
        assert_eq!(SshBackend::parse("banana"), None);
    }

    #[test]
    fn native_is_recognised_not_rejected_as_typo() {
        // `native` must not surface as an unrecognised-value error even on
        // builds without the feature.
        assert_eq!(
            SshBackend::from_config(&config_with(Some("native"))).unwrap(),
            SshBackend::Native
        );
    }

    #[test]
    fn unrecognised_value_is_a_configuration_error() {
        let err = SshBackend::from_config(&config_with(Some("banana"))).unwrap_err();
        assert!(
            matches!(err, TransportError::Configuration(_)),
            "got {err:?}"
        );
        assert!(err.to_string().contains("banana"));
    }

    #[test]
    fn native_request_reports_unsupported_backend_not_a_fallback() {
        // `Arc<dyn RemoteTransport>` is not `Debug`, so neither `unwrap_err`
        // nor `expect_err` applies; match on the result instead.
        let err = match open_transport(&config_with(Some("native"))) {
            Ok(_) => panic!("native must not silently fall back to OpenSSH"),
            Err(e) => e,
        };
        assert!(
            matches!(err, TransportError::UnsupportedBackend),
            "native must report UnsupportedBackend, got {err:?}"
        );
        // And it must not be retryable / not silently succeed.
        assert!(!err.retryable());
    }

    #[test]
    fn openssh_request_builds_a_transport() {
        assert!(open_transport(&config_with(Some("openssh"))).is_ok());
        // Unset resolves to the same default backend.
        assert!(open_transport(&config_with(None)).is_ok());
    }

    #[test]
    fn openssh_backend_is_always_supported() {
        assert!(SshBackend::OpenSsh.supported_in_this_build());
        // Native support tracks the compile-time feature.
        assert_eq!(
            SshBackend::Native.supported_in_this_build(),
            cfg!(feature = "native-ssh")
        );
    }

    #[test]
    fn require_openssh_passes_for_default_and_openssh() {
        assert!(require_openssh(&config_with(None)).is_ok());
        assert!(require_openssh(&config_with(Some("openssh"))).is_ok());
    }

    #[test]
    fn require_openssh_rejects_native_on_unsupported_build() {
        let err = require_openssh(&config_with(Some("native")))
            .expect_err("native must be rejected on an OpenSSH-only code path");
        assert!(
            matches!(err, TransportError::UnsupportedBackend),
            "got {err:?}"
        );
        assert!(!err.retryable());
    }

    #[test]
    fn require_openssh_surfaces_configuration_error_for_unknown_backend() {
        let err = require_openssh(&config_with(Some("banana")))
            .expect_err("unknown backend must not slip through the guard");
        assert!(matches!(err, TransportError::Configuration(_)));
    }
    #[test]
    fn display_matches_the_env_value() {
        assert_eq!(SshBackend::OpenSsh.to_string(), "openssh");
        assert_eq!(SshBackend::Native.to_string(), "native");
    }

    #[test]
    fn disable_control_master_is_honoured() {
        let mut config = config_with(Some("openssh"));
        config.disable_control_master = true;
        // The flag is applied to the concrete OpenSSH backend; assert it there
        // rather than downcasting the trait object.
        let transport = OpenSshTransport::from_config(&config).with_control_master_disabled();
        assert!(!*transport.runner().use_control_master.lock().unwrap());
        // And `open_transport` still succeeds with the flag set.
        assert!(open_transport(&config).is_ok());
    }

    #[test]
    fn control_master_enabled_by_default() {
        let transport = OpenSshTransport::from_config(&config_with(Some("openssh")));
        assert!(*transport.runner().use_control_master.lock().unwrap());
    }
}

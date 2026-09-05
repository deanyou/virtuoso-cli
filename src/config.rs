use crate::error::{Result, VirtuosoError};
use std::env;
use std::path::PathBuf;

/// Functional role split for the multi-host EDA layout.
///
/// A single EDA environment may have several distinct hosts:
///   - **GUI host**: where Virtuoso CIW/GUI runs (used for X11, bootstrap)
///   - **deploy host**: where bridge files are pushed (often the same as GUI)
///   - **daemon host**: where the RAMIC/HBridge daemon listens (compute node)
///   - **spectre host**: where Spectre executes (compute node, possibly
///     on HPC batch queues)
///
/// For the common single-host setup, all roles collapse onto
/// `VB_REMOTE_HOST`. When roles diverge (CIW on bastion, daemon on
/// compute-42, Spectre on hpc-7), each can be set independently via
/// `VB_GUI_HOST` / `VB_DEPLOY_HOST` / `VB_DAEMON_HOST` / `VB_SPECTRE_HOST`.
/// SSH topology (jump host, ssh user/key) is shared across all roles.
///
/// `VB_REMOTE_SCRATCH_ROOT` is an absolute path visible from BOTH the GUI
/// and daemon hosts so they can exchange files without round-tripping
/// through the local box.
#[derive(Debug, Clone, Default)]
pub struct RemoteRoles {
    pub gui_host: Option<String>,
    pub deploy_host: Option<String>,
    pub daemon_host: Option<String>,
    pub spectre_host: Option<String>,
    pub scratch_root: Option<String>,
}

impl RemoteRoles {
    /// Resolve a role to its configured value, falling back to the
    /// legacy `remote_host` default. Returns `None` only when no
    /// fallback is available (local mode).
    fn resolve(role: Option<String>, fallback: Option<&str>) -> Option<String> {
        match role {
            Some(v) if !v.is_empty() => Some(v),
            _ => fallback.map(|s| s.to_string()),
        }
    }

    /// Resolve with the role's own configured value, ignoring fallback.
    fn own(&self, role: &Option<String>) -> Option<String> {
        role.as_ref().filter(|v| !v.is_empty()).cloned()
    }

    /// GUI host (Virtuoso CIW/X11) — `None` in local mode.
    pub fn gui_host_opt(&self) -> Option<String> {
        self.own(&self.gui_host)
    }

    /// Deploy host (bridge file push target).
    pub fn deploy_host_opt(&self) -> Option<String> {
        self.own(&self.deploy_host)
    }

    /// Daemon host (RAMIC bridge listener).
    pub fn daemon_host_opt(&self) -> Option<String> {
        self.own(&self.daemon_host)
    }

    /// Spectre compute host.
    pub fn spectre_host_opt(&self) -> Option<String> {
        self.own(&self.spectre_host)
    }

    /// Shared scratch path visible to both GUI and daemon hosts. No
    /// fallback — only meaningful when explicitly set.
    pub fn scratch_root(&self) -> Option<&str> {
        self.scratch_root.as_deref()
    }

    /// Resolved string for a role with the provided fallback
    /// (`remote_host`). Empty string when neither is set; callers
    /// must gate on `Config::is_remote()` for meaningful use.
    pub fn resolve_with(&self, role: &Option<String>, fallback: Option<&str>) -> String {
        Self::resolve(role.clone(), fallback).unwrap_or_default()
    }

    /// GUI host with fallback to the legacy `remote_host`.
    pub fn gui_host(&self, fallback: Option<&str>) -> String {
        self.resolve_with(&self.gui_host, fallback)
    }

    /// Deploy host with fallback.
    pub fn deploy_host(&self, fallback: Option<&str>) -> String {
        self.resolve_with(&self.deploy_host, fallback)
    }

    /// Daemon host with fallback.
    pub fn daemon_host(&self, fallback: Option<&str>) -> String {
        self.resolve_with(&self.daemon_host, fallback)
    }

    /// Spectre host with fallback.
    pub fn spectre_host(&self, fallback: Option<&str>) -> String {
        self.resolve_with(&self.spectre_host, fallback)
    }
}

#[derive(Clone)]
pub struct Config {
    #[allow(dead_code)]
    pub profile: Option<String>,
    pub remote_host: Option<String>,
    pub remote_user: Option<String>,
    pub port: u16,
    pub jump_host: Option<String>,
    pub jump_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_key: Option<String>,
    /// Path to a custom SSH config file (VB_SSH_CONFIG). Passed as `-F` to ssh.
    pub ssh_config: Option<String>,
    /// Which SSH backend to use (VB_SSH_BACKEND): `openssh` (default) or
    /// `native`. `native` requires the `native-ssh` Cargo feature; without it the
    /// request is rejected with a structured `UnsupportedBackend` rather than
    /// silently falling back to OpenSSH.
    pub ssh_backend: Option<String>,
    /// Disable SSH ControlMaster multiplexing (VB_DISABLE_CONTROL_MASTER=1).
    /// Set this on WSL2/Windows when the CM socket path contains non-ASCII chars.
    pub disable_control_master: bool,
    pub timeout: u64,
    /// Timeout for read operations (list_instances, list_nets, etc.) in seconds.
    /// VB_READ_TIMEOUT, default 120. Separate from VB_TIMEOUT which covers write ops.
    pub read_timeout: u64,
    pub keep_remote_files: bool,
    pub spectre_cmd: String,
    pub spectre_args: Vec<String>,
    /// Maximum parallel Spectre compute threads (VB_SPECTRE_MAX_WORKERS, default: 8)
    pub spectre_max_workers: u32,
    /// Total concurrent exec/SFTP sessions per endpoint (VB_SSH_MAX_SESSIONS,
    /// default 10). Native backend only — the OpenSSH backend multiplexes
    /// through ControlMaster and has no such ceiling.
    pub ssh_max_sessions: usize,
    /// How many of those sessions bulk transfers may occupy
    /// (VB_SSH_MAX_BULK_SESSIONS, default 2). Native backend only.
    pub ssh_max_bulk_sessions: usize,
    /// Reconnect attempts before the endpoint is marked Degraded
    /// (VB_SSH_RECONNECT_MAX_ATTEMPTS, default 8). Native backend only.
    pub ssh_reconnect_max_attempts: u32,
    /// Upper bound on a single reconnect wait in seconds
    /// (VB_SSH_RECONNECT_MAX_DELAY, default 30). Native backend only.
    pub ssh_reconnect_max_delay: u64,
    /// Seconds between native SSH keepalive probes
    /// (VB_SSH_KEEPALIVE_INTERVAL, default 30). Native backend only.
    pub ssh_keepalive_interval: u64,
    /// Consecutive missed keepalives before the connection is declared dead
    /// (VB_SSH_KEEPALIVE_FAILURES, default 3). Native backend only.
    pub ssh_keepalive_failures: u32,
    /// Grace period in seconds that `tunnel stop` grants running work before
    /// cancelling it (VB_TRANSPORT_SHUTDOWN_GRACE, default 10).
    pub transport_shutdown_grace: u64,
    /// Path to Cadence environment setup file (VB_CADENCE_CSHRC).
    /// Used to load Spectre environment for remote SSH execution.
    pub cadence_cshrc: Option<String>,
    /// Absolute path to Spectre binary (VB_SPECTRE_BIN).
    /// When set, this path is used directly instead of relying on PATH.
    /// Useful when Spectre is not in PATH or multiple versions exist.
    pub spectre_bin: Option<String>,
    /// Multi-host role split. When empty (the common case), all roles
    /// resolve to `remote_host`. See [`RemoteRoles`].
    pub roles: RemoteRoles,
    /// Path to a running transport daemon's IPC socket (VB_TRANSPORT_DAEMON_SOCKET).
    /// When set, the native backend connects to the daemon via IPC instead of
    /// opening a fresh SSH connection per operation. Unix-only.
    pub transport_daemon_socket: Option<String>,
    /// Auth token for the transport daemon IPC connection (VB_TRANSPORT_DAEMON_TOKEN).
    /// Must match the token the daemon was started with. Unix-only.
    pub transport_daemon_token: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("profile", &self.profile)
            .field("remote_host", &self.remote_host)
            .field("remote_user", &self.remote_user)
            .field("port", &self.port)
            .field("jump_host", &self.jump_host)
            .field("jump_user", &self.jump_user)
            .field("ssh_port", &self.ssh_port)
            .field("ssh_key", &self.ssh_key)
            .field("ssh_config", &self.ssh_config)
            .field("ssh_backend", &self.ssh_backend)
            .field("disable_control_master", &self.disable_control_master)
            .field("timeout", &self.timeout)
            .field("read_timeout", &self.read_timeout)
            .field("keep_remote_files", &self.keep_remote_files)
            .field("spectre_cmd", &self.spectre_cmd)
            .field("spectre_args", &self.spectre_args)
            .field("spectre_max_workers", &self.spectre_max_workers)
            .field("ssh_max_sessions", &self.ssh_max_sessions)
            .field("ssh_max_bulk_sessions", &self.ssh_max_bulk_sessions)
            .field(
                "ssh_reconnect_max_attempts",
                &self.ssh_reconnect_max_attempts,
            )
            .field("ssh_reconnect_max_delay", &self.ssh_reconnect_max_delay)
            .field("ssh_keepalive_interval", &self.ssh_keepalive_interval)
            .field("ssh_keepalive_failures", &self.ssh_keepalive_failures)
            .field("transport_shutdown_grace", &self.transport_shutdown_grace)
            .field("cadence_cshrc", &self.cadence_cshrc)
            .field("spectre_bin", &self.spectre_bin)
            .field("roles", &self.roles)
            .field("transport_daemon_socket", &self.transport_daemon_socket)
            .field(
                "transport_daemon_token",
                &self
                    .transport_daemon_token
                    .as_ref()
                    .map(|_| "***redacted***"),
            )
            .finish()
    }
}

impl Config {
    /// Read a config variable, checking profile-specific first (e.g. VB_REMOTE_HOST_prod).
    ///
    /// `pub(crate)` so transport submodules can resolve their own settings with
    /// the *same* precedence rule instead of re-implementing it — a second copy
    /// of this lookup would drift the moment one side changed.
    pub(crate) fn env_with_profile(key: &str, profile: Option<&str>) -> Option<String> {
        if let Some(p) = profile {
            if let Ok(v) = env::var(format!("{key}_{p}")) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        env::var(key).ok().filter(|s| !s.is_empty())
    }

    pub fn from_env() -> Result<Self> {
        // Use the hierarchical profile resolver:
        // 1. VB_PROFILE env var
        // 2. Virtualenv binding ($VIRTUAL_ENV/.vcli-profile)
        // 3. User-level ~/.vcli/.env VB_PROFILE
        let profile = Self::resolve_profile();
        Self::from_env_with_profile(profile.as_deref())
    }

    /// Resolve profile from env var, venv binding, or user-level config.
    ///
    /// `pub(crate)` so that `commands::profile::show()` and integration
    /// tests can introspect resolution without going through `from_env`.
    pub(crate) fn resolve_profile() -> Option<String> {
        // 1. Process environment VB_PROFILE
        if let Ok(v) = env::var("VB_PROFILE") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        // 2. Virtualenv binding ($VIRTUAL_ENV/.vcli-profile)
        if let Ok(venv) = env::var("VIRTUAL_ENV") {
            if !venv.is_empty() {
                let binding_path = std::path::PathBuf::from(&venv).join(".vcli-profile");
                if let Ok(content) = std::fs::read_to_string(&binding_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with('#') {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }

        // 3. User-level ~/.vcli/.env VB_PROFILE
        if let Some(home) = dirs::home_dir() {
            let user_env = home.join(".vcli").join(".env");
            if user_env.exists() {
                if let Ok(content) = std::fs::read_to_string(&user_env) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("VB_PROFILE=") {
                            if let Some(value) = trimmed.strip_prefix("VB_PROFILE=") {
                                let trimmed = value.trim();
                                if !trimmed.is_empty() {
                                    return Some(trimmed.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    pub fn from_env_with_profile(profile: Option<&str>) -> Result<Self> {
        Self::from_env_resolve(profile, true)
    }

    /// Like [`from_env_with_profile`] but never honors the ambient `VB_TARGET`
    /// bridge. Used by `target::resolve` for profile/legacy selections so a
    /// leftover `VB_TARGET` cannot hijack the resolved configuration.
    pub(crate) fn from_env_with_profile_no_target(profile: Option<&str>) -> Result<Self> {
        Self::from_env_resolve(profile, false)
    }

    fn from_env_resolve(profile: Option<&str>, honor_vb_target: bool) -> Result<Self> {
        load_dotenv_upward();

        if honor_vb_target {
            // TEMPORARY bridge (P0-A): main() resolves the target/profile
            // selection via target::resolve and syncs VB_TARGET here. This
            // branch must be removed together with the env-var bridge when
            // commands receive the resolved Config explicitly (CommandContext
            // propagation).
            if let Ok(target_name) = std::env::var("VB_TARGET") {
                if !target_name.is_empty() {
                    let manager = crate::target::TargetManager::load().map_err(|e| {
                        VirtuosoError::Config(format!("failed to load targets: {e}"))
                    })?;
                    let target = manager.get(&target_name).ok_or_else(|| {
                        VirtuosoError::Config(format!("target '{}' not found", target_name))
                    })?;
                    return Self::from_target(target, &target_name);
                }
            }
        }

        let remote_host = Self::env_with_profile("VB_REMOTE_HOST", profile);

        let port: u16 = Self::env_with_profile("VB_PORT", profile)
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(Self::default_port);

        if port == 0 {
            return Err(VirtuosoError::Config(
                "VB_PORT must be between 1 and 65535".into(),
            ));
        }

        let sessions_dir = Some(crate::runtime_paths::cache_subdir(&["sessions"]));
        if let Some(ref d) = sessions_dir {
            tracing::debug!("session dir: {}", d.display());
        }

        Ok(Self {
            profile: profile.map(|s| s.to_string()),
            remote_host,
            remote_user: Self::env_with_profile("VB_REMOTE_USER", profile),
            port,
            jump_host: Self::env_with_profile("VB_JUMP_HOST", profile),
            jump_user: Self::env_with_profile("VB_JUMP_USER", profile),
            ssh_port: Self::env_with_profile("VB_SSH_PORT", profile).and_then(|v| v.parse().ok()),
            ssh_key: Self::env_with_profile("VB_SSH_KEY", profile),
            ssh_config: Self::env_with_profile("VB_SSH_CONFIG", profile),
            ssh_backend: Self::env_with_profile("VB_SSH_BACKEND", profile),
            disable_control_master: Self::env_with_profile("VB_DISABLE_CONTROL_MASTER", profile)
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            timeout: Self::env_with_profile("VB_TIMEOUT", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            read_timeout: Self::env_with_profile("VB_READ_TIMEOUT", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
            keep_remote_files: Self::env_with_profile("VB_KEEP_REMOTE_FILES", profile)
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            spectre_cmd: Self::env_with_profile("VB_SPECTRE_CMD", profile)
                .unwrap_or_else(|| "spectre".into()),
            spectre_args: Self::env_with_profile("VB_SPECTRE_ARGS", profile)
                .map(|v| {
                    shlex::split(&v).ok_or_else(|| {
                        VirtuosoError::Config(format!(
                            "VB_SPECTRE_ARGS contains invalid shell syntax: {v}"
                        ))
                    })
                })
                .unwrap_or(Ok(Vec::new()))?,
            spectre_max_workers: Self::env_with_profile("VB_SPECTRE_MAX_WORKERS", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            ssh_max_sessions: Self::env_with_profile("VB_SSH_MAX_SESSIONS", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::transport::scheduler::SchedulerLimits::DEFAULT_TOTAL),
            ssh_max_bulk_sessions: Self::env_with_profile("VB_SSH_MAX_BULK_SESSIONS", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::transport::scheduler::SchedulerLimits::DEFAULT_BULK),
            ssh_reconnect_max_attempts: Self::env_with_profile(
                "VB_SSH_RECONNECT_MAX_ATTEMPTS",
                profile,
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_ATTEMPTS),
            ssh_reconnect_max_delay: Self::env_with_profile("VB_SSH_RECONNECT_MAX_DELAY", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_DELAY),
            ssh_keepalive_interval: Self::env_with_profile("VB_SSH_KEEPALIVE_INTERVAL", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::transport::lifecycle::KeepalivePolicy::DEFAULT_INTERVAL),
            ssh_keepalive_failures: Self::env_with_profile("VB_SSH_KEEPALIVE_FAILURES", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::transport::lifecycle::KeepalivePolicy::DEFAULT_FAILURES),
            transport_shutdown_grace: Self::env_with_profile(
                "VB_TRANSPORT_SHUTDOWN_GRACE",
                profile,
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(crate::transport::lifecycle::ShutdownCoordinator::DEFAULT_GRACE),
            cadence_cshrc: Self::env_with_profile("VB_CADENCE_CSHRC", profile),
            spectre_bin: Self::env_with_profile("VB_SPECTRE_BIN", profile),
            roles: RemoteRoles {
                gui_host: Self::env_with_profile("VB_GUI_HOST", profile),
                deploy_host: Self::env_with_profile("VB_DEPLOY_HOST", profile),
                daemon_host: Self::env_with_profile("VB_DAEMON_HOST", profile),
                spectre_host: Self::env_with_profile("VB_SPECTRE_HOST", profile),
                scratch_root: Self::env_with_profile("VB_REMOTE_SCRATCH_ROOT", profile),
            },
            transport_daemon_socket: Self::env_with_profile("VB_TRANSPORT_DAEMON_SOCKET", profile),
            transport_daemon_token: Self::env_with_profile("VB_TRANSPORT_DAEMON_TOKEN", profile),
        })
    }

    /// Derive a stable default port from the current username.
    /// Range: 65000-65499, deterministic per user to reduce collisions.
    fn default_port() -> u16 {
        let user = env::var("USER")
            .or_else(|_| env::var("USERNAME"))
            .unwrap_or_default();
        let hash: u16 = user.bytes().map(|b| b as u16).sum::<u16>() % 500;
        65000 + hash
    }

    /// Create a Config from a TargetConfig (multi-target mode).
    ///
    /// TargetConfig fields override defaults; None fields fall back to
    /// the same defaults as from_env().
    pub fn from_target(target: &crate::target::TargetConfig, target_name: &str) -> Result<Self> {
        let port = target.port.unwrap_or_else(Self::default_port);
        if port == 0 {
            return Err(VirtuosoError::Config(
                "target port must be between 1 and 65535".into(),
            ));
        }

        Ok(Self {
            profile: Some(target_name.to_string()),
            remote_host: target.remote_host.clone(),
            remote_user: target.remote_user.clone(),
            port,
            jump_host: target.jump_host.clone(),
            jump_user: target.jump_user.clone(),
            ssh_port: target.ssh_port,
            ssh_key: target.ssh_key.clone(),
            ssh_config: target.ssh_config.clone(),
            ssh_backend: target.ssh_backend.clone(),
            disable_control_master: target.disable_control_master.unwrap_or(false),
            timeout: target.timeout.unwrap_or(30),
            read_timeout: target.read_timeout.unwrap_or(120),
            keep_remote_files: target.keep_remote_files.unwrap_or(false),
            spectre_cmd: target
                .spectre_cmd
                .clone()
                .unwrap_or_else(|| "spectre".into()),
            spectre_args: target.spectre_args.clone().unwrap_or_default(),
            spectre_max_workers: target.spectre_max_workers.unwrap_or(8),
            ssh_max_sessions: target
                .ssh_max_sessions
                .unwrap_or(crate::transport::scheduler::SchedulerLimits::DEFAULT_TOTAL),
            ssh_max_bulk_sessions: target
                .ssh_max_bulk_sessions
                .unwrap_or(crate::transport::scheduler::SchedulerLimits::DEFAULT_BULK),
            ssh_reconnect_max_attempts: target
                .ssh_reconnect_max_attempts
                .unwrap_or(crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_ATTEMPTS),
            ssh_reconnect_max_delay: target
                .ssh_reconnect_max_delay
                .unwrap_or(crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_DELAY),
            ssh_keepalive_interval: target
                .ssh_keepalive_interval
                .unwrap_or(crate::transport::lifecycle::KeepalivePolicy::DEFAULT_INTERVAL),
            ssh_keepalive_failures: target
                .ssh_keepalive_failures
                .unwrap_or(crate::transport::lifecycle::KeepalivePolicy::DEFAULT_FAILURES),
            transport_shutdown_grace: 10,
            cadence_cshrc: target.cadence_cshrc.clone(),
            spectre_bin: target.spectre_bin.clone(),
            roles: RemoteRoles::default(),
            transport_daemon_socket: target.transport_daemon_socket.clone(),
            transport_daemon_token: target.transport_daemon_token.clone(),
        })
    }

    /// Deterministic SHA-256 over the non-secret identity fields of the
    /// resolved config. Used for config identity (F05): `tunnel status` drift
    /// detection and daemon Hello validation compare this digest instead of
    /// trusting parsed values alone. Credentials are deliberately excluded,
    /// but all fields that shape the connection identity ARE included: host,
    /// bridge port, SSH port, the *path* of the identity key / ssh_config
    /// (paths, not key material), jump route, backend, timeouts and control
    /// master behaviour.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for part in [
            self.remote_host.as_deref().unwrap_or(""),
            &self.port.to_string(),
            self.remote_user.as_deref().unwrap_or(""),
            &self
                .ssh_port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "22".into()),
            self.ssh_key.as_deref().unwrap_or(""),
            self.ssh_config.as_deref().unwrap_or(""),
            self.jump_host.as_deref().unwrap_or(""),
            self.jump_user.as_deref().unwrap_or(""),
            self.profile.as_deref().unwrap_or(""),
            self.ssh_backend.as_deref().unwrap_or(""),
            &self.disable_control_master.to_string(),
            &self.timeout.to_string(),
            &self.read_timeout.to_string(),
        ] {
            hasher.update(part.as_bytes());
            hasher.update([0u8]);
        }
        hex::encode(hasher.finalize())
    }

    pub fn is_remote(&self) -> bool {
        self.remote_host.is_some()
    }

    #[allow(dead_code)]
    pub fn ssh_target(&self) -> String {
        let host = self.remote_host.as_deref().unwrap_or("");
        match &self.remote_user {
            Some(user) => format!("{user}@{host}"),
            None => host.to_string(),
        }
    }

    #[allow(dead_code)]
    pub fn ssh_jump(&self) -> Option<String> {
        match (&self.jump_host, &self.jump_user) {
            (Some(host), Some(user)) => Some(format!("{user}@{host}")),
            (Some(host), None) => Some(host.clone()),
            _ => None,
        }
    }
}

/// Walk cwd → parent → … until a `.env` is found, then load it.
/// Stops at filesystem root if no `.env` exists anywhere.
fn load_dotenv_upward() {
    let Ok(start) = std::env::current_dir() else {
        return;
    };
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join(".env");
        if candidate.exists() {
            match dotenvy::from_path(&candidate) {
                Ok(()) => tracing::debug!("loaded .env from {}", candidate.display()),
                Err(e) => tracing::warn!("failed to load .env from {}: {e}", candidate.display()),
            }
            return;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return,
        }
    }
}

#[allow(dead_code)]
pub fn find_project_root() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".env").exists() {
            return Some(current);
        }
        if current.join("pyproject.toml").exists() {
            let content = std::fs::read_to_string(current.join("pyproject.toml")).ok()?;
            if content.contains("virtuoso-bridge") || content.contains("virtuoso-cli") {
                return Some(current);
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
}

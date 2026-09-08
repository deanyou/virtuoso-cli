use crate::config_file::ConfigFile;
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
    /// Whether `port` is an explicit user constraint (VB_PORT set / target's
    /// `port:` field present) rather than the hash-of-USER default. A default
    /// port is NOT a bridge-endpoint constraint: daemons bind OS-assigned
    /// ports, so a default `port` must never be used to filter or reject
    /// discovered sessions (only an explicit port can be).
    pub port_explicit: bool,
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
    /// If true, suppress the cross-user daemon warning when the daemon's Unix
    /// $USER differs from the configured `remote_user`. Set via
    /// `VB_ALLOW_CROSS_USER_DAEMON=1` (env/profile) or `allow_cross_user_daemon: true`
    /// in a target config. Does NOT bypass target/session ownership validation —
    /// only silences the informational cross-user warning.
    pub allow_cross_user_daemon: bool,
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

/// Layered lookup used while a [`Config`] is being built.
///
/// The configuration file is read **once**, by the caller, and every field is
/// resolved against the parsed result — a `Config` has ~36 fields and none of
/// them may touch the disk.
///
/// Precedence, highest first (RFC #83):
///
/// 1. `<KEY>_<PROFILE>` environment variable
/// 2. `<KEY>` environment variable
/// 3. `config.toml` `[profile.<PROFILE>]` section
/// 4. `config.toml` global section
/// 5. the caller's default
///
/// A value that is **set but unparseable** is an error rather than a silent
/// fall back to the default: asking for a 45 second timeout must not quietly
/// become 30. Only an unset value reaches the next layer.
struct Layers<'a> {
    profile: Option<&'a str>,
    file: Option<&'a ConfigFile>,
}

impl Layers<'_> {
    /// Profile-specific env, then general env, then the file.
    fn raw(&self, key: &str) -> Option<String> {
        if let Some(p) = self.profile {
            if let Ok(v) = env::var(format!("{key}_{p}")) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        if let Ok(v) = env::var(key) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        self.file.and_then(|f| f.get(key, self.profile))
    }

    fn text(&self, key: &str) -> Option<String> {
        self.raw(key)
    }

    fn text_or(&self, key: &str, default: &str) -> String {
        self.raw(key).unwrap_or_else(|| default.to_string())
    }

    /// Parse a `FromStr` value. `Ok(None)` when no layer supplies one.
    fn parsed<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        match self.raw(key) {
            None => Ok(None),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|e| VirtuosoError::Config(format!("{key}: {v:?} is not valid: {e}"))),
        }
    }

    /// [`Layers::parsed`] with the lowest-precedence layer folded in.
    fn parsed_or<T>(&self, key: &str, default: T) -> Result<T>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        Ok(self.parsed::<T>(key)?.unwrap_or(default))
    }

    /// Boolean flag: `1` / `0` / `true` / `false` (case-insensitive).
    /// Anything else that is *set* is an error, not `false`.
    fn flag(&self, key: &str) -> Result<bool> {
        match self.raw(key) {
            None => Ok(false),
            Some(v) if v == "1" || v.eq_ignore_ascii_case("true") => Ok(true),
            Some(v) if v == "0" || v.eq_ignore_ascii_case("false") => Ok(false),
            Some(v) => Err(VirtuosoError::Config(format!(
                "{key}: expected true/false/1/0, got {v:?}"
            ))),
        }
    }

    /// Like [`Layers::flag`], but also accepts `yes` / `no` / `on` / `off`.
    ///
    /// `VB_ALLOW_CROSS_USER_DAEMON` shipped accepting those spellings, so they
    /// keep working here. Unrecognised values still mean "off" rather than an
    /// error: this field only *suppresses an informational warning*, so the
    /// safe direction for an unreadable value is the conservative one.
    fn flag_truthy(&self, key: &str) -> bool {
        matches!(
            self.raw(key)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
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
        // 3. User-level ~/.vcli/profile (deprecated fallback: ~/.vcli/.env)
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

        // 3. User-level ~/.vcli/profile (sole content is the profile name),
        //    with ~/.vcli/.env VB_PROFILE= kept as a deprecated fallback.
        if let Some(home) = dirs::home_dir() {
            let vcli_dir = home.join(".vcli");

            let user_profile = vcli_dir.join("profile");
            if let Ok(content) = std::fs::read_to_string(&user_profile) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        return Some(trimmed.to_string());
                    }
                }
            }

            let user_env = vcli_dir.join(".env");
            if user_env.exists() {
                if let Ok(content) = std::fs::read_to_string(&user_env) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if let Some(value) = trimmed.strip_prefix("VB_PROFILE=") {
                            let trimmed = value.trim();
                            if !trimmed.is_empty() {
                                tracing::warn!(
                                    "reading VB_PROFILE from {} is deprecated and will be \
                                     removed — run `vcli profile bind <name> --user` to \
                                     migrate, or export VB_PROFILE in your shell",
                                    user_env.display()
                                );
                                return Some(trimmed.to_string());
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
        // No `.env` loading (RFC #83): configuration comes from the process
        // environment and from a single, fixed, queryable `config.toml`.
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

        // One read, shared by every field below. The explicit-target branch
        // above returns before reaching here on purpose: a target's identity
        // comes from targets.yaml alone and must not be diluted by
        // config.toml.
        let file = ConfigFile::load()?;
        let lz = Layers {
            profile,
            file: file.as_ref(),
        };

        let port: u16 = lz.parsed_or("VB_PORT", Self::default_port())?;
        let port_explicit = lz.raw("VB_PORT").is_some();

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
            remote_host: lz.text("VB_REMOTE_HOST"),
            remote_user: lz.text("VB_REMOTE_USER"),
            port,
            port_explicit,
            jump_host: lz.text("VB_JUMP_HOST"),
            jump_user: lz.text("VB_JUMP_USER"),
            ssh_port: lz.parsed("VB_SSH_PORT")?,
            ssh_key: lz.text("VB_SSH_KEY"),
            ssh_config: lz.text("VB_SSH_CONFIG"),
            ssh_backend: lz.text("VB_SSH_BACKEND"),
            disable_control_master: lz.flag("VB_DISABLE_CONTROL_MASTER")?,
            timeout: lz.parsed_or("VB_TIMEOUT", 30)?,
            read_timeout: lz.parsed_or("VB_READ_TIMEOUT", 120)?,
            keep_remote_files: lz.flag("VB_KEEP_REMOTE_FILES")?,
            spectre_cmd: lz.text_or("VB_SPECTRE_CMD", "spectre"),
            spectre_args: match lz.text("VB_SPECTRE_ARGS") {
                None => Vec::new(),
                Some(v) => shlex::split(&v).ok_or_else(|| {
                    VirtuosoError::Config(format!(
                        "VB_SPECTRE_ARGS contains invalid shell syntax: {v}"
                    ))
                })?,
            },
            spectre_max_workers: lz.parsed_or("VB_SPECTRE_MAX_WORKERS", 8)?,
            ssh_max_sessions: lz.parsed_or(
                "VB_SSH_MAX_SESSIONS",
                crate::transport::scheduler::SchedulerLimits::DEFAULT_TOTAL,
            )?,
            ssh_max_bulk_sessions: lz.parsed_or(
                "VB_SSH_MAX_BULK_SESSIONS",
                crate::transport::scheduler::SchedulerLimits::DEFAULT_BULK,
            )?,
            ssh_reconnect_max_attempts: lz.parsed_or(
                "VB_SSH_RECONNECT_MAX_ATTEMPTS",
                crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_ATTEMPTS,
            )?,
            ssh_reconnect_max_delay: lz.parsed_or(
                "VB_SSH_RECONNECT_MAX_DELAY",
                crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_DELAY,
            )?,
            ssh_keepalive_interval: lz.parsed_or(
                "VB_SSH_KEEPALIVE_INTERVAL",
                crate::transport::lifecycle::KeepalivePolicy::DEFAULT_INTERVAL,
            )?,
            ssh_keepalive_failures: lz.parsed_or(
                "VB_SSH_KEEPALIVE_FAILURES",
                crate::transport::lifecycle::KeepalivePolicy::DEFAULT_FAILURES,
            )?,
            transport_shutdown_grace: lz.parsed_or(
                "VB_TRANSPORT_SHUTDOWN_GRACE",
                crate::transport::lifecycle::ShutdownCoordinator::DEFAULT_GRACE,
            )?,
            cadence_cshrc: lz.text("VB_CADENCE_CSHRC"),
            spectre_bin: lz.text("VB_SPECTRE_BIN"),
            roles: RemoteRoles {
                gui_host: lz.text("VB_GUI_HOST"),
                deploy_host: lz.text("VB_DEPLOY_HOST"),
                daemon_host: lz.text("VB_DAEMON_HOST"),
                spectre_host: lz.text("VB_SPECTRE_HOST"),
                scratch_root: lz.text("VB_REMOTE_SCRATCH_ROOT"),
            },
            transport_daemon_socket: lz.text("VB_TRANSPORT_DAEMON_SOCKET"),
            transport_daemon_token: lz.text("VB_TRANSPORT_DAEMON_TOKEN"),
            allow_cross_user_daemon: lz.flag_truthy("VB_ALLOW_CROSS_USER_DAEMON"),
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
        let port_explicit = target.port.is_some();
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
            port_explicit,
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
            allow_cross_user_daemon: target.allow_cross_user_daemon.unwrap_or(false),
        })
    }

    /// Deterministic SHA-256 over the non-secret identity fields of the
    /// resolved config. Used for config identity (F05): `tunnel status` drift
    /// detection and daemon Hello validation compare this digest instead of
    /// trusting parsed values alone. Credentials are deliberately excluded,
    /// but all fields that shape the connection identity ARE included: host,
    /// bridge port AND whether that port is an explicit constraint
    /// (`port_explicit` — the same port value with auto-discovery vs forced
    /// port-matching must hash differently), SSH port, the *path* of the
    /// identity key / ssh_config (paths, not key material), jump route,
    /// backend, timeouts and control master behaviour.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for part in [
            self.remote_host.as_deref().unwrap_or(""),
            &self.port.to_string(),
            &self.port_explicit.to_string(),
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

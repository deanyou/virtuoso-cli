use crate::config_file::{ConfigFile, FileLoc};
use crate::error::{Result, VirtuosoError};
use serde::Serialize;
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

/// Which layer a resolved value came from.
///
/// Used by [`Config::build_report`] — the heart of `vcli config check` —
/// so a user can ask "why did this 30-second timeout become 120?" and get an
/// answer that names the actual env var, the exact `[profile.<name>]` section
/// or the defaulting code path.
/// How a [`ConfigSource::Default`] value was derived.
///
/// `Hardcoded` values are compile-time constants (e.g. `timeout = 30`).
/// `Derived` values are computed at runtime from the environment (e.g. the
/// default bridge port is picked by the OS when `port = 0`). Surfacing this
/// distinction lets a user tell "I didn't set it, and the code picked 30"
/// apart from "I didn't set it, and the OS picked 40163".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultKind {
    Hardcoded,
    Derived,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "layer", rename_all = "snake_case")]
pub enum ConfigSource {
    /// `<KEY>_<PROFILE>` environment variable. `var` is the full env-var name.
    EnvProfile { var: String },
    /// `<KEY>` environment variable. `var` is the env-var name.
    Env { var: String },
    /// `config.toml` `[profile.<PROFILE>]` section. `file` is the absolute
    /// path of the parsed file (so a user can `vim` it).
    FileProfile { file: PathBuf },
    /// `config.toml` global section.
    FileGlobal { file: PathBuf },
    /// An active `targets.yaml` entry whose field overrode this setting.
    /// `target` is the target name; `file` is the absolute path of the
    /// `targets.yaml` that owned it (so a user can `vim` it).
    Target { target: String, file: PathBuf },
    /// No higher-precedence layer supplied a value. `default` documents the
    /// constant the resolver fell through to; `kind` distinguishes compile-time
    /// constants from runtime-derived values (e.g. OS-assigned ports).
    Default { default: String, kind: DefaultKind },
}

/// One resolved key, with its value and provenance.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigEntry {
    pub key: &'static str,
    pub value: String,
    pub source: ConfigSource,
}

/// One diagnostic entry in a [`ConfigReport`].
///
/// Diagnostics are structured errors/warnings with stable codes so CI and
/// scripts can match on them without parsing human-readable text. `level`
/// is `"error"` (blocks usage) or `"warning"` (advisory). `field` names the
/// config key or subsystem the diagnostic applies to (e.g. `"VB_TIMEOUT"`,
/// `"targets.yaml"`, `"config.toml"`).
#[derive(Debug, Clone, Serialize)]
pub struct ConfigDiagnostic {
    pub code: &'static str,
    pub level: &'static str,
    pub field: String,
    pub message: String,
}

/// Aggregated report of the resolved configuration.
///
/// Always carries enough context to reproduce the resolution: the active
/// profile, the file that was read (if any), the active target (if any),
/// and a per-key [`ConfigEntry`]. JSON-stable so `vcli config check --format
/// json` round-trips through `jq` without surprises.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigReport {
    pub profile: Option<String>,
    pub active_target: Option<String>,
    /// Path of the config.toml that was read, if any. In target mode this is
    /// `None` because targets.yaml drives resolution and config.toml is not
    /// consulted — callers should check `active_target.is_some()` to tell
    /// "not applicable" apart from "file missing".
    pub config_file: Option<PathBuf>,
    /// Path of the targets.yaml that was read, if a target is active. `None`
    /// in legacy mode.
    pub targets_file: Option<PathBuf>,
    /// Resolution precedence, highest first, with the key spelling each layer
    /// expects. Surfaced in `--format json` so a user can verify the order
    /// without reading code.
    pub precedence: [&'static str; 4],
    pub entries: Vec<ConfigEntry>,
    /// Soft warnings the report noticed (e.g. a deprecated `~/.vcli/.env`
    /// fallback being read). Empty when nothing to warn about. Failures
    /// here don't elevate the process exit code — they are advisory.
    pub warnings: Vec<String>,
    /// Structured diagnostics with stable codes. Includes both parse failures
    /// (e.g. invalid port, missing target) and advisory warnings.
    pub diagnostics: Vec<ConfigDiagnostic>,
    /// `false` when any diagnostic is `level = "error"` — the config cannot
    /// be used as-is. `true` when only warnings (or nothing) were found.
    pub valid: bool,
    /// Config digest (hex-encoded) when resolution succeeded. `None` when a
    /// hard error prevented a complete config from being built (e.g. targets
    /// file unreadable, target not found).
    pub digest: Option<String>,
}

impl ConfigReport {
    /// `Ok(true)` when every entry has a non-default source; `Ok(false)` when
    /// at least one field fell through to a hard-coded default. `Err` when
    /// the user passed `--require-explicit` and a default was the only
    /// source. Distinguishing this from "the default is correct for my
    /// workflow" is the whole point of `config check`.
    pub fn all_explicit(&self) -> bool {
        self.entries
            .iter()
            .all(|e| !matches!(e.source, ConfigSource::Default { .. }))
    }
}

/// Static description of one configuration key.
///
/// `default` is the literal the resolver falls through to. `display` formats
/// the resolved value for the report (so a `bool` reads `true`/`false`, a
/// `u16` port reads `65432`, and a `Vec<String>` reads `[]` or joined with
/// single spaces). `validate` re-runs the same parser the runtime uses so a
/// malformed env var (e.g. `VB_TIMEOUT=abc`) is flagged in the report
/// instead of silently passing through.
struct KeySpec {
    key: &'static str,
    default: &'static str,
    display: fn(&Config) -> String,
    validate: fn(&str) -> std::result::Result<(), String>,
}

fn report_remote_host(cfg: &Config) -> String {
    cfg.remote_host.clone().unwrap_or_default()
}
fn report_remote_user(cfg: &Config) -> String {
    cfg.remote_user.clone().unwrap_or_default()
}
fn report_port(cfg: &Config) -> String {
    cfg.port.to_string()
}
fn report_jump_host(cfg: &Config) -> String {
    cfg.jump_host.clone().unwrap_or_default()
}
fn report_jump_user(cfg: &Config) -> String {
    cfg.jump_user.clone().unwrap_or_default()
}
fn report_ssh_port(cfg: &Config) -> String {
    cfg.ssh_port
        .map(|p| p.to_string())
        .unwrap_or_else(|| "22".into())
}
fn report_ssh_key(cfg: &Config) -> String {
    // The key path may contain sensitive info (username, custom paths).
    // Show only "set" / "unset" to avoid leaking it through a config-check
    // snapshot or terminal scrollback — same policy as transport_daemon_token.
    if cfg.ssh_key.is_some() {
        "***set***".to_string()
    } else {
        String::new()
    }
}
fn report_ssh_config(cfg: &Config) -> String {
    cfg.ssh_config.clone().unwrap_or_default()
}
fn report_ssh_backend(cfg: &Config) -> String {
    cfg.ssh_backend.clone().unwrap_or_default()
}
fn report_disable_control_master(cfg: &Config) -> String {
    cfg.disable_control_master.to_string()
}
fn report_timeout(cfg: &Config) -> String {
    cfg.timeout.to_string()
}
fn report_read_timeout(cfg: &Config) -> String {
    cfg.read_timeout.to_string()
}
fn report_keep_remote_files(cfg: &Config) -> String {
    cfg.keep_remote_files.to_string()
}
fn report_spectre_cmd(cfg: &Config) -> String {
    cfg.spectre_cmd.clone()
}
fn report_spectre_args(cfg: &Config) -> String {
    if cfg.spectre_args.is_empty() {
        String::new()
    } else {
        cfg.spectre_args.join(" ")
    }
}
fn report_spectre_max_workers(cfg: &Config) -> String {
    cfg.spectre_max_workers.to_string()
}
fn report_ssh_max_sessions(cfg: &Config) -> String {
    cfg.ssh_max_sessions.to_string()
}
fn report_ssh_max_bulk_sessions(cfg: &Config) -> String {
    cfg.ssh_max_bulk_sessions.to_string()
}
fn report_ssh_reconnect_max_attempts(cfg: &Config) -> String {
    cfg.ssh_reconnect_max_attempts.to_string()
}
fn report_ssh_reconnect_max_delay(cfg: &Config) -> String {
    cfg.ssh_reconnect_max_delay.to_string()
}
fn report_ssh_keepalive_interval(cfg: &Config) -> String {
    cfg.ssh_keepalive_interval.to_string()
}
fn report_ssh_keepalive_failures(cfg: &Config) -> String {
    cfg.ssh_keepalive_failures.to_string()
}
fn report_transport_shutdown_grace(cfg: &Config) -> String {
    cfg.transport_shutdown_grace.to_string()
}
fn report_cadence_cshrc(cfg: &Config) -> String {
    cfg.cadence_cshrc.clone().unwrap_or_default()
}
fn report_spectre_bin(cfg: &Config) -> String {
    cfg.spectre_bin.clone().unwrap_or_default()
}
fn report_gui_host(cfg: &Config) -> String {
    cfg.roles.gui_host.clone().unwrap_or_default()
}
fn report_deploy_host(cfg: &Config) -> String {
    cfg.roles.deploy_host.clone().unwrap_or_default()
}
fn report_daemon_host(cfg: &Config) -> String {
    cfg.roles.daemon_host.clone().unwrap_or_default()
}
fn report_spectre_host(cfg: &Config) -> String {
    cfg.roles.spectre_host.clone().unwrap_or_default()
}
fn report_remote_scratch_root(cfg: &Config) -> String {
    cfg.roles.scratch_root.clone().unwrap_or_default()
}
fn report_transport_daemon_socket(cfg: &Config) -> String {
    cfg.transport_daemon_socket.clone().unwrap_or_default()
}
fn report_transport_daemon_token(cfg: &Config) -> String {
    // The token is sensitive; show only "set" / "unset" to avoid leaking it
    // through a config-check snapshot or terminal scrollback.
    if cfg.transport_daemon_token.is_some() {
        "***set***".to_string()
    } else {
        String::new()
    }
}

fn report_allow_cross_user_daemon(cfg: &Config) -> String {
    cfg.allow_cross_user_daemon.to_string()
}

/// Sanitise a raw (string) config value for `vcli config check` output.
///
/// The legacy report path reads values as plain strings from env/file and
/// never builds a `&Config`, so it cannot call the `display:` functions
/// (which take `&Config`). This helper mirrors the policy of those functions
/// without requiring a full `Config`: sensitive fields are replaced with
/// `"***set***"` when non-empty, everything else is returned unchanged.
///
/// Keys covered here MUST stay in sync with the `display:` functions in
/// [`KEY_SPECS`] that mask values — currently [`report_transport_daemon_token`]
/// and [`report_ssh_key`].
fn sanitize_raw_value(key: &str, raw: &str) -> String {
    if raw.is_empty() {
        return raw.to_string();
    }
    match key {
        "VB_TRANSPORT_DAEMON_TOKEN" | "VB_SSH_KEY" => "***set***".to_string(),
        _ => raw.to_string(),
    }
}

// ----- Validators (mirror the runtime parsers in from_env_resolve) ----------

fn v_text(_raw: &str) -> std::result::Result<(), String> {
    Ok(())
}
fn v_u16(raw: &str) -> std::result::Result<(), String> {
    raw.parse::<u16>()
        .map(|_| ())
        .map_err(|e| format!("expected u16, got {raw:?}: {e}"))
}
fn v_u32(raw: &str) -> std::result::Result<(), String> {
    raw.parse::<u32>()
        .map(|_| ())
        .map_err(|e| format!("expected u32, got {raw:?}: {e}"))
}
fn v_u64(raw: &str) -> std::result::Result<(), String> {
    raw.parse::<u64>()
        .map(|_| ())
        .map_err(|e| format!("expected u64, got {raw:?}: {e}"))
}
fn v_bool(raw: &str) -> std::result::Result<(), String> {
    if raw == "1"
        || raw == "0"
        || raw.eq_ignore_ascii_case("true")
        || raw.eq_ignore_ascii_case("false")
    {
        Ok(())
    } else {
        Err(format!("expected true/false/1/0, got {raw:?}"))
    }
}
/// Mirrors [`Layers::flag_truthy`]: `VB_ALLOW_CROSS_USER_DAEMON` shipped
/// accepting `yes` / `on`, so those spellings are valid here even though
/// [`v_bool`] stays strict for every other flag. An unrecognised value is
/// reported as invalid (the runtime silently treats it as "off"), so a typo
/// surfaces here instead of hiding.
fn v_truthy(raw: &str) -> std::result::Result<(), String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "0" | "false" | "no" | "off" => Ok(()),
        other => Err(format!(
            "expected true/false/1/0/yes/no/on/off, got {other:?}"
        )),
    }
}
fn v_shlex(raw: &str) -> std::result::Result<(), String> {
    shlex::split(raw)
        .map(|_| ())
        .ok_or_else(|| format!("invalid shell syntax: {raw:?}"))
}

/// The exhaustive list of configuration keys this report covers.
///
/// Order = display order. New keys added to [`Config`] should land here too —
/// the report is the user's authoritative way to verify resolution, and a
/// missing key would silently mis-attribute a value.
#[rustfmt::skip]
const KEY_SPECS: &[KeySpec] = &[
    KeySpec { key: "VB_REMOTE_HOST",         default: "",          display: report_remote_host,            validate: v_text },
    KeySpec { key: "VB_REMOTE_USER",         default: "",          display: report_remote_user,            validate: v_text },
    KeySpec { key: "VB_PORT",                default: "0",         display: report_port,                   validate: v_u16 },
    KeySpec { key: "VB_JUMP_HOST",           default: "",          display: report_jump_host,              validate: v_text },
    KeySpec { key: "VB_JUMP_USER",           default: "",          display: report_jump_user,              validate: v_text },
    KeySpec { key: "VB_SSH_PORT",            default: "22",        display: report_ssh_port,               validate: v_u16 },
    KeySpec { key: "VB_SSH_KEY",             default: "",          display: report_ssh_key,                validate: v_text },
    KeySpec { key: "VB_SSH_CONFIG",          default: "",          display: report_ssh_config,             validate: v_text },
    KeySpec { key: "VB_SSH_BACKEND",         default: "openssh",   display: report_ssh_backend,            validate: v_text },
    KeySpec { key: "VB_DISABLE_CONTROL_MASTER", default: "false",  display: report_disable_control_master, validate: v_bool },
    KeySpec { key: "VB_TIMEOUT",             default: "30",        display: report_timeout,                validate: v_u64 },
    KeySpec { key: "VB_READ_TIMEOUT",        default: "120",       display: report_read_timeout,           validate: v_u64 },
    KeySpec { key: "VB_KEEP_REMOTE_FILES",   default: "false",     display: report_keep_remote_files,      validate: v_bool },
    KeySpec { key: "VB_SPECTRE_CMD",         default: "spectre",   display: report_spectre_cmd,            validate: v_text },
    KeySpec { key: "VB_SPECTRE_ARGS",        default: "",          display: report_spectre_args,           validate: v_shlex },
    KeySpec { key: "VB_SPECTRE_MAX_WORKERS", default: "8",         display: report_spectre_max_workers,    validate: v_u32 },
    KeySpec { key: "VB_SSH_MAX_SESSIONS",    default: "10",        display: report_ssh_max_sessions,       validate: v_u64 },
    KeySpec { key: "VB_SSH_MAX_BULK_SESSIONS", default: "2",       display: report_ssh_max_bulk_sessions,  validate: v_u64 },
    KeySpec { key: "VB_SSH_RECONNECT_MAX_ATTEMPTS", default: "8",  display: report_ssh_reconnect_max_attempts, validate: v_u32 },
    KeySpec { key: "VB_SSH_RECONNECT_MAX_DELAY",   default: "30", display: report_ssh_reconnect_max_delay,   validate: v_u64 },
    KeySpec { key: "VB_SSH_KEEPALIVE_INTERVAL",   default: "30", display: report_ssh_keepalive_interval,   validate: v_u64 },
    KeySpec { key: "VB_SSH_KEEPALIVE_FAILURES",   default: "3",  display: report_ssh_keepalive_failures,   validate: v_u32 },
    KeySpec { key: "VB_TRANSPORT_SHUTDOWN_GRACE", default: "10",  display: report_transport_shutdown_grace, validate: v_u64 },
    KeySpec { key: "VB_CADENCE_CSHRC",       default: "",          display: report_cadence_cshrc,          validate: v_text },
    KeySpec { key: "VB_SPECTRE_BIN",         default: "",          display: report_spectre_bin,            validate: v_text },
    KeySpec { key: "VB_GUI_HOST",            default: "",          display: report_gui_host,               validate: v_text },
    KeySpec { key: "VB_DEPLOY_HOST",         default: "",          display: report_deploy_host,            validate: v_text },
    KeySpec { key: "VB_DAEMON_HOST",         default: "",          display: report_daemon_host,            validate: v_text },
    KeySpec { key: "VB_SPECTRE_HOST",        default: "",          display: report_spectre_host,           validate: v_text },
    KeySpec { key: "VB_REMOTE_SCRATCH_ROOT", default: "",          display: report_remote_scratch_root,    validate: v_text },
    KeySpec { key: "VB_TRANSPORT_DAEMON_SOCKET", default: "",      display: report_transport_daemon_socket, validate: v_text },
    KeySpec { key: "VB_TRANSPORT_DAEMON_TOKEN", default: "",       display: report_transport_daemon_token, validate: v_text },
    KeySpec { key: "VB_ALLOW_CROSS_USER_DAEMON", default: "false", display: report_allow_cross_user_daemon, validate: v_truthy },
];

/// Build the report from a `TargetConfig`.
///
/// Mirrors [`Self::from_target`] field-for-field so the report and the
/// runtime agree: a `None` target field is attributed to the matching
/// `from_env_resolve` default (so the user sees "this value comes from the
/// legacy default, not from the target"); a `Some` value is attributed to
/// the target itself.
fn build_report_from_target(
    target: &crate::target::TargetConfig,
    target_name: &str,
    mut report: ConfigReport,
) -> ConfigReport {
    // We compute a synthetic Config from the target (with defaults applied)
    // so the display closures agree with the runtime path byte-for-byte.
    let cfg =
        Config::from_target(target, target_name).expect("target validates port at from_target");
    for spec in KEY_SPECS {
        let value = (spec.display)(&cfg);
        // Per-field attribution: if the *target* had this field set, the value
        // came from the target. If the target had `None` and the synthetic
        // config shows the constant default, attribute it to "default".
        let target_provided = target_provides(spec.key, target);
        let source = if target_provided {
            ConfigSource::Target {
                target: target_name.to_string(),
                file: crate::target::TargetManager::default_config_path(),
            }
        } else {
            // None from the target means the runtime applied its built-in default.
            // Port is derived (OS-assigned when 0); everything else is hardcoded.
            let kind = if spec.key == "VB_PORT" {
                DefaultKind::Derived
            } else {
                DefaultKind::Hardcoded
            };
            ConfigSource::Default {
                default: spec.default.to_string(),
                kind,
            }
        };
        report.entries.push(ConfigEntry {
            key: spec.key,
            value,
            source,
        });
    }
    report
}

/// Did the active target explicitly set this key?
///
/// `from_target` flattens a `TargetConfig` into a `Config` by applying
/// defaults to `None` fields; the only way to attribute a value to "the
/// target" is to ask whether the underlying `TargetConfig` carried it. This
/// list mirrors [`crate::target::TargetConfig`] field-for-field — when the
/// schema drifts, this must drift with it.
fn target_provides(key: &str, t: &crate::target::TargetConfig) -> bool {
    match key {
        "VB_REMOTE_HOST" => t.remote_host.is_some(),
        "VB_REMOTE_USER" => t.remote_user.is_some(),
        "VB_PORT" => t.port.is_some(),
        "VB_JUMP_HOST" => t.jump_host.is_some(),
        "VB_JUMP_USER" => t.jump_user.is_some(),
        "VB_SSH_PORT" => t.ssh_port.is_some(),
        "VB_SSH_KEY" => t.ssh_key.is_some(),
        "VB_SSH_CONFIG" => t.ssh_config.is_some(),
        "VB_SSH_BACKEND" => t.ssh_backend.is_some(),
        "VB_DISABLE_CONTROL_MASTER" => t.disable_control_master.is_some(),
        "VB_TIMEOUT" => t.timeout.is_some(),
        "VB_READ_TIMEOUT" => t.read_timeout.is_some(),
        "VB_KEEP_REMOTE_FILES" => t.keep_remote_files.is_some(),
        "VB_SPECTRE_CMD" => t.spectre_cmd.is_some(),
        "VB_SPECTRE_ARGS" => t.spectre_args.is_some(),
        "VB_SPECTRE_MAX_WORKERS" => t.spectre_max_workers.is_some(),
        "VB_SSH_MAX_SESSIONS" => t.ssh_max_sessions.is_some(),
        "VB_SSH_MAX_BULK_SESSIONS" => t.ssh_max_bulk_sessions.is_some(),
        "VB_SSH_RECONNECT_MAX_ATTEMPTS" => t.ssh_reconnect_max_attempts.is_some(),
        "VB_SSH_RECONNECT_MAX_DELAY" => t.ssh_reconnect_max_delay.is_some(),
        "VB_SSH_KEEPALIVE_INTERVAL" => t.ssh_keepalive_interval.is_some(),
        "VB_SSH_KEEPALIVE_FAILURES" => t.ssh_keepalive_failures.is_some(),
        "VB_CADENCE_CSHRC" => t.cadence_cshrc.is_some(),
        "VB_SPECTRE_BIN" => t.spectre_bin.is_some(),
        "VB_TRANSPORT_DAEMON_SOCKET" => t.transport_daemon_socket.is_some(),
        "VB_TRANSPORT_DAEMON_TOKEN" => t.transport_daemon_token.is_some(),
        "VB_ALLOW_CROSS_USER_DAEMON" => t.allow_cross_user_daemon.is_some(),
        // TargetConfig has no equivalent for these — `from_target` always
        // returns hard-coded constants here, so they cannot be attributed to
        // the target.
        "VB_TRANSPORT_SHUTDOWN_GRACE"
        | "VB_GUI_HOST"
        | "VB_DEPLOY_HOST"
        | "VB_DAEMON_HOST"
        | "VB_SPECTRE_HOST"
        | "VB_REMOTE_SCRATCH_ROOT" => false,
        _ => false,
    }
}

impl Layers<'_> {
    /// Profile-specific env, then general env, then the file.
    fn raw(&self, key: &str) -> Option<String> {
        self.raw_with_source(key).map(|(v, _)| v)
    }

    /// Same precedence as [`Layers::raw`], but reports which layer actually
    /// produced the value. Used by `Config::build_report` so `vcli config check`
    /// can answer "where did this resolved value come from?" — a 30-second
    /// timeout silently becoming 120 is the exact failure mode #73 was opened
    /// to surface.
    ///
    /// Precedence, highest first (RFC #83, §3 — `vcli config check`):
    /// 1. `<KEY>_<PROFILE>` environment variable
    /// 2. `<KEY>` environment variable
    /// 3. `config.toml` `[profile.<PROFILE>]` section
    /// 4. `config.toml` global section
    fn raw_with_source(&self, key: &str) -> Option<(String, ConfigSource)> {
        if let Some(p) = self.profile {
            let profile_key = format!("{key}_{p}");
            if let Ok(v) = env::var(&profile_key) {
                if !v.is_empty() {
                    return Some((v, ConfigSource::EnvProfile { var: profile_key }));
                }
            }
        }
        if let Ok(v) = env::var(key) {
            if !v.is_empty() {
                return Some((
                    v,
                    ConfigSource::Env {
                        var: key.to_string(),
                    },
                ));
            }
        }
        if let Some(file) = self.file {
            // One walk, one truth. `get_with_loc` returns the value and the
            // precise layer it was read from, so we never have to first probe
            // with `profile_section_has` and read again with `get`. The two
            // passes could disagree on which spelling matched, misattributing a
            // global value to a profile section (and vice versa).
            if let Some((v, loc)) = file.get_with_loc(key, self.profile) {
                let source = match loc {
                    FileLoc::Profile { .. } => ConfigSource::FileProfile {
                        file: crate::config_file::path(),
                    },
                    FileLoc::Global { .. } => ConfigSource::FileGlobal {
                        file: crate::config_file::path(),
                    },
                };
                return Some((v, source));
            }
        }
        None
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

    /// Resolve every field from these layers into a [`Config`].
    ///
    /// The field-by-field read logic here is IDENTICAL to the read logic in
    /// `Config::from_env_resolve`. It lives on `Layers` so the legacy-mode
    /// call site in `build_report` can reuse the SAME `Layers` (and therefore
    /// the SAME `ConfigFile` snapshot) for both per-field provenance tracing
    /// and digest construction — eliminating the redundant second
    /// `ConfigFile::load()` that used to happening there.
    ///
    /// Target-mode must NOT use this path — see
    /// [`Config::from_target`].
    fn to_config(&self) -> Result<Config> {
        let port: u16 = self.parsed_or("VB_PORT", Config::default_port())?;
        let port_explicit = self.raw("VB_PORT").is_some();
        if port == 0 {
            return Err(VirtuosoError::Config(
                "VB_PORT must be between 1 and 65535".into(),
            ));
        }
        Ok(Config {
            profile: self.profile.map(|s| s.to_string()),
            remote_host: self.text("VB_REMOTE_HOST"),
            remote_user: self.text("VB_REMOTE_USER"),
            port,
            port_explicit,
            jump_host: self.text("VB_JUMP_HOST"),
            jump_user: self.text("VB_JUMP_USER"),
            ssh_port: self.parsed("VB_SSH_PORT")?,
            ssh_key: self.text("VB_SSH_KEY"),
            ssh_config: self.text("VB_SSH_CONFIG"),
            ssh_backend: self.text("VB_SSH_BACKEND"),
            disable_control_master: self.flag("VB_DISABLE_CONTROL_MASTER")?,
            timeout: self.parsed_or("VB_TIMEOUT", 30)?,
            read_timeout: self.parsed_or("VB_READ_TIMEOUT", 120)?,
            keep_remote_files: self.flag("VB_KEEP_REMOTE_FILES")?,
            spectre_cmd: self.text_or("VB_SPECTRE_CMD", "spectre"),
            spectre_args: match self.text("VB_SPECTRE_ARGS") {
                None => Vec::new(),
                Some(v) => shlex::split(&v).ok_or_else(|| {
                    VirtuosoError::Config(format!(
                        "VB_SPECTRE_ARGS contains invalid shell syntax: {v}"
                    ))
                })?,
            },
            spectre_max_workers: self.parsed_or("VB_SPECTRE_MAX_WORKERS", 8)?,
            ssh_max_sessions: self.parsed_or(
                "VB_SSH_MAX_SESSIONS",
                crate::transport::scheduler::SchedulerLimits::DEFAULT_TOTAL,
            )?,
            ssh_max_bulk_sessions: self.parsed_or(
                "VB_SSH_MAX_BULK_SESSIONS",
                crate::transport::scheduler::SchedulerLimits::DEFAULT_BULK,
            )?,
            ssh_reconnect_max_attempts: self.parsed_or(
                "VB_SSH_RECONNECT_MAX_ATTEMPTS",
                crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_ATTEMPTS,
            )?,
            ssh_reconnect_max_delay: self.parsed_or(
                "VB_SSH_RECONNECT_MAX_DELAY",
                crate::transport::lifecycle::ReconnectPolicy::DEFAULT_MAX_DELAY,
            )?,
            ssh_keepalive_interval: self.parsed_or(
                "VB_SSH_KEEPALIVE_INTERVAL",
                crate::transport::lifecycle::KeepalivePolicy::DEFAULT_INTERVAL,
            )?,
            ssh_keepalive_failures: self.parsed_or(
                "VB_SSH_KEEPALIVE_FAILURES",
                crate::transport::lifecycle::KeepalivePolicy::DEFAULT_FAILURES,
            )?,
            transport_shutdown_grace: self.parsed_or(
                "VB_TRANSPORT_SHUTDOWN_GRACE",
                crate::transport::lifecycle::ShutdownCoordinator::DEFAULT_GRACE,
            )?,
            cadence_cshrc: self.text("VB_CADENCE_CSHRC"),
            spectre_bin: self.text("VB_SPECTRE_BIN"),
            roles: RemoteRoles {
                gui_host: self.text("VB_GUI_HOST"),
                deploy_host: self.text("VB_DEPLOY_HOST"),
                daemon_host: self.text("VB_DAEMON_HOST"),
                spectre_host: self.text("VB_SPECTRE_HOST"),
                scratch_root: self.text("VB_REMOTE_SCRATCH_ROOT"),
            },
            transport_daemon_socket: self.text("VB_TRANSPORT_DAEMON_SOCKET"),
            transport_daemon_token: self.text("VB_TRANSPORT_DAEMON_TOKEN"),
            allow_cross_user_daemon: self.flag_truthy("VB_ALLOW_CROSS_USER_DAEMON"),
        })
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
        //
        // `Layers::to_config` centralises the field-by-field read so that
        // `build_report` and `from_env_resolve` share ONE code path — any
        // change to a field's resolution rule only needs to live in ONE place.
        let file = ConfigFile::load()?;
        let lz = Layers {
            profile,
            file: file.as_ref(),
        };
        lz.to_config()
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

    /// Build a per-key provenance report for `vcli config check`.
    ///
    /// The report re-runs [`Layers::raw_with_source`] against the same
    /// `profile` and the file already loaded by the live resolution path, so
    /// what it shows **is what the runtime config will actually use** — no
    /// drift between the report and the next `vcli …` invocation. Two entry
    /// modes are honored:
    ///
    /// * **target mode** (`VB_TARGET` set and the target exists): every field
    ///   traces back to the `targets.yaml` entry; `Layer::Target { .. }`
    ///   records the target name. None of the file layers participate — that
    ///   is intentional, mirroring [`Self::from_env_resolve`]'s early return.
    /// * **legacy mode** (no `VB_TARGET`, or unknown target): the full
    ///   four-layer precedence is consulted.
    ///
    /// This function never returns `Err` — resolution failures are captured as
    /// [`ConfigDiagnostic`] entries with `level = "error"` and `valid = false`
    /// so `vcli config check --format json` always produces a valid document
    /// (with a non-zero exit code driven by the caller).
    ///
    /// `pre_check` carries an earlier selection error (e.g. active_target not
    /// found, corrupt targets.yaml) WITHOUT aborting the report. When
    /// `Err`, the report is marked `valid = false` and carries an
    /// `ILLEGAL_CONFIG` diagnostic describing the selection failure. The rest
    /// of the report is still generated so the user sees BOTH the selection
    /// problem and whatever config could be read.
    pub fn build_report(
        profile: Option<&str>,
        pre_check: std::result::Result<(), &VirtuosoError>,
    ) -> Result<ConfigReport> {
        let mut report = ConfigReport {
            profile: profile.map(|s| s.to_string()),
            active_target: None,
            config_file: None,
            targets_file: None,
            precedence: [
                "<KEY>_<PROFILE> env",
                "<KEY> env",
                "config.toml [profile.<PROFILE>]",
                "config.toml global",
            ],
            entries: Vec::with_capacity(KEY_SPECS.len()),
            warnings: Vec::new(),
            diagnostics: Vec::new(),
            valid: true,
            digest: None,
        };

        // Pre-check: selection/resolve failures detected before we got here.
        // Mark invalid, emit a diagnostic, but KEEP going — seeing the
        // selection error alongside whatever config we *can* read is more
        // useful than bailing out.
        if let Err(e) = pre_check {
            report.valid = false;
            let msg = format!("config selection failed: {e}");
            report.warnings.push(msg.clone());
            report.diagnostics.push(ConfigDiagnostic {
                code: "ILLEGAL_CONFIG",
                level: "error",
                field: "selection".into(),
                message: msg,
            });
        }

        // Active target detection — mirrors from_env_resolve's honor_vb_target
        // branch. We never *consume* it here; we only report whether one is
        // driving the resolution so the user can see "your 30-second timeout
        // came from the prod target, not from any VB_… env var".
        if let Ok(name) = std::env::var("VB_TARGET") {
            if !name.is_empty() {
                report.active_target = Some(name);
            }
        }

        // Surface the deprecated ~/.vcli/.env VB_PROFILE= fallback so the
        // report tells users their profile moved. Cheap (one fopen); we only
        // warn when the file exists with a non-empty VB_PROFILE=.
        if let Some(home) = dirs::home_dir() {
            let legacy_env = home.join(".vcli").join(".env");
            if legacy_env.is_file() {
                if let Ok(content) = std::fs::read_to_string(&legacy_env) {
                    if content.lines().any(|l| {
                        let t = l.trim();
                        t.starts_with("VB_PROFILE=") && !t["VB_PROFILE=".len()..].trim().is_empty()
                    }) {
                        let msg = format!(
                            "{}: VB_PROFILE= is a deprecated fallback. Run `vcli profile bind \
                             <name> --user` (or export VB_PROFILE in your shell) — this file \
                             will stop being read in a future release.",
                            legacy_env.display()
                        );
                        report.warnings.push(msg.clone());
                        report.diagnostics.push(ConfigDiagnostic {
                            code: "LEGACY_ENV_PROFILE",
                            level: "warning",
                            field: "profile".into(),
                            message: msg,
                        });
                    }
                }
            }
        }

        if let Some(ref target_name) = report.active_target.clone() {
            // Target mode: load the target and trace every key to it.
            report.targets_file = Some(crate::target::TargetManager::default_config_path());
            let manager = match crate::target::TargetManager::load() {
                Ok(m) => m,
                Err(e) => {
                    let msg = format!("failed to load targets: {e}");
                    report.diagnostics.push(ConfigDiagnostic {
                        code: "TARGETS_LOAD_FAILED",
                        level: "error",
                        field: "targets.yaml".into(),
                        message: msg.clone(),
                    });
                    report.valid = false;
                    report.warnings.push(msg);
                    return Ok(report);
                }
            };
            let target = match manager.get(target_name) {
                Some(t) => t,
                None => {
                    let msg = format!("target '{}' not found", target_name);
                    report.diagnostics.push(ConfigDiagnostic {
                        code: "TARGET_NOT_FOUND",
                        level: "error",
                        field: target_name.clone(),
                        message: msg.clone(),
                    });
                    report.valid = false;
                    report.warnings.push(msg);
                    return Ok(report);
                }
            };
            let mut report = build_report_from_target(target, target_name, report);
            // Compute digest from the synthetic config so the report carries the
            // same identity the runtime would use. If construction fails here
            // the same error would block the live path — surface it as an
            // error rather than returning a misleading valid=true + empty
            // digest.
            match Config::from_target(target, target_name) {
                Ok(cfg) => report.digest = Some(cfg.digest()),
                Err(e) => {
                    let msg = format!(
                        "resolved target '{}' failed to build a Config: {e} — \
                         the same error would block the runtime",
                        target_name
                    );
                    report.valid = false;
                    report.warnings.push(msg.clone());
                    report.diagnostics.push(ConfigDiagnostic {
                        code: "TARGET_CONFIG_BUILD_FAILED",
                        level: "error",
                        field: target_name.clone(),
                        message: msg,
                    });
                }
            }
            return Ok(report);
        }

        // Legacy mode: walk the four-layer precedence for every known key.
        let file = match ConfigFile::load() {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("failed to load config.toml: {e}");
                report.diagnostics.push(ConfigDiagnostic {
                    code: "CONFIG_LOAD_FAILED",
                    level: "error",
                    field: "config.toml".into(),
                    message: msg.clone(),
                });
                report.valid = false;
                report.warnings.push(msg);
                // Still report defaults so the user sees what would be used.
                for spec in KEY_SPECS {
                    let kind = if spec.key == "VB_PORT" {
                        DefaultKind::Derived
                    } else {
                        DefaultKind::Hardcoded
                    };
                    report.entries.push(ConfigEntry {
                        key: spec.key,
                        value: spec.default.to_string(),
                        source: ConfigSource::Default {
                            default: spec.default.to_string(),
                            kind,
                        },
                    });
                }
                return Ok(report);
            }
        };
        report.config_file = file.as_ref().map(|_| crate::config_file::path());
        let lz = Layers {
            profile,
            file: file.as_ref(),
        };
        for spec in KEY_SPECS {
            let (raw, source) = match lz.raw_with_source(spec.key) {
                Some((v, s)) => (v, s),
                None => {
                    let kind = if spec.key == "VB_PORT" {
                        DefaultKind::Derived
                    } else {
                        DefaultKind::Hardcoded
                    };
                    (
                        spec.default.to_string(),
                        ConfigSource::Default {
                            default: spec.default.to_string(),
                            kind,
                        },
                    )
                }
            };
            // Apply the same scalar validators as the live path so a value
            // that would fail at runtime shows up here *with the same error
            // message*. We surface parse failures as warnings rather than
            // aborting — the user is asking "what would happen?", not
            // "tunnel now".
            //
            // Sanitise sensitive values (tokens, key paths) before they reach
            // JSON / table output. Returns e.g. "***set***" in place of the
            // raw secret when the key is sensitive and the value is non-empty.
            let display_value = sanitize_raw_value(spec.key, &raw);
            match (spec.validate)(&raw) {
                Ok(()) => report.entries.push(ConfigEntry {
                    key: spec.key,
                    value: display_value,
                    source,
                }),
                Err(e) => {
                    report.entries.push(ConfigEntry {
                        key: spec.key,
                        value: display_value,
                        source,
                    });
                    let msg = format!(
                        "{}: {e} — this value would fail to parse at runtime",
                        spec.key
                    );
                    report.warnings.push(msg.clone());
                    // A value that cannot be parsed is NOT a valid runtime
                    // config — mark the report invalid so --format json carries
                    // valid=false and the exit code is non-zero. Previously
                    // this only produced a warning, so an invalid timeout etc.
                    // slipped through as valid=true + exit 0.
                    report.valid = false;
                    report.diagnostics.push(ConfigDiagnostic {
                        code: "VALUE_PARSE_FAILED",
                        level: "error",
                        field: spec.key.to_string(),
                        message: msg,
                    });
                }
            }
        }
        // Compute digest from the SAME `Layers` (and therefore the SAME
        // `ConfigFile` snapshot) the per-field loop just traced. This is the
        // single-snapshot rule in action: `build_report` does ONE
        // `ConfigFile::load()` and uses its result for both provenance and
        // identity. Previously this called `Config::from_env_with_profile()`
        // which re-loaded `config.toml` — two snapshots, double I/O, and a
        // window where the file could change between the two passes.
        match lz.to_config() {
            Ok(cfg) => report.digest = Some(cfg.digest()),
            Err(e) => {
                let msg = format!(
                    "failed to build Config from the merged resolution: {e} — \
                     the runtime would refuse to start"
                );
                report.valid = false;
                report.warnings.push(msg.clone());
                report.diagnostics.push(ConfigDiagnostic {
                    code: "CONFIG_BUILD_FAILED",
                    level: "error",
                    field: "config".into(),
                    message: msg,
                });
                report.digest = None;
            }
        }
        Ok(report)
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

#[cfg(test)]
mod report_tests {
    //! Unit tests for [`Config::build_report`].
    //!
    //! Every test that touches the env or the filesystem must hold an
    //! `EnvGuard` that resets `VB_CONFIG_DIR`, every `VB_*` variable we read
    //! directly, and clears the test env so the report actually observes the
    //! state the test sets up. Without the guard, a developer machine with a
    //! `~/.vcli/config.toml` would silently override the test fixture and the
    //! assertion would drift.

    use super::*;
    use serial_test::serial;

    /// Save and restore the keys this module's resolution actually consults.
    ///
    /// We don't blanket-clear every `VB_*` — that would mask real bugs in the
    /// report's ability to attribute values. We restore whatever was set on
    /// entry, blank the env for the duration of the test, and only seed the
    /// specific keys the test exercises. The `Drop` impl restores the saved
    /// state so a failing assertion doesn't poison the next test.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        saved_profile: Option<String>,
        saved_target: Option<String>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let mut saved: Vec<(&'static str, Option<String>)> =
                keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            // Always preserve these — they belong to the framework, not the
            // caller's choice, and we want them unchanged even if the test
            // didn't mention them.
            for k in ["VB_PROFILE", "VB_TARGET"].iter() {
                if !saved.iter().any(|(name, _)| *name == *k) {
                    saved.push((k, std::env::var(k).ok()));
                }
            }
            let saved_profile = std::env::var("VB_PROFILE").ok();
            let saved_target = std::env::var("VB_TARGET").ok();
            for k in keys {
                std::env::remove_var(k);
            }
            std::env::remove_var("VB_PROFILE");
            std::env::remove_var("VB_TARGET");
            Self {
                saved,
                saved_profile,
                saved_target,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(value) => std::env::set_var(k, value),
                    None => std::env::remove_var(k),
                }
            }
            match &self.saved_profile {
                Some(v) => std::env::set_var("VB_PROFILE", v),
                None => std::env::remove_var("VB_PROFILE"),
            }
            match &self.saved_target {
                Some(v) => std::env::set_var("VB_TARGET", v),
                None => std::env::remove_var("VB_TARGET"),
            }
        }
    }

    /// Point `VB_CONFIG_DIR` at a fresh empty tempdir so `ConfigFile::load()`
    /// observes `Ok(None)` and the report runs the "no file" branch. The
    /// guard restores the prior `VB_CONFIG_DIR` (or removes it if unset).
    fn isolate_config_dir() -> (tempfile::TempDir, Option<std::ffi::OsString>) {
        let prev = std::env::var_os("VB_CONFIG_DIR");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("VB_CONFIG_DIR", dir.path());
        (dir, prev)
    }

    struct ConfigDirGuard(Option<std::ffi::OsString>);

    impl Drop for ConfigDirGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("VB_CONFIG_DIR", v),
                None => std::env::remove_var("VB_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial]
    fn report_with_no_env_and_no_file_attributes_everything_to_default() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST", "VB_TIMEOUT"]);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        let report = Config::build_report(None, Ok(())).expect("build_report");
        let remote_host = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(remote_host.value, "");
        assert!(matches!(remote_host.source, ConfigSource::Default { .. }));
        let timeout = report
            .entries
            .iter()
            .find(|e| e.key == "VB_TIMEOUT")
            .expect("entry exists");
        assert_eq!(timeout.value, "30");
        assert!(matches!(timeout.source, ConfigSource::Default { .. }));
        assert!(!report.all_explicit());
    }

    #[test]
    #[serial]
    fn report_attributes_env_var_to_env_layer() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST"]);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        std::env::set_var("VB_REMOTE_HOST", "eda-from-env");
        let report = Config::build_report(None, Ok(())).expect("build_report");
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(e.value, "eda-from-env");
        match &e.source {
            ConfigSource::Env { var } => assert_eq!(var, "VB_REMOTE_HOST"),
            other => panic!("expected Env source, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn report_prefers_profile_specific_env_over_global_env() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST"]);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        std::env::set_var("VB_REMOTE_HOST", "global-host");
        std::env::set_var("VB_REMOTE_HOST_prod", "profile-host");
        let report = Config::build_report(Some("prod"), Ok(())).expect("build_report");
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(e.value, "profile-host");
        match &e.source {
            ConfigSource::EnvProfile { var } => assert_eq!(var, "VB_REMOTE_HOST_prod"),
            other => panic!("expected EnvProfile source, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn report_attributes_file_global_when_no_env_present() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST"]);
        let (tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        // runtime_paths::config_subdir joins `vcli/` under VB_CONFIG_DIR.
        // Match the live layout so ConfigFile::load() finds the file we
        // write — it doesn't auto-create parent directories.
        std::fs::create_dir_all(tmp.path().join("vcli")).expect("mkdir");
        std::fs::write(
            tmp.path().join("vcli/config.toml"),
            r#"remote_host = "eda-from-file""#,
        )
        .expect("write config");
        let report = Config::build_report(None, Ok(())).expect("build_report");
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(e.value, "eda-from-file");
        assert!(matches!(e.source, ConfigSource::FileGlobal { .. }));
    }

    #[test]
    #[serial]
    fn report_prefers_file_profile_section_over_file_global() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST", "VB_REMOTE_HOST_prod"]);
        let (tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        std::fs::create_dir_all(tmp.path().join("vcli")).expect("mkdir");
        std::fs::write(
            tmp.path().join("vcli/config.toml"),
            r#"
remote_host = "eda-global"

[profile.prod]
remote_host = "eda-prod"
"#,
        )
        .expect("write config");
        let report = Config::build_report(Some("prod"), Ok(())).expect("build_report");
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(e.value, "eda-prod");
        assert!(matches!(e.source, ConfigSource::FileProfile { .. }));
    }

    #[test]
    #[serial]
    fn report_flags_unparseable_value_as_warning() {
        let _env = EnvGuard::new(&["VB_TIMEOUT"]);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        std::env::set_var("VB_TIMEOUT", "not-a-number");
        let report = Config::build_report(None, Ok(())).expect("build_report");
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_TIMEOUT")
            .expect("entry exists");
        // The raw value is still surfaced so the user can see what was set;
        // the warning tells them it would fail at runtime.
        assert_eq!(e.value, "not-a-number");
        assert!(matches!(e.source, ConfigSource::Env { .. }));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("VB_TIMEOUT") && w.contains("parse")),
            "expected parse warning, got: {:?}",
            report.warnings
        );
    }

    #[test]
    #[serial]
    fn report_target_mode_attributes_explicit_target_field_to_target() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST"]);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        // Write a targets.yaml with one explicit target.
        let target_yaml = r#"
active_target: prod
targets:
  prod:
    remote_host: target-host
    port: 65535
"#;
        std::fs::write(_tmp.path().join("targets.yaml"), target_yaml).expect("write targets");
        std::env::set_var("VB_TARGETS_FILE", _tmp.path().join("targets.yaml"));
        std::env::set_var("VB_TARGET", "prod");
        // Manually invoking build_report here is tricky because the active
        // target is detected via VB_TARGET but the target loader uses
        // VB_TARGETS_FILE — we still need to guard that variable too.
        let report = Config::build_report(None, Ok(())).expect("build_report");
        assert_eq!(report.active_target.as_deref(), Some("prod"));
        let e = report
            .entries
            .iter()
            .find(|e| e.key == "VB_REMOTE_HOST")
            .expect("entry exists");
        assert_eq!(e.value, "target-host");
        assert!(matches!(&e.source, ConfigSource::Target { target, .. } if target == "prod"));
        // Clean up the VB_TARGETS_FILE we added — Drop on EnvGuard doesn't
        // know about it because it's not in the spec list.
        std::env::remove_var("VB_TARGETS_FILE");
    }

    #[test]
    #[serial]
    // `dirs::home_dir()` honours `$HOME` on unix only; on Windows it reads
    // `USERPROFILE`, so pointing `HOME` at a tempdir would not make the
    // `.env` visible and the warning would never fire.
    #[cfg(unix)]
    fn report_legacy_env_dotenv_fallback_emits_warning() {
        let _env = EnvGuard::new(&["VB_REMOTE_HOST"]);
        // Pretend `~/.vcli/.env` exists with VB_PROFILE=set, by setting HOME
        // to a tempdir.
        let prev_home = std::env::var_os("HOME");
        let dir = tempfile::tempdir().expect("home");
        std::fs::create_dir_all(dir.path().join(".vcli")).expect("mkdir");
        std::fs::write(dir.path().join(".vcli/.env"), "VB_PROFILE=stale-binding\n")
            .expect("write env");
        std::env::set_var("HOME", dir.path());
        // Force a fresh config dir so the file lookup is consistent.
        let (_cfg, prev_cfg) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev_cfg);
        let report = Config::build_report(None, Ok(())).expect("build_report");
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        assert!(
            report.warnings.iter().any(|w| w.contains("deprecated")),
            "expected deprecated .env fallback warning, got: {:?}",
            report.warnings
        );
    }

    #[test]
    #[serial]
    fn all_explicit_is_true_when_no_default_sources() {
        // Shield every key the report consults. The body of the test sets
        // each one to a non-empty value; without an explicit shield list,
        // EnvGuard::Drop would only restore the original value for keys the
        // test passed to `new`, leaking "x" into the rest of the suite.
        let all_keys: Vec<&'static str> = KEY_SPECS.iter().map(|s| s.key).collect();
        let _env = EnvGuard::new(&all_keys);
        let (_tmp, prev) = isolate_config_dir();
        let _restore = ConfigDirGuard(prev);
        for k in KEY_SPECS.iter().map(|s| s.key) {
            std::env::set_var(k, "x");
        }
        let report = Config::build_report(None, Ok(())).expect("build_report");
        assert!(report.all_explicit(), "every entry must be non-default");
    }

    #[test]
    fn report_pre_check_error_marks_invalid_with_diagnostic() {
        let err = VirtuosoError::Config("active_target 'nope' not found".into());
        let report = Config::build_report(None, Err(&err)).expect("build_report");
        assert!(!report.valid, "pre_check error must flip valid to false");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "ILLEGAL_CONFIG"),
            "must emit an ILLEGAL_CONFIG diagnostic"
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.message.contains("nope")),
            "diagnostic message must surface the underlying failure"
        );
    }
}

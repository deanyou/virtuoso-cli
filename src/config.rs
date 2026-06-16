use crate::error::{Result, VirtuosoError};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
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
    /// Disable SSH ControlMaster multiplexing (VB_DISABLE_CONTROL_MASTER=1).
    /// Set this on WSL2/Windows when the CM socket path contains non-ASCII chars.
    pub disable_control_master: bool,
    pub timeout: u64,
    pub keep_remote_files: bool,
    pub spectre_cmd: String,
    pub spectre_args: Vec<String>,
    /// Maximum parallel Spectre compute threads (VB_SPECTRE_MAX_WORKERS, default: 8)
    pub spectre_max_workers: u32,
    /// Path to Cadence environment setup file (VB_CADENCE_CSHRC).
    /// Used to load Spectre environment for remote SSH execution.
    pub cadence_cshrc: Option<String>,
    /// Absolute path to Spectre binary (VB_SPECTRE_BIN).
    /// When set, this path is used directly instead of relying on PATH.
    /// Useful when Spectre is not in PATH or multiple versions exist.
    pub spectre_bin: Option<String>,
}

impl Config {
    /// Read a config variable, checking profile-specific first (e.g. VB_REMOTE_HOST_prod).
    fn env_with_profile(key: &str, profile: Option<&str>) -> Option<String> {
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
        load_dotenv_upward();

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
            disable_control_master: Self::env_with_profile("VB_DISABLE_CONTROL_MASTER", profile)
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            timeout: Self::env_with_profile("VB_TIMEOUT", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
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
            cadence_cshrc: Self::env_with_profile("VB_CADENCE_CSHRC", profile),
            spectre_bin: Self::env_with_profile("VB_SPECTRE_BIN", profile),
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

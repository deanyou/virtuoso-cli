use crate::error::{Result, VirtuosoError};
use dotenvy::dotenv;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub profile: Option<String>,
    pub remote_host: Option<String>,
    pub remote_user: Option<String>,
    pub port: u16,
    pub jump_host: Option<String>,
    pub jump_user: Option<String>,
    pub timeout: u64,
    pub keep_remote_files: bool,
    pub spectre_cmd: String,
    pub spectre_args: Vec<String>,
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
        let profile = env::var("VB_PROFILE").ok();
        Self::from_env_with_profile(profile.as_deref())
    }

    pub fn from_env_with_profile(profile: Option<&str>) -> Result<Self> {
        if let Err(e) = dotenv() {
            if !e.not_found() {
                tracing::warn!("failed to load .env: {e}");
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

        Ok(Self {
            profile: profile.map(|s| s.to_string()),
            remote_host,
            remote_user: Self::env_with_profile("VB_REMOTE_USER", profile),
            port,
            jump_host: Self::env_with_profile("VB_JUMP_HOST", profile),
            jump_user: Self::env_with_profile("VB_JUMP_USER", profile),
            timeout: Self::env_with_profile("VB_TIMEOUT", profile)
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            keep_remote_files: Self::env_with_profile("VB_KEEP_REMOTE_FILES", profile)
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            spectre_cmd: Self::env_with_profile("VB_SPECTRE_CMD", profile)
                .unwrap_or_else(|| "spectre".into()),
            spectre_args: Self::env_with_profile("VB_SPECTRE_ARGS", profile)
                .map(|v| shlex::split(&v).unwrap_or_default())
                .unwrap_or_default(),
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

    pub fn ssh_target(&self) -> String {
        let host = self.remote_host.as_deref().unwrap_or("");
        match &self.remote_user {
            Some(user) => format!("{user}@{host}"),
            None => host.to_string(),
        }
    }

    pub fn ssh_jump(&self) -> Option<String> {
        match (&self.jump_host, &self.jump_user) {
            (Some(host), Some(user)) => Some(format!("{user}@{host}")),
            (Some(host), None) => Some(host.clone()),
            _ => None,
        }
    }
}

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

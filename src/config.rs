use crate::error::{Result, VirtuosoError};
use dotenvy::dotenv;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
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
    pub fn from_env() -> Result<Self> {
        if let Err(e) = dotenv() {
            if !e.not_found() {
                tracing::warn!("failed to load .env: {e}");
            }
        }

        let remote_host = env::var("VB_REMOTE_HOST").ok().filter(|s| !s.is_empty());

        let port: u16 = env::var("VB_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(65432);

        if port == 0 {
            return Err(VirtuosoError::Config(
                "VB_PORT must be between 1 and 65535".into(),
            ));
        }

        Ok(Self {
            remote_host,
            remote_user: env::var("VB_REMOTE_USER").ok(),
            port,
            jump_host: env::var("VB_JUMP_HOST").ok(),
            jump_user: env::var("VB_JUMP_USER").ok(),
            timeout: env::var("VB_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            keep_remote_files: env::var("VB_KEEP_REMOTE_FILES")
                .ok()
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            spectre_cmd: env::var("VB_SPECTRE_CMD")
                .ok()
                .unwrap_or_else(|| "spectre".into()),
            spectre_args: env::var("VB_SPECTRE_ARGS")
                .ok()
                .map(|v| shlex::split(&v).unwrap_or_default())
                .unwrap_or_default(),
        })
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

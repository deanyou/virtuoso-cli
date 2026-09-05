//! Multi-target connection pool management.
//!
//! Allows vcli to manage multiple Virtuoso targets (host+port combinations)
//! via a YAML configuration file, instead of relying solely on environment
//! variables. Each target has its own connection pool.

pub mod resolve;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for a single Virtuoso target.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TargetConfig {
    /// Remote host running Virtuoso (compute host, not jump host)
    pub remote_host: Option<String>,
    /// SSH username for remote host
    pub remote_user: Option<String>,
    /// Bridge port on the remote host
    pub port: Option<u16>,
    /// Jump host (bastion) for SSH tunneling
    pub jump_host: Option<String>,
    /// SSH username for jump host
    pub jump_user: Option<String>,
    /// SSH port (default 22)
    pub ssh_port: Option<u16>,
    /// Path to SSH private key
    pub ssh_key: Option<String>,
    /// Path to custom SSH config file
    pub ssh_config: Option<String>,
    /// SSH backend: "openssh" (default) or "native"
    pub ssh_backend: Option<String>,
    /// Disable SSH ControlMaster multiplexing
    pub disable_control_master: Option<bool>,
    /// Timeout for write operations in seconds (default 30)
    pub timeout: Option<u64>,
    /// Timeout for read operations in seconds (default 120)
    pub read_timeout: Option<u64>,
    /// Keep remote files after operation
    pub keep_remote_files: Option<bool>,
    /// Spectre command (default "spectre")
    pub spectre_cmd: Option<String>,
    /// Additional Spectre arguments
    pub spectre_args: Option<Vec<String>>,
    /// Maximum parallel Spectre workers (default 8)
    pub spectre_max_workers: Option<u32>,
    /// Path to Cadence environment setup file
    pub cadence_cshrc: Option<String>,
    /// Absolute path to Spectre binary
    pub spectre_bin: Option<String>,
    /// Native SSH: max concurrent sessions (default 10)
    pub ssh_max_sessions: Option<usize>,
    /// Native SSH: max bulk transfer sessions (default 2)
    pub ssh_max_bulk_sessions: Option<usize>,
    /// Native SSH: reconnect attempts before degraded (default 8)
    pub ssh_reconnect_max_attempts: Option<u32>,
    /// Native SSH: max reconnect delay in seconds (default 30)
    pub ssh_reconnect_max_delay: Option<u64>,
    /// Native SSH: keepalive interval in seconds (default 30)
    pub ssh_keepalive_interval: Option<u64>,
    /// Native SSH: missed keepalives before dead (default 3)
    pub ssh_keepalive_failures: Option<u32>,
    /// Transport daemon IPC socket path
    pub transport_daemon_socket: Option<String>,
    /// Transport daemon auth token
    pub transport_daemon_token: Option<String>,
    /// Human-readable description
    pub description: Option<String>,
}

/// Top-level targets configuration file structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TargetsConfig {
    /// Map of target name → configuration
    pub targets: HashMap<String, TargetConfig>,
    /// Name of the currently active target (used when --target is not specified)
    #[serde(default)]
    pub active_target: Option<String>,
}

/// Manages multiple Virtuoso targets and their connection pools.
pub struct TargetManager {
    config: TargetsConfig,
    config_path: PathBuf,
}

impl TargetManager {
    /// Default config file path: ~/.vcli/targets.yaml
    pub fn default_config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".vcli")
            .join("targets.yaml")
    }

    /// Load targets from the default config path.
    /// Returns an empty manager if the file doesn't exist.
    pub fn load() -> Result<Self, TargetError> {
        Self::load_from(&Self::default_config_path())
    }

    /// Load targets from a specific config file path.
    pub fn load_from(path: &std::path::Path) -> Result<Self, TargetError> {
        if !path.exists() {
            return Ok(Self {
                config: TargetsConfig::default(),
                config_path: path.to_path_buf(),
            });
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| TargetError::Io(format!("failed to read {}: {e}", path.display())))?;

        let config: TargetsConfig = serde_yaml::from_str(&content)
            .map_err(|e| TargetError::Parse(format!("failed to parse {}: {e}", path.display())))?;

        Ok(Self {
            config,
            config_path: path.to_path_buf(),
        })
    }

    /// Get a target by name.
    pub fn get(&self, name: &str) -> Option<&TargetConfig> {
        self.config.targets.get(name)
    }

    /// Get the active target (or "default" if no active is set).
    pub fn active_target_name(&self) -> &str {
        self.config.active_target.as_deref().unwrap_or("default")
    }

    /// Get the active target configuration.
    pub fn active_target(&self) -> Option<&TargetConfig> {
        self.get(self.active_target_name())
    }

    /// The raw `active_target` field value, if explicitly set (no "default"
    /// fallback). Lets the resolver distinguish "unset" from "set but the
    /// target definition is missing" so the latter can be reported as an error.
    pub fn active_target_raw(&self) -> Option<&str> {
        self.config.active_target.as_deref()
    }

    /// List all target names.
    pub fn list_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.config.targets.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Add or update a target.
    pub fn set(&mut self, name: &str, config: TargetConfig) {
        self.config.targets.insert(name.to_string(), config);
    }

    /// Remove a target. Returns true if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.config.targets.remove(name).is_some()
    }

    /// Set the active target. Returns error if the target doesn't exist.
    pub fn set_active(&mut self, name: &str) -> Result<(), TargetError> {
        if !self.config.targets.contains_key(name) {
            return Err(TargetError::NotFound(name.to_string()));
        }
        self.config.active_target = Some(name.to_string());
        Ok(())
    }

    /// Clear the active target (e.g. after the active target is removed).
    pub fn clear_active(&mut self) {
        self.config.active_target = None;
    }

    /// Save configuration to disk.
    pub fn save(&self) -> Result<(), TargetError> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                TargetError::Io(format!("failed to create {}: {e}", parent.display()))
            })?;
        }

        let yaml = serde_yaml::to_string(&self.config)
            .map_err(|e| TargetError::Parse(format!("failed to serialize config: {e}")))?;

        std::fs::write(&self.config_path, yaml).map_err(|e| {
            TargetError::Io(format!(
                "failed to write {}: {e}",
                self.config_path.display()
            ))
        })?;

        Ok(())
    }

    /// Get the config file path.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Check if the manager has any targets configured.
    pub fn is_empty(&self) -> bool {
        self.config.targets.is_empty()
    }
}

/// Errors for target management.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Target not found: {0}")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_config_serialization() {
        let config = TargetConfig {
            remote_host: Some("compute-eda-42".to_string()),
            port: Some(30001),
            ssh_backend: Some("native".to_string()),
            ..Default::default()
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: TargetConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.remote_host, Some("compute-eda-42".to_string()));
        assert_eq!(parsed.port, Some(30001));
        assert_eq!(parsed.ssh_backend, Some("native".to_string()));
    }

    #[test]
    fn test_targets_config_serialization() {
        let mut targets = HashMap::new();
        targets.insert(
            "prod".to_string(),
            TargetConfig {
                remote_host: Some("compute-eda-42".to_string()),
                port: Some(30001),
                ..Default::default()
            },
        );
        targets.insert(
            "test".to_string(),
            TargetConfig {
                remote_host: Some("compute-eda-99".to_string()),
                port: Some(30002),
                ..Default::default()
            },
        );

        let config = TargetsConfig {
            targets,
            active_target: Some("prod".to_string()),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: TargetsConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.targets.len(), 2);
        assert_eq!(parsed.active_target, Some("prod".to_string()));
        assert!(parsed.targets.contains_key("prod"));
        assert!(parsed.targets.contains_key("test"));
    }

    #[test]
    fn test_target_manager_get_and_set() {
        let mut manager = TargetManager {
            config: TargetsConfig::default(),
            config_path: PathBuf::from("/tmp/test_targets.yaml"),
        };

        assert!(manager.is_empty());

        manager.set(
            "prod",
            TargetConfig {
                remote_host: Some("host1".to_string()),
                port: Some(1000),
                ..Default::default()
            },
        );

        assert!(!manager.is_empty());
        assert!(manager.get("prod").is_some());
        assert_eq!(manager.get("prod").unwrap().port, Some(1000));
        assert_eq!(manager.list_names(), vec!["prod"]);
    }

    #[test]
    fn test_target_manager_set_active() {
        let mut manager = TargetManager {
            config: TargetsConfig::default(),
            config_path: PathBuf::from("/tmp/test_targets.yaml"),
        };

        manager.set("prod", TargetConfig::default());
        assert!(manager.set_active("prod").is_ok());
        assert_eq!(manager.active_target_name(), "prod");
        assert!(manager.set_active("nonexistent").is_err());
    }

    #[test]
    fn test_target_manager_remove() {
        let mut manager = TargetManager {
            config: TargetsConfig::default(),
            config_path: PathBuf::from("/tmp/test_targets.yaml"),
        };

        manager.set("prod", TargetConfig::default());
        assert!(manager.remove("prod"));
        assert!(!manager.remove("prod"));
        assert!(manager.is_empty());
    }
}

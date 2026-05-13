//! Capability plugin system.
//!
//! Supports:
//! - YAML config files for capability definitions
//! - Capability inheritance (role-based)
//! - Rate limiting per capability
//! - IP whitelist per capability
//! - Capability expiration with TTL

use std::collections::HashMap;
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Rate limiter for RPC calls.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Max requests per window
    max_requests: u32,
    /// Window duration in seconds
    window_secs: u64,
    /// Request timestamps
    requests: Vec<Instant>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            max_requests,
            window_secs,
            requests: Vec::new(),
        }
    }

    /// Check if request is allowed. Returns true if allowed, false if rate limited.
    pub fn check(&mut self) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(self.window_secs);

        // Remove old requests outside the window
        self.requests.retain(|t| now.duration_since(*t) < window);

        if self.requests.len() >= self.max_requests as usize {
            return false;
        }

        self.requests.push(now);
        true
    }

    /// Get remaining requests in current window.
    pub fn remaining(&self) -> u32 {
        self.max_requests.saturating_sub(self.requests.len() as u32)
    }

    /// Get reset time in seconds.
    pub fn reset_in(&self) -> u64 {
        if self.requests.is_empty() {
            return 0;
        }
        let oldest = self.requests.iter().min().unwrap();
        let elapsed = Instant::now().duration_since(*oldest);
        self.window_secs.saturating_sub(elapsed.as_secs())
    }
}

/// IP whitelist entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IpEntry {
    pub ip: String,
    pub description: Option<String>,
}

impl IpEntry {
    pub fn matches(&self, addr: &IpAddr) -> bool {
        // Try exact match first
        if let Ok(target) = addr.to_string().parse::<IpAddr>() {
            if let Ok(entry_ip) = self.ip.parse::<IpAddr>() {
                return target == entry_ip;
            }
        }

        // Try CIDR matching
        if let Some((ip, bits)) = self.ip.split_once('/') {
            if let (Ok(network), Ok(client)) =
                (ip.parse::<IpAddr>(), addr.to_string().parse::<IpAddr>())
            {
                let mask = if bits.parse::<u32>().unwrap_or(32) == 32 {
                    !0u32
                } else {
                    !0u32 << (32 - bits.parse::<u32>().unwrap_or(32))
                };
                return (to_ipv4(&network) & mask) == (to_ipv4(&client) & mask);
            }
        }
        false
    }
}

fn to_ipv4(ip: &IpAddr) -> u32 {
    match ip {
        IpAddr::V4(v4) => u32::from(*v4),
        IpAddr::V6(_) => 0,
    }
}

/// Capability definition from config.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityDef {
    pub name: String,
    pub domains: Vec<String>,
    pub inherits: Option<Vec<String>>,
    pub rate_limit: Option<RateLimitDef>,
    pub ip_whitelist: Option<Vec<IpEntry>>,
    pub ttl_seconds: Option<u64>,
    pub description: Option<String>,
}

/// Rate limit definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitDef {
    pub max_requests: u32,
    pub window_seconds: u64,
}

/// Plugin config file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginConfig {
    pub version: String,
    pub capabilities: Vec<CapabilityDef>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            version: "1.0".into(),
            capabilities: Vec::new(),
        }
    }
}

/// Capability plugin manager.
#[derive(Debug, Clone)]
pub struct CapabilityPlugin {
    /// Config loaded from file
    config: PluginConfig,
    /// Computed capability hierarchy (resolved inheritance)
    resolved: Arc<RwLock<HashMap<String, ResolvedCapability>>>,
    /// Rate limiters per capability
    rate_limiters: Arc<RwLock<HashMap<String, RateLimiter>>>,
    /// Expiration times per granted capability
    expirations: Arc<RwLock<HashMap<String, Instant>>>,
    /// Config file path
    config_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ResolvedCapability {
    pub name: String,
    pub domains: Vec<String>,
    pub rate_limit: Option<RateLimitDef>,
    pub ip_whitelist: Vec<IpEntry>,
    pub ttl_seconds: Option<u64>,
}

impl CapabilityPlugin {
    /// Create a new plugin with default config.
    pub fn new() -> Self {
        Self {
            config: PluginConfig::default(),
            resolved: Arc::new(RwLock::new(HashMap::new())),
            rate_limiters: Arc::new(RwLock::new(HashMap::new())),
            expirations: Arc::new(RwLock::new(HashMap::new())),
            config_path: None,
        }
    }

    /// Load config from YAML file.
    pub fn load<P: Into<PathBuf>>(path: P) -> std::io::Result<Self> {
        let path = path.into();
        let content = fs::read_to_string(&path)?;
        let config: PluginConfig = serde_yaml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut plugin = Self {
            config,
            resolved: Arc::new(RwLock::new(HashMap::new())),
            rate_limiters: Arc::new(RwLock::new(HashMap::new())),
            expirations: Arc::new(RwLock::new(HashMap::new())),
            config_path: Some(path),
        };

        plugin.resolve_inheritance();
        Ok(plugin)
    }

    /// Save config to YAML file.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(ref path) = self.config_path {
            let content = serde_yaml::to_string(&self.config)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            fs::write(path, content)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no config path set",
            ))
        }
    }

    /// Resolve capability inheritance.
    fn resolve_inheritance(&mut self) {
        let mut resolved = HashMap::new();

        // First pass: add all capabilities without inheritance
        for cap in &self.config.capabilities {
            if cap.inherits.is_none() {
                let domains = cap.domains.clone();
                let rate_limit = cap.rate_limit.clone();
                let ip_whitelist = cap.ip_whitelist.clone().unwrap_or_default();

                resolved.insert(
                    cap.name.clone(),
                    ResolvedCapability {
                        name: cap.name.clone(),
                        domains,
                        rate_limit,
                        ip_whitelist,
                        ttl_seconds: cap.ttl_seconds,
                    },
                );
            }
        }

        // Second pass: resolve inheritance
        for cap in &self.config.capabilities {
            if let Some(ref inherits) = cap.inherits {
                let mut domains = cap.domains.clone();
                let mut ip_whitelist = cap.ip_whitelist.clone().unwrap_or_default();
                let mut rate_limit = cap.rate_limit.clone();
                let mut ttl = cap.ttl_seconds;

                // Collect from parent capabilities
                for parent_name in inherits {
                    if let Some(parent) = resolved.get(parent_name) {
                        domains.extend(parent.domains.iter().cloned());
                        ip_whitelist.extend(parent.ip_whitelist.clone());
                        if rate_limit.is_none() {
                            rate_limit = parent.rate_limit.clone();
                        }
                        if ttl.is_none() {
                            ttl = parent.ttl_seconds;
                        }
                    }
                }

                // Deduplicate
                domains.sort();
                domains.dedup();

                resolved.insert(
                    cap.name.clone(),
                    ResolvedCapability {
                        name: cap.name.clone(),
                        domains,
                        rate_limit,
                        ip_whitelist,
                        ttl_seconds: ttl,
                    },
                );
            }
        }

        *self.resolved.write().unwrap() = resolved;
    }

    /// Add a capability definition dynamically.
    pub fn add_capability(&mut self, def: CapabilityDef) {
        self.config.capabilities.push(def);
        self.resolve_inheritance();
    }

    /// Get a capability by name.
    pub fn get_capability(&self, name: &str) -> Option<ResolvedCapability> {
        self.resolved.read().unwrap().get(name).cloned()
    }

    /// List all capability names.
    pub fn list_capabilities(&self) -> Vec<String> {
        self.resolved.read().unwrap().keys().cloned().collect()
    }

    /// Check if a domain is allowed for a capability.
    pub fn permits_domain(&self, cap_name: &str, domain: &str) -> bool {
        if let Some(cap) = self.get_capability(cap_name) {
            cap.domains.iter().any(|d| {
                if d == "*" {
                    true
                } else if d.ends_with(".*") {
                    // Wildcard match: "schematic.*" matches "schematic.list_instances"
                    let prefix = &d[..d.len() - 2];
                    domain.starts_with(prefix) && domain.len() > prefix.len()
                } else {
                    d == domain
                }
            })
        } else {
            false
        }
    }

    /// Check if an IP is allowed for a capability.
    pub fn permits_ip(&self, cap_name: &str, ip: &IpAddr) -> bool {
        if let Some(cap) = self.get_capability(cap_name) {
            cap.ip_whitelist.is_empty() || cap.ip_whitelist.iter().any(|e| e.matches(ip))
        } else {
            false
        }
    }

    /// Check rate limit for a capability. Returns true if allowed.
    pub fn check_rate_limit(&self, cap_name: &str) -> bool {
        if let Some(cap) = self.get_capability(cap_name) {
            if let Some(ref rl) = cap.rate_limit {
                let mut limiters = self.rate_limiters.write().unwrap();
                let limiter = limiters
                    .entry(cap_name.to_string())
                    .or_insert_with(|| RateLimiter::new(rl.max_requests, rl.window_seconds));
                limiter.check()
            } else {
                true
            }
        } else {
            true
        }
    }

    /// Get rate limit status for a capability.
    /// Returns None if no rate limiter has been created yet, or if revoked.
    pub fn rate_limit_status(&self, cap_name: &str) -> Option<(u32, u64)> {
        let limiters = self.rate_limiters.read().unwrap();
        limiters
            .get(cap_name)
            .map(|limiter| (limiter.remaining(), limiter.reset_in()))
    }

    /// Set expiration for a granted capability.
    pub fn set_expiration(&self, cap_name: &str, ttl_secs: u64) {
        let mut expirations = self.expirations.write().unwrap();
        expirations.insert(
            cap_name.to_string(),
            Instant::now() + Duration::from_secs(ttl_secs),
        );
    }

    /// Check if a capability has expired.
    pub fn is_expired(&self, cap_name: &str) -> bool {
        let expirations = self.expirations.read().unwrap();
        if let Some(expires_at) = expirations.get(cap_name) {
            Instant::now() > *expires_at
        } else {
            false
        }
    }

    /// Get remaining TTL for a capability.
    pub fn remaining_ttl(&self, cap_name: &str) -> Option<Duration> {
        let expirations = self.expirations.read().unwrap();
        expirations
            .get(cap_name)
            .map(|expires_at| expires_at.saturating_duration_since(Instant::now()))
    }

    /// Revoke a capability immediately.
    pub fn revoke(&self, cap_name: &str) {
        let mut expirations = self.expirations.write().unwrap();
        expirations.remove(cap_name);
        let mut limiters = self.rate_limiters.write().unwrap();
        limiters.remove(cap_name);
    }

    /// Get the raw config.
    pub fn config(&self) -> &PluginConfig {
        &self.config
    }
}

impl Default for CapabilityPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_basic() {
        let mut limiter = RateLimiter::new(3, 60);
        assert!(limiter.check()); // 1
        assert!(limiter.check()); // 2
        assert!(limiter.check()); // 3
        assert!(!limiter.check()); // denied
        assert_eq!(limiter.remaining(), 0);
    }

    #[test]
    fn test_ip_entry_exact() {
        let entry = IpEntry {
            ip: "192.168.1.100".into(),
            description: None,
        };
        assert!(entry.matches(&"192.168.1.100".parse().unwrap()));
        assert!(!entry.matches(&"192.168.1.101".parse().unwrap()));
    }

    #[test]
    fn test_capability_inheritance() {
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "base".into(),
            domains: vec!["util.*".into()],
            inherits: None,
            rate_limit: Some(RateLimitDef {
                max_requests: 100,
                window_seconds: 60,
            }),
            ip_whitelist: Some(vec![IpEntry {
                ip: "127.0.0.1".into(),
                description: None,
            }]),
            ttl_seconds: None,
            description: None,
        });
        plugin.add_capability(CapabilityDef {
            name: "extended".into(),
            domains: vec!["schematic.*".into()],
            inherits: Some(vec!["base".into()]),
            rate_limit: None,
            ip_whitelist: None,
            ttl_seconds: Some(3600),
            description: None,
        });

        let ext = plugin.get_capability("extended").unwrap();
        assert!(ext.domains.contains(&"schematic.*".into()));
        assert!(ext.domains.contains(&"util.*".into())); // inherited
        assert_eq!(ext.rate_limit.as_ref().unwrap().max_requests, 100); // inherited
        assert_eq!(ext.ip_whitelist.len(), 1); // inherited
        assert_eq!(ext.ttl_seconds, Some(3600)); // own
    }

    #[test]
    fn test_permits_domain() {
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "test".into(),
            domains: vec!["schematic.*".into(), "maestro.run".into()],
            inherits: None,
            rate_limit: None,
            ip_whitelist: None,
            ttl_seconds: None,
            description: None,
        });

        assert!(plugin.permits_domain("test", "schematic.list_instances"));
        assert!(plugin.permits_domain("test", "maestro.run"));
        assert!(!plugin.permits_domain("test", "window.list"));
        assert!(!plugin.permits_domain("nonexistent", "anything"));
    }

    #[test]
    fn test_rate_limit_tracking() {
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "limited".into(),
            domains: vec!["*".into()],
            inherits: None,
            rate_limit: Some(RateLimitDef {
                max_requests: 2,
                window_seconds: 60,
            }),
            ip_whitelist: None,
            ttl_seconds: None,
            description: None,
        });

        assert!(plugin.check_rate_limit("limited"));
        assert!(plugin.check_rate_limit("limited"));
        assert!(!plugin.check_rate_limit("limited"));
        assert!(!plugin.check_rate_limit("limited"));
    }

    #[test]
    fn test_expiration() {
        let plugin = CapabilityPlugin::new();
        plugin.set_expiration("temp", 1);

        assert!(!plugin.is_expired("temp"));
        std::thread::sleep(Duration::from_secs(2));
        assert!(plugin.is_expired("temp"));
    }

    #[test]
    fn test_revoke() {
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "revoke_test".into(),
            domains: vec!["*".into()],
            inherits: None,
            rate_limit: Some(RateLimitDef {
                max_requests: 1,
                window_seconds: 60,
            }),
            ip_whitelist: None,
            ttl_seconds: None,
            description: None,
        });

        // Initially no limiter exists
        assert!(plugin.rate_limit_status("revoke_test").is_none());

        // Trigger rate limit by making requests
        plugin.check_rate_limit("revoke_test");
        assert_eq!(plugin.rate_limit_status("revoke_test").unwrap().0, 0);

        // After revoke, limiter is removed
        plugin.revoke("revoke_test");
        assert!(plugin.rate_limit_status("revoke_test").is_none());
    }

    #[test]
    fn test_yaml_config() {
        let yaml = r#"
version: "1.0"
capabilities:
  - name: designer
    domains:
      - schematic.*
      - maestro.*
    inherits:
      - viewer
    rate_limit:
      max_requests: 100
      window_seconds: 60
    ip_whitelist:
      - ip: "10.0.0.0/8"
        description: "Internal network"
    ttl_seconds: 86400
    description: "Full design access"
  - name: viewer
    domains:
      - window.*
      - util.*
    rate_limit:
      max_requests: 50
      window_seconds: 60
"#;
        let config: PluginConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.capabilities.len(), 2);
        assert_eq!(config.capabilities[0].name, "designer");
        assert!(config.capabilities[0]
            .inherits
            .as_ref()
            .unwrap()
            .contains(&"viewer".into()));
    }
}

//! Capability-based permission model.
//!
//! Controls which RPC domains and operations a client can access.
//! Supports:
//! - Environment variable loading (VCLI_CAPABILITY)
//! - YAML config file loading
//! - Capability inheritance
//! - Rate limiting
//! - IP whitelist
//! - Capability expiration

mod plugin;
pub use plugin::*;

use std::collections::HashSet;
use std::env;
use std::net::IpAddr;

/// High-level capability categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Schematic,
    Maestro,
    Window,
    Cell,
    Simulation,
    Transaction,
    /// Allow raw SKILL exec (dangerous — local dev only)
    Admin,
}

impl Capability {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "schematic" => Some(Self::Schematic),
            "maestro" => Some(Self::Maestro),
            "window" => Some(Self::Window),
            "cell" => Some(Self::Cell),
            "simulation" => Some(Self::Simulation),
            "transaction" => Some(Self::Transaction),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// Returns the domain prefix used in RPC method names.
    pub fn domain(&self) -> &'static str {
        match self {
            Self::Schematic => "schematic",
            Self::Maestro => "maestro",
            Self::Window => "window",
            Self::Cell => "cell",
            Self::Simulation => "simulation",
            Self::Transaction => "transaction",
            Self::Admin => "*",
        }
    }

    /// Returns true if this capability allows admin/raw skill exec.
    pub fn is_admin(&self) -> bool {
        *self == Self::Admin
    }
}

/// A set of capabilities loaded from environment.
#[derive(Debug, Clone)]
pub struct CapabilitySet(HashSet<Capability>);

impl CapabilitySet {
    /// Load from VCLI_CAPABILITY env var (comma-separated).
    pub fn from_env() -> Self {
        let caps = env::var("VCLI_CAPABILITY")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|part| Capability::from_str(part.trim()))
                    .collect()
            })
            .unwrap_or_else(|| {
                // Default: allow all capabilities (existing behavior)
                let mut set = HashSet::new();
                set.insert(Capability::Schematic);
                set.insert(Capability::Maestro);
                set.insert(Capability::Window);
                set.insert(Capability::Cell);
                set.insert(Capability::Simulation);
                set.insert(Capability::Transaction);
                set
            });
        Self(caps)
    }

    /// Check if this set includes the given capability.
    pub fn permits(&self, cap: Capability) -> bool {
        self.0.contains(&cap) || self.0.contains(&Capability::Admin)
    }

    /// Check if a specific RPC method name is permitted.
    /// Method names are "domain.operation" (e.g. "schematic.place").
    pub fn permits_method(&self, method: &str) -> bool {
        let domain = method.split('.').next().unwrap_or("");
        match domain {
            "schematic" => self.permits(Capability::Schematic),
            "maestro" => self.permits(Capability::Maestro),
            "window" => self.permits(Capability::Window),
            "cell" => self.permits(Capability::Cell),
            "tx" => self.permits(Capability::Transaction),
            "file" => true,                     // File operations require full access
            "util" => true,                     // Utility methods are always allowed
            "skill" => self.allows_raw_skill(), // Only Admin can execute raw SKILL
            _ => false,
        }
    }

    /// Returns true if raw SKILL exec is allowed (Admin capability).
    pub fn allows_raw_skill(&self) -> bool {
        self.0.contains(&Capability::Admin)
    }

    /// Check if a specific RPC method name is permitted, with plugin integration.
    /// Also checks rate limiting, IP whitelist, and expiration if plugin is provided.
    pub fn permits_with_plugin(
        &self,
        method: &str,
        plugin: Option<&CapabilityPlugin>,
        client_ip: Option<&IpAddr>,
    ) -> Result<(), CapabilityError> {
        // First check basic capability permission
        if !self.permits_method(method) {
            return Err(CapabilityError::NotPermitted(method.to_string()));
        }

        // If plugin is available, check extended rules
        if let Some(pl) = plugin {
            // Check rate limiting
            for cap_name in &self.0 {
                let name = format!("{:?}", cap_name).to_lowercase();
                if !pl.check_rate_limit(&name) {
                    let status = pl.rate_limit_status(&name);
                    return Err(CapabilityError::RateLimited {
                        capability: name,
                        remaining: status.map(|(r, _)| r).unwrap_or(0),
                        reset_in: status.map(|(_, t)| t).unwrap_or(60),
                    });
                }
            }

            // Check IP whitelist
            for cap_name in &self.0 {
                let name = format!("{:?}", cap_name).to_lowercase();
                if !pl.permits_ip(&name, client_ip.unwrap_or(&"127.0.0.1".parse().unwrap())) {
                    return Err(CapabilityError::IpNotAllowed(
                        client_ip
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                    ));
                }
            }

            // Check expiration
            for cap_name in &self.0 {
                let name = format!("{:?}", cap_name).to_lowercase();
                if pl.is_expired(&name) {
                    return Err(CapabilityError::Expired(name));
                }
            }
        }

        Ok(())
    }
}

/// Errors from capability checks.
#[derive(Debug, Clone)]
pub enum CapabilityError {
    /// Method not permitted for this capability set
    NotPermitted(String),
    /// Rate limited
    RateLimited {
        capability: String,
        remaining: u32,
        reset_in: u64,
    },
    /// IP not in whitelist
    IpNotAllowed(String),
    /// Capability has expired
    Expired(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPermitted(m) => write!(f, "method '{}' not permitted", m),
            Self::RateLimited {
                capability,
                remaining,
                reset_in,
            } => {
                write!(
                    f,
                    "rate limited for '{}': {} remaining, resets in {}s",
                    capability, remaining, reset_in
                )
            }
            Self::IpNotAllowed(ip) => write!(f, "IP '{}' not in whitelist", ip),
            Self::Expired(cap) => write!(f, "capability '{}' has expired", cap),
        }
    }
}

impl std::error::Error for CapabilityError {}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_domain() {
        assert_eq!(Capability::Schematic.domain(), "schematic");
        assert_eq!(Capability::Maestro.domain(), "maestro");
        assert_eq!(Capability::Window.domain(), "window");
        assert_eq!(Capability::Cell.domain(), "cell");
    }

    #[test]
    fn permits_method() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic, Capability::Maestro]));
        assert!(caps.permits_method("schematic.place"));
        assert!(caps.permits_method("maestro.open_session"));
        assert!(!caps.permits_method("window.list"));
        assert!(!caps.permits_method("cell.open"));
    }

    #[test]
    fn permits_tx_methods() {
        let caps = CapabilitySet(HashSet::from([Capability::Transaction]));
        assert!(caps.permits_method("tx.begin"));
        assert!(caps.permits_method("tx.commit"));
        assert!(caps.permits_method("tx.rollback"));
        assert!(caps.permits_method("tx.diff"));
        assert!(caps.permits_method("tx.status"));
    }

    #[test]
    fn permits_file_methods() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        // file operations should be permitted with any capability
        assert!(caps.permits_method("file.upload"));
        assert!(caps.permits_method("file.download"));
    }

    #[test]
    fn permits_util_methods() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        // util methods should be permitted with any capability
        assert!(caps.permits_method("util.version"));
        assert!(caps.permits_method("util.ping"));
        assert!(caps.permits_method("util.ciw_print"));
    }

    #[test]
    fn skill_methods_require_admin() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        // skill methods should NOT be permitted without Admin
        assert!(!caps.permits_method("skill.exec"));
        assert!(!caps.permits_method("skill.load"));
    }

    #[test]
    fn admin_allows_everything() {
        let caps = CapabilitySet(HashSet::from([Capability::Admin]));
        assert!(caps.permits_method("schematic.place"));
        assert!(caps.permits_method("maestro.run"));
        assert!(caps.permits_method("window.list"));
        assert!(caps.permits_method("cell.open"));
        assert!(caps.permits_method("tx.begin"));
        assert!(caps.permits_method("file.upload"));
        assert!(caps.permits_method("util.version"));
        assert!(caps.permits_method("skill.exec"));
    }

    #[test]
    fn capability_from_str() {
        assert_eq!(
            Capability::from_str("schematic"),
            Some(Capability::Schematic)
        );
        assert_eq!(Capability::from_str("MAESTRO"), Some(Capability::Maestro));
        assert_eq!(Capability::from_str("admin"), Some(Capability::Admin));
        assert_eq!(Capability::from_str("unknown"), None);
    }

    #[test]
    fn permits_with_plugin_rate_limit() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "schematic".into(),
            domains: vec!["schematic.*".into()],
            inherits: None,
            rate_limit: Some(RateLimitDef {
                max_requests: 1,
                window_seconds: 60,
            }),
            ip_whitelist: None,
            ttl_seconds: None,
            description: None,
        });

        // First request should pass
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(caps
            .permits_with_plugin("schematic.list", Some(&plugin), Some(&ip))
            .is_ok());

        // Second request should be rate limited
        let result = caps.permits_with_plugin("schematic.list", Some(&plugin), Some(&ip));
        assert!(result.is_err());
        if let Err(CapabilityError::RateLimited { .. }) = result {
            // expected
        } else {
            panic!("expected rate limit error");
        }
    }

    #[test]
    fn permits_with_plugin_ip_whitelist() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "schematic".into(),
            domains: vec!["schematic.*".into()],
            inherits: None,
            rate_limit: None,
            ip_whitelist: Some(vec![IpEntry {
                ip: "10.0.0.0/8".into(),
                description: Some("Internal".into()),
            }]),
            ttl_seconds: None,
            description: None,
        });

        // Internal IP should pass
        let internal_ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(caps
            .permits_with_plugin("schematic.list", Some(&plugin), Some(&internal_ip))
            .is_ok());

        // External IP should fail
        let external_ip: IpAddr = "8.8.8.8".parse().unwrap();
        let result = caps.permits_with_plugin("schematic.list", Some(&plugin), Some(&external_ip));
        assert!(matches!(result, Err(CapabilityError::IpNotAllowed(_))));
    }

    #[test]
    fn permits_with_plugin_expiration() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "schematic".into(),
            domains: vec!["schematic.*".into()],
            inherits: None,
            rate_limit: None,
            ip_whitelist: None,
            ttl_seconds: Some(1),
            description: None,
        });

        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Set expiration explicitly
        plugin.set_expiration("schematic", 1);

        // Should pass initially
        assert!(caps
            .permits_with_plugin("schematic.list", Some(&plugin), Some(&ip))
            .is_ok());

        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Should now fail due to expiration
        let result = caps.permits_with_plugin("schematic.list", Some(&plugin), Some(&ip));
        assert!(matches!(result, Err(CapabilityError::Expired(_))));
    }

    #[test]
    fn permits_with_plugin_not_permitted() {
        let caps = CapabilitySet(HashSet::from([Capability::Schematic]));
        let mut plugin = CapabilityPlugin::new();
        plugin.add_capability(CapabilityDef {
            name: "schematic".into(),
            domains: vec!["schematic.*".into()],
            inherits: None,
            rate_limit: None,
            ip_whitelist: None,
            ttl_seconds: None,
            description: None,
        });

        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Cell methods not in Schematic capability
        let result = caps.permits_with_plugin("cell.open", Some(&plugin), Some(&ip));
        assert!(matches!(result, Err(CapabilityError::NotPermitted(_))));
    }

    #[test]
    fn capability_error_display() {
        let err = CapabilityError::NotPermitted("test.method".into());
        assert_eq!(err.to_string(), "method 'test.method' not permitted");

        let err = CapabilityError::RateLimited {
            capability: "test".into(),
            remaining: 0,
            reset_in: 30,
        };
        assert!(err.to_string().contains("rate limited"));

        let err = CapabilityError::IpNotAllowed("1.2.3.4".into());
        assert!(err.to_string().contains("1.2.3.4"));

        let err = CapabilityError::Expired("test".into());
        assert!(err.to_string().contains("expired"));
    }
}

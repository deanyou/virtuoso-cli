//! Auth — API key validation and per-capability access control.
//!
//! API key is loaded from VCLI_API_KEY env var. The key itself is a shared
//! secret; validation is a simple constant-time comparison.
//!
//! Capabilities are loaded from VCLI_CAPABILITY env var (comma-separated list).
//! When auth is enabled, the validated API key's associated capabilities are
//! checked before each RPC call.
//!
//! Audit log is written to ~/.cache/virtuoso_bridge/logs/audit.log

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::capability::CapabilitySet;
use crate::error::VirtuosoError;

/// API key authentication state.
#[derive(Debug)]
pub struct Auth {
    api_key: Option<String>,
    /// Capabilities granted to the authenticated caller.
    capabilities: CapabilitySet,
    /// Rate limit state: number of failed auth attempts (clears on success)
    failed_attempts: std::sync::atomic::AtomicU32,
    /// Lockout until timestamp (seconds since epoch), set on too many failures
    lockout_until: std::sync::atomic::AtomicU64,
}

/// Singleton auth state.
static AUTH: std::sync::OnceLock<Auth> = std::sync::OnceLock::new();

impl Auth {
    /// Initialize auth from environment. Called once at startup.
    pub fn init() {
        let api_key = std::env::var("VCLI_API_KEY").ok().filter(|k| !k.is_empty());
        let enabled = api_key.is_some();
        // Load capabilities from VCLI_CAPABILITY (loaded from env by CapabilitySet::from_env)
        let capabilities = CapabilitySet::from_env();

        AUTH.get_or_init(|| Auth {
            api_key,
            capabilities,
            failed_attempts: std::sync::atomic::AtomicU32::new(0),
            lockout_until: std::sync::atomic::AtomicU64::new(0),
        });
    }

    /// Check if API key is valid (constant-time compare).
    /// Returns Ok(()) if valid, Err(VirtuosoError::Auth) otherwise.
    pub fn validate(&self, key: &str) -> Result<(), VirtuosoError> {
        if self.api_key.is_none() {
            return Ok(());
        }

        // Check lockout
        let lockout = self.lockout_until.load(Ordering::SeqCst);
        if lockout > 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now < lockout {
                return Err(VirtuosoError::Auth(format!(
                    "too many failed attempts — locked out for {}s",
                    lockout - now
                )));
            }
            // Lockout expired
            self.lockout_until.store(0, Ordering::SeqCst);
            self.failed_attempts.store(0, Ordering::SeqCst);
        }

        match &self.api_key {
            Some(expected) if constant_time_eq(key.as_bytes(), expected.as_bytes()) => {
                self.failed_attempts.store(0, Ordering::SeqCst);
                Ok(())
            }
            _ => {
                let fail = self.failed_attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if fail >= 5 {
                    // 5 failed attempts → 5 minute lockout
                    let lockout_until = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        + 300;
                    self.lockout_until.store(lockout_until, Ordering::SeqCst);
                    tracing::warn!("API auth locked out for 5 minutes after {} failures", fail);
                }
                Err(VirtuosoError::Auth("invalid API key".into()))
            }
        }
    }

    /// Returns true if auth is enabled (API key was provided).
    pub fn is_enabled(&self) -> bool {
        self.api_key.is_some()
    }

    /// Returns the capabilities granted to the authenticated caller.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Audit log entry for RPC calls.
#[derive(Debug, serde::Serialize)]
pub struct AuditEntry {
    pub ts: String,
    pub user: String,
    pub session_id: Option<String>,
    pub method: String,
    pub params: serde_json::Value,
    pub result: String,
    pub capabilities: Vec<String>,
}

impl AuditEntry {
    pub fn new(
        method: &str,
        params: serde_json::Value,
        result: &str,
        session_id: Option<&str>,
    ) -> Self {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".into());

        Self {
            ts: chrono::Local::now()
                .format("%Y-%m-%dT%H:%M:%S%.3f")
                .to_string(),
            user,
            session_id: session_id.map(|s| s.to_string()),
            method: method.to_string(),
            params,
            result: result.to_string(),
            capabilities: Vec::new(),
        }
    }

    /// Write this audit entry to the audit log.
    pub fn write(&self) {
        let path = audit_path();
        let line = serde_json::to_string(self).unwrap_or_default();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

fn audit_path() -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("virtuoso_bridge")
        .join("logs");
    let _ = fs::create_dir_all(&dir);
    dir.join("audit.log")
}

/// Log an RPC call with result.
pub fn log_rpc(method: &str, params: &serde_json::Value, result: &str, session_id: Option<&str>) {
    let entry = AuditEntry::new(method, params.clone(), result, session_id);
    entry.write();
}

/// Middleware for RPC dispatcher — checks API key and returns capabilities.
/// Returns Ok(CapabilitySet) if authorized, Err otherwise.
pub fn check_auth(api_key: Option<&str>) -> Result<CapabilitySet, VirtuosoError> {
    let auth = auth();

    if !auth.is_enabled() {
        return Ok(auth.capabilities().clone());
    }

    let key = api_key.ok_or_else(|| VirtuosoError::Auth("API key required".into()))?;
    auth.validate(key)?;
    Ok(auth.capabilities().clone())
}

/// Get the global auth instance.
pub fn auth() -> &'static Auth {
    AUTH.get()
        .expect("Auth::init() must be called before auth()")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"secretkey123", b"secretkey123"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq(b"secretkey123", b"secretkey456"));
    }

    #[test]
    fn constant_time_eq_different_len() {
        assert!(!constant_time_eq(b"short", b"muchlonger"));
    }
}

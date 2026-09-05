//! Integration tests for Config parsing.
//!
//! Isolation contract: `Config::from_env_with_profile()` loads `.env` files
//! from the current directory upward (so `~/.env` can leak values here), and
//! honours the ambient `VB_TARGET` bridge (which can jump the parse into a
//! target file). dotenvy's `load()` never overrides an already-set env var, so
//! a test that must see a specific value sets that var explicitly first, using
//! [`EnvGuard`] so the original value is restored on drop (RAII).
//!
//! Every env-reading/writing test is `#[serial]`: mutating the process
//! environment is shared state, so these tests must never run concurrently
//! with each other, and each must shield every variable its assertion depends
//! on — the mechanism is shared, not per-test.

/// RAII guard that sets an env var for the scope of the test and restores the
/// original value (or removes it if it was absent) on drop.
struct EnvGuard {
    restored: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    /// Set `key` to `val` for the rest of this scope, restoring the prior
    /// value on drop.
    fn set(key: &str, val: &str) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, val);
        Self {
            restored: vec![(key.to_string(), old)],
        }
    }

    /// Shield a variable from both the process env and any upward `.env`:
    /// dotenvy skips already-set vars, and `Config` treats an empty value as
    /// absent, so the parser falls back to its default. The original value is
    /// restored on drop.
    fn shield(key: &str) -> Self {
        Self::set(key, "")
    }

    fn restore(&mut self) {
        for (key, val) in self.restored.drain(..) {
            match val {
                Some(v) => std::env::set_var(&key, v),
                None => std::env::remove_var(&key),
            }
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Shield `VB_TARGET` so `from_env_with_profile` cannot jump the parse into a
/// target file. Used by every test below.
fn shield_target() -> EnvGuard {
    EnvGuard::shield("VB_TARGET")
}

/// Test that Config can be created without panicking.
#[serial_test::serial]
#[test]
fn test_config_from_env_works() {
    let _g = shield_target();
    let result = virtuoso_cli::config::Config::from_env_with_profile(None);
    assert!(result.is_ok());
    // Should have a valid config with reasonable defaults
    let config = result.unwrap();
    assert!(config.port > 0);
    assert!(config.timeout > 0);
}

/// Test that spectre_max_workers has a reasonable default, verified in
/// isolation (no ambient `VB_SPECTRE_MAX_WORKERS`, no target file).
#[serial_test::serial]
#[test]
fn test_config_spectre_max_workers_default() {
    let _g = EnvGuard::shield("VB_SPECTRE_MAX_WORKERS");
    let _t = shield_target();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    // Should be 8 by default
    assert_eq!(config.spectre_max_workers, 8);
}

/// The default timeout is 30, verified in isolation.
///
/// Both `VB_TIMEOUT` and `VB_TARGET` are shielded: the upward `.env` (which on
/// this machine sets `VB_TIMEOUT=60`) and any ambient target selection cannot
/// leak in. The prior values are restored on drop.
#[serial_test::serial]
#[test]
fn test_config_timeout_default_isolated() {
    let _g = EnvGuard::shield("VB_TIMEOUT");
    let _t = shield_target();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.timeout, 30);
}

/// An explicit ambient `VB_TIMEOUT` overrides both the default and any `.env`
/// value. `45` differs from the default (30) and from this machine's `~/.env`
/// (60), so a pass proves the process env var wins. `VB_TARGET` is shielded so
/// the parse stays on the legacy path.
#[serial_test::serial]
#[test]
fn test_config_timeout_env_override_wins() {
    let _g = EnvGuard::set("VB_TIMEOUT", "45");
    let _t = shield_target();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.timeout, 45);
}

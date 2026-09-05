//! Integration tests for Config parsing.
//!
//! `Config::from_env_with_profile()` loads `.env` files from the current
//! directory upward (so `~/.env` can leak values here). dotenvy's `load()`
//! never overrides an already-set env var, so tests that must see a specific
//! value set that var explicitly in the process first — that shields them from
//! `.env`. Env-mutating tests are marked `#[serial]`.

/// Test that Config can be created without panicking.
#[test]
fn test_config_from_env_works() {
    let result = virtuoso_cli::config::Config::from_env_with_profile(None);
    assert!(result.is_ok());
    // Should have a valid config with reasonable defaults
    let config = result.unwrap();
    assert!(config.port > 0);
    assert!(config.timeout > 0);
}

/// Test that spectre_max_workers has a reasonable default.
#[test]
fn test_config_spectre_max_workers_default() {
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    // Should be 8 by default
    assert_eq!(config.spectre_max_workers, 8);
}

/// The default timeout is 30, verified in isolation.
///
/// An empty `VB_TIMEOUT` is set first so the upward `.env` (which on this
/// machine sets `VB_TIMEOUT=60`) cannot leak in: dotenvy skips already-set
/// vars, and `Config` treats an empty value as absent, so the parser falls
/// back to the built-in default.
#[serial_test::serial]
#[test]
fn test_config_timeout_default_isolated() {
    std::env::set_var("VB_TIMEOUT", "");
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    std::env::remove_var("VB_TIMEOUT");
    assert_eq!(config.timeout, 30);
}

/// An explicit ambient `VB_TIMEOUT` overrides both the default and any `.env`
/// value. `45` differs from the default (30) and from this machine's `~/.env`
/// (60), so a pass proves the process env var wins.
#[serial_test::serial]
#[test]
fn test_config_timeout_env_override_wins() {
    std::env::set_var("VB_TIMEOUT", "45");
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    std::env::remove_var("VB_TIMEOUT");
    assert_eq!(config.timeout, 45);
}

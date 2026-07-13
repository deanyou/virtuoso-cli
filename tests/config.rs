//! Integration tests for Config parsing.
//!
//! Note: These tests are limited because Config::from_env_with_profile() loads
//! .env files from the current directory upward, which can override test values.
//! Only tests that don't depend on specific env values or are resilient to .env
//! overrides are included here.

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

/// Test that timeout has a reasonable default.
#[test]
fn test_config_timeout_default() {
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    // Should be 30 by default
    assert_eq!(config.timeout, 30);
}

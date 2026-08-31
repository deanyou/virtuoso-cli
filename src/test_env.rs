//! Tests that read or write environment variables must run serially.
//!
//! `std::env::set_var` mutates process-global state, and cargo runs tests in
//! parallel by default, so two tests that touch the same env var race each
//! other. The standard fix is the `#[serial]` attribute from the `serial_test`
//! crate: any test that reads or writes `std::env::*` (directly, or indirectly
//! via `Config::from_env()`) must be marked `#[serial]`, and the framework
//! acquires a global mutex around the test body.
//!
//! This module is intentionally empty — it exists so future test helpers have
//! a single, obvious home if they need to coordinate with the env.

#![cfg(test)]

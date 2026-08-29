//! One process-wide lock for tests that read or write environment variables.
//!
//! `std::env::set_var` mutates process-global state and cargo runs tests in
//! parallel threads, so two tests that touch env vars can corrupt each other's
//! view. The lock must be **one** static shared by every test module in the
//! binary: a `static ENV_LOCK` declared inside each `mod tests` is a *different*
//! mutex per module and gives no mutual exclusion between modules — which is
//! exactly the race this replaces.
//!
//! [`lock`] also recovers from poisoning, so a panicking test does not cascade
//! into every later env test failing on a poisoned mutex (that previously
//! turned one real failure into six and buried the cause).

#![cfg(test)]

use std::sync::{Mutex, MutexGuard};

/// The single environment lock for this test binary.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the environment lock, recovering from poisoning.
///
/// A test that panics while holding the lock should not sabotage unrelated
/// tests; the panic is already reported, so swallow the poisoning.
pub fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

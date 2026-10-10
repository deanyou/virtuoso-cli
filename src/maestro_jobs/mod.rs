//! Maestro persistent job management.
//!
//! Provides a file-backed JSON store for Maestro simulation jobs, mirroring
//! the semantics of virtuoso-bridge-lite PR #173.

mod store;

pub use store::{MaestroJob, MaestroJobStatus, MaestroJobStore};

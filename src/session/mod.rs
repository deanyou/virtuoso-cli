//! Session management — heartbeat, stale detection, reconnection.
//!
//! The heartbeat runs in a background thread and periodically pings
//! registered Virtuoso sessions to detect crashed processes.

pub mod heartbeat;

pub use crate::models::SessionInfo;
pub use heartbeat::{is_session_stale, SessionHeartbeat};

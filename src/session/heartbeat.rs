//! Heartbeat daemon — pings sessions to detect stale Virtuoso processes.
//!
//! Runs in the background and periodically checks that registered sessions
//! are still alive. Stale sessions are marked but not deleted (preserved for
//! user inspection/recovery).

use crate::client::bridge::VirtuosoClient;
use crate::error::Result;
use crate::models::SessionInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time;

/// Heartbeat state tracked per session.
#[derive(Debug)]
pub struct SessionHeartbeatState {
    pub session_id: String,
    pub last_heartbeat: SystemTime,
    pub is_stale: bool,
}

/// Heartbeat manager that periodically pings all registered sessions.
pub struct SessionHeartbeat {
    interval_secs: u64,
    stop_flag: Arc<AtomicBool>,
}

impl SessionHeartbeat {
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval_secs,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the heartbeat loop in a background tokio task.
    /// Returns immediately; the task runs until `stop()` is called or process exits.
    pub fn start(&self) {
        let interval_secs = self.interval_secs;
        let stop = self.stop_flag.clone();

        std::thread::spawn(move || {
            let rt = crate::async_runtime::runtime();
            rt.block_on(async move {
                let interval = Duration::from_secs(interval_secs);
                let mut interval_timer = time::interval(interval);
                tracing::info!("session heartbeat started (interval={}s)", interval_secs);

                loop {
                    interval_timer.tick().await;

                    if stop.load(Ordering::SeqCst) {
                        tracing::info!("session heartbeat stopped");
                        break;
                    }

                    if let Err(e) = Self::check_all_sessions() {
                        tracing::warn!("heartbeat check failed: {e}");
                    }
                }
            });
        });
    }

    /// Signal the heartbeat thread to stop.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Check all local sessions, ping each, and update stale status.
    /// For stale sessions, attempt reconnection and clear stale flag if successful.
    fn check_all_sessions() -> Result<()> {
        let sessions = SessionInfo::list().map_err(|e| {
            crate::error::VirtuosoError::Execution(format!("failed to list sessions: {e}"))
        })?;

        for session in sessions {
            let client = VirtuosoClient::new(&session.host, session.port, 5000);
            match client.reconnect_session(&session.id) {
                Ok(true) => {
                    // Session was stale but Virtuoso reconnected — stale flag already cleared
                    tracing::debug!(
                        "session '{}' is alive (was attempting reconnect)",
                        session.id
                    );
                }
                Ok(false) => {
                    // Session is still stale — mark it
                    tracing::warn!(
                        "session '{}' on port {} is stale (Virtuoso pid={} may have crashed)",
                        session.id,
                        session.port,
                        session.pid
                    );
                    if let Err(e) = Self::mark_stale(&session) {
                        tracing::warn!("failed to mark session '{}' as stale: {e}", session.id);
                    }
                }
                Err(e) => {
                    tracing::warn!("failed to check session '{}': {e}", session.id);
                }
            }
        }

        Ok(())
    }

    /// Mark a session as stale by appending `.stale` flag file.
    fn mark_stale(session: &SessionInfo) -> Result<()> {
        let dir = SessionInfo::sessions_dir();
        let stale_flag = dir.join(format!("{}.stale", session.id));
        std::fs::write(&stale_flag, "").map_err(|e| {
            crate::error::VirtuosoError::Execution(format!("failed to write stale flag: {e}"))
        })
    }

    /// Remove stale flag for a session (called on successful reconnect).
    pub fn clear_stale(session_id: &str) -> Result<()> {
        let dir = SessionInfo::sessions_dir();
        let stale_flag = dir.join(format!("{}.stale", session_id));
        if stale_flag.exists() {
            std::fs::remove_file(&stale_flag).map_err(|e| {
                crate::error::VirtuosoError::Execution(format!("failed to remove stale flag: {e}"))
            })?;
        }
        Ok(())
    }
}

/// Returns true if a session is marked stale.
pub fn is_session_stale(session_id: &str) -> bool {
    let dir = SessionInfo::sessions_dir();
    dir.join(format!("{}.stale", session_id)).exists()
}

//! Discover a live RAMIC Bridge daemon on a remote host.
//!
//! `tunnel attach` uses this to pick which Virtuoso session to plug into
//! without deploying a fresh daemon. The two helpers here are kept separate
//! from `SessionInfo` (which only deals with on-disk metadata) because the
//! question "is this daemon actually serving requests?" requires a live
//! remote probe — the session JSON file alone can be stale (daemon crashed
//! without cleaning its registration, or was launched by an earlier Virtuoso
//! run that has since exited).
//!
//! What we deliberately do **not** do here:
//! - Verify the daemon is piped to a live Virtuoso MPS via cdsServIpc. That
//!   would need either `/proc/<pid>/cmdline` parsing or a SKILL-side
//!   handshake; both add complexity for diminishing returns because the
//!   common failure mode (MPS died but daemon still listens) is caught by
//!   `tunnel diagnose`'s SKILL eval probe, which runs after the attach.
//! - Touch the on-disk session cache. `tunnel attach` calls
//!   `SessionInfo::sync_from_remote` itself; mixing writes here would
//!   duplicate that responsibility.

use crate::error::Result;
use crate::models::SessionInfo;
use crate::transport::contract::{CommandRequest, RemoteTransport};

/// `ss` invocation that returns listening sockets on a single port.
/// Empty output ⇒ the port is not listening. `-H` suppresses the header,
/// `-l` filters to listening sockets, `-t` to TCP. The `sport = :N` filter
/// is portable across iproute2 versions shipped with RHEL 8/9, Debian 11+,
/// Ubuntu 20.04+.
const SS_PROBE_TEMPLATE: &str = "ss -tlnH 'sport = :{port}' 2>/dev/null";

/// Probe the remote host for a listening socket on `port`.
///
/// Returns `Ok(true)` when `ss` reports at least one matching line. Any
/// error invoking `ss` (binary missing, permission denied, etc.) is folded
/// into `Ok(false)` — for the purposes of session selection, "could not
/// verify" is the same outcome as "not listening": we move on to the next
/// candidate. The connection-level error from the runner itself
/// (SSH failure, host unreachable) is propagated as `Err` because that
/// affects every candidate and is the user's real problem.
pub fn remote_port_alive(runner: &dyn RemoteTransport, port: u16) -> Result<bool> {
    let script = SS_PROBE_TEMPLATE.replace("{port}", &port.to_string());
    let result = runner.run_command(&CommandRequest::untimed(&script))?;
    Ok(!result.stdout.trim().is_empty())
}

/// Pick the best live session from a list of candidates.
///
/// Selection rules (in order):
/// 1. If `host_hint` is `Some(h)` and any live session has
///    `host == h`, prefer the most recent of those.
/// 2. Otherwise take the most recent live session overall.
///
/// "Most recent" sorts on `created` lexicographically. The field is a
/// SKILL `getCurrentTime()` string like `"Aug 30 12:03:58 2026"`, which
/// happens to sort correctly by wall-clock because SKILL's `Mon DD
/// HH:MM:SS YYYY` is month-major. If we ever switch to ISO-8601 in
/// ramic_bridge.il this keeps working.
///
/// A "live" session is one whose remote port is currently listening
/// (verified via [`remote_port_alive`]). Candidates whose probe fails are
/// silently dropped — `tunnel attach` is allowed to ignore dead daemons.
///
/// Returns:
/// - `Ok(Some(session))` when at least one live candidate exists.
/// - `Ok(None)` when every candidate is dead or the list is empty.
/// - `Err(_)` only when the underlying SSH probe itself fails (network,
///   auth) — not when individual ports happen to be unbound.
pub fn pick_live_session(
    sessions: Vec<SessionInfo>,
    runner: &dyn RemoteTransport,
    host_hint: Option<&str>,
) -> Result<Option<SessionInfo>> {
    if sessions.is_empty() {
        return Ok(None);
    }

    // Sort newest-first. `created` is `Mon DD HH:MM:SS YYYY`; for two
    // sessions from the same month this is wall-clock-ordered.
    let mut by_recency = sessions;
    by_recency.sort_by(|a, b| b.created.cmp(&a.created));

    let mut candidates: Vec<SessionInfo> = if let Some(host) = host_hint {
        let matching: Vec<SessionInfo> = by_recency
            .iter()
            .filter(|s| s.host == host)
            .cloned()
            .collect();
        if matching.is_empty() {
            by_recency
        } else {
            matching
        }
    } else {
        by_recency
    };

    // Walk in recency order; first one whose port is listening wins.
    while let Some(s) = candidates.first().cloned() {
        if remote_port_alive(runner, s.port)? {
            return Ok(Some(s));
        }
        candidates.remove(0);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::{
        CommandRequest, CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest,
        TransportError, UploadFileRequest, UploadTextRequest,
    };
    use std::sync::Mutex;
    use std::time::Duration;

    /// Stub runner that records every script it gets and replays a fixed
    /// sequence of stdout responses. `alive_ports` is the set of ports that
    /// should report "listening"; everything else reports empty stdout.
    struct StubRunner {
        /// Set of ports whose `ss -tlnH 'sport = :P'` should return a fake
        /// listening line. Iteration order over `HashSet` is unstable, so
        /// tests assert against the recorded commands instead of probing
        /// port order.
        alive_ports: std::collections::HashSet<u16>,
        commands: Mutex<Vec<String>>,
        /// When true, `run_command` returns an error (simulates SSH down).
        fail: bool,
    }

    impl StubRunner {
        fn new(alive: &[u16]) -> Self {
            Self {
                alive_ports: alive.iter().copied().collect(),
                commands: Mutex::new(Vec::new()),
                fail: false,
            }
        }
    }

    impl RemoteTransport for StubRunner {
        fn test_connection(
            &self,
            _deadline: Deadline,
        ) -> std::result::Result<bool, TransportError> {
            Ok(true)
        }

        fn run_command(
            &self,
            req: &CommandRequest,
        ) -> std::result::Result<CommandResult, TransportError> {
            if self.fail {
                return Err(TransportError::RemoteExit {
                    status: -1,
                    stderr: "stub-fail".into(),
                });
            }
            self.commands.lock().unwrap().push(req.command.clone());
            // Parse the port out of `ss -tlnH 'sport = :NNN'`. The template
            // emits exactly one colon immediately before the port, so
            // splitting on `:` and taking the segment right after it is
            // unambiguous.
            let port = req
                .command
                .split(':')
                .nth(1)
                .and_then(|s: &str| s.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s: &str| s.parse::<u16>().ok());
            let listening = port.map(|p| self.alive_ports.contains(&p)).unwrap_or(false);
            Ok(CommandResult {
                exit_status: 0,
                stdout: if listening {
                    "LISTEN 0  128  *:1234  *:*\n".into()
                } else {
                    String::new()
                },
                stderr: String::new(),
                success: true,
                duration: Duration::ZERO,
            })
        }

        fn upload_file(&self, _req: &UploadFileRequest) -> std::result::Result<(), TransportError> {
            Ok(())
        }

        fn upload_text(&self, _req: &UploadTextRequest) -> std::result::Result<(), TransportError> {
            Ok(())
        }

        fn download_file(
            &self,
            _req: &DownloadFileRequest,
        ) -> std::result::Result<(), TransportError> {
            Ok(())
        }

        fn download_dir(
            &self,
            _req: &DownloadDirRequest,
        ) -> std::result::Result<(), TransportError> {
            Ok(())
        }
    }

    fn session(id: &str, port: u16, host: &str, created: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            port,
            pid: 0,
            host: host.into(),
            user: "user1".into(),
            created: created.into(),
            daemon_user: None,
            daemon_version: None,
        }
    }

    #[test]
    fn empty_list_returns_none() {
        let runner = StubRunner::new(&[]);
        let result = pick_live_session(vec![], &runner, None).unwrap();
        assert!(result.is_none());
        // No port probes should have been issued.
        assert!(runner.commands.lock().unwrap().is_empty());
    }

    #[test]
    fn picks_live_session_when_present() {
        // The newer-looking candidate is dead; the older-looking one is
        // alive. Walking in recency order must probe the dead one first
        // (and only that one, because the loop returns on the first hit).
        let runner = StubRunner::new(&[46075]);
        let sessions = vec![
            session("new", 41469, "h", "Aug 30 12:03:00 2026"),
            session("older", 46075, "h", "Aug 30 10:55:00 2026"),
        ];
        let result = pick_live_session(sessions, &runner, Some("h"))
            .unwrap()
            .unwrap();
        assert_eq!(result.id, "older");
        // The 41469 candidate was probed (dead) and 46075 was probed (live).
        let cmds = runner.commands.lock().unwrap();
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn returns_none_when_all_dead() {
        let runner = StubRunner::new(&[]);
        let sessions = vec![
            session("a", 41469, "h", "Aug 30 10:55:00 2026"),
            session("b", 46075, "h", "Aug 30 12:03:00 2026"),
        ];
        let result = pick_live_session(sessions, &runner, None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn host_hint_prefers_matching_then_recency() {
        // host=h has 46075 (live) and 41469 (dead);
        // host=other has 39000 (live, newer).
        // With hint=Some("h"), the matcher picks from h's pool first
        // and returns 46075 (the live h-session, even though 39000 is newer).
        let runner = StubRunner::new(&[46075, 39000]);
        let sessions = vec![
            session("old-h", 41469, "h", "Aug 30 10:55:00 2026"),
            session("new-h", 46075, "h", "Aug 30 12:03:00 2026"),
            session("other", 39000, "other", "Aug 31 09:00:00 2026"),
        ];
        let result = pick_live_session(sessions, &runner, Some("h"))
            .unwrap()
            .unwrap();
        assert_eq!(result.id, "new-h");
    }

    #[test]
    fn no_host_hint_uses_recency_only() {
        let runner = StubRunner::new(&[39000]);
        let sessions = vec![
            session("old", 41469, "h", "Aug 30 10:55:00 2026"),
            session("newest", 39000, "other", "Aug 31 09:00:00 2026"),
            session("mid", 46075, "h", "Aug 30 12:03:00 2026"),
        ];
        let result = pick_live_session(sessions, &runner, None).unwrap().unwrap();
        assert_eq!(result.id, "newest");
    }

    #[test]
    fn ssh_failure_propagates() {
        let mut runner = StubRunner::new(&[46075]);
        runner.fail = true;
        let sessions = vec![session("s1", 46075, "h", "Aug 30 12:03:00 2026")];
        let result = pick_live_session(sessions, &runner, None);
        assert!(result.is_err());
    }
}

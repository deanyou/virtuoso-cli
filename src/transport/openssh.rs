//! OpenSSH backend for the [`RemoteTransport`] contract.
//!
//! This wraps the existing `SSHRunner` and is intentionally a pure translation
//! layer: it adds the contract vocabulary (request identity, deadlines,
//! structured errors) without changing what `SSHRunner` does. Nothing here
//! alters the ssh command line, the ControlMaster fallback policy, or the
//! timeout semantics. Existing call sites keep using `SSHRunner` directly until
//! they are migrated in a later increment.

#![allow(dead_code)]

use crate::config::Config;
use crate::error::VirtuosoError;
use crate::models::RemoteTaskResult;
use crate::transport::contract::{
    CommandRequest, CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest,
    RemoteTransport, RequestId, TransportError, UploadFileRequest, UploadTextRequest,
};
use crate::transport::ssh::SSHRunner;

/// The OpenSSH backend: one `SSHRunner` behind the transport contract.
pub struct OpenSshTransport {
    runner: SSHRunner,
}

impl OpenSshTransport {
    pub fn new(runner: SSHRunner) -> Self {
        Self { runner }
    }

    /// Build from configuration, mirroring `SSHRunner::from_config`.
    pub fn from_config(config: &Config) -> Self {
        Self::new(SSHRunner::from_config(config))
    }

    /// Honour `VB_DISABLE_CONTROL_MASTER`, which `SSHClient::from_env` applies
    /// but `SSHRunner::from_config` does not. Exposed so that migrating
    /// `SSHClient` call sites is a faithful move.
    pub fn with_control_master_disabled(self) -> Self {
        *self.runner.use_control_master.lock().unwrap() = false;
        self
    }

    /// Escape hatch for the migration period: lets a call site that has not
    /// fully moved to the contract still reach runner-specific behaviour.
    pub fn runner(&self) -> &SSHRunner {
        &self.runner
    }
}

/// Classify an `SSHRunner` failure without substring-matching its message.
///
/// The previous code inferred causes from stderr text (`summarize_error`). This
/// maps on the error *variant* instead. Where the variant does not carry enough
/// information to be certain — notably timeouts — it resolves toward the
/// conservative classification.
fn classify(id: &RequestId, e: VirtuosoError) -> TransportError {
    match e {
        VirtuosoError::Timeout(secs) | VirtuosoError::TimeoutWithContext(secs, _) => {
            // `SSHRunner` kills the local ssh client on timeout, which does not
            // prove the remote command stopped. A Spectre job can still be
            // running. Unknown outcome, therefore not retryable.
            TransportError::ExecutionTimeout {
                request: id.clone(),
                after_secs: secs,
                remote_terminated: false,
            }
        }
        VirtuosoError::Connection(m) | VirtuosoError::Ssh(m) => TransportError::ConnectionFailed(m),
        VirtuosoError::Config(m) => TransportError::Configuration(m),
        VirtuosoError::Auth(m) => TransportError::AuthenticationFailed(m),
        VirtuosoError::Io(e) => TransportError::LocalIo(e.to_string()),
        VirtuosoError::Execution(m) => TransportError::RemoteExit {
            status: -1,
            stderr: m,
        },
        other => TransportError::RemoteIo(other.to_string()),
    }
}

fn to_command_result(r: RemoteTaskResult) -> CommandResult {
    CommandResult {
        exit_status: r.returncode,
        stdout: r.stdout,
        stderr: r.stderr,
        success: r.success,
        duration: std::time::Duration::from_secs_f64(
            r.timings.get("total").copied().unwrap_or(0.0),
        ),
    }
}

impl RemoteTransport for OpenSshTransport {
    fn test_connection(&self, deadline: Deadline) -> Result<bool, TransportError> {
        self.runner
            .test_connection(Some(deadline.remaining_secs()))
            .map_err(|e| classify(&RequestId::default(), e))
    }

    fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        self.runner
            .run_command(&req.command, req.timeout.map(|d| d.as_secs()))
            .map(to_command_result)
            .map_err(|e| classify(&req.id, e))
    }

    fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        // `SSHRunner::upload` takes `&str`. A non-UTF-8 local path cannot be
        // represented without changing it to take a `&Path`, which would be a
        // behaviour change, so the limitation is recorded rather than papered
        // over. `download_dir` already accepts `&Path` and is unaffected.
        self.runner
            .upload(&req.local.to_string_lossy(), &req.remote)
            .map_err(|e| classify(&req.id, e))
    }

    fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        self.runner
            .upload_text(&req.text, &req.remote)
            .map_err(|e| classify(&req.id, e))
    }

    fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        self.runner
            .download(&req.remote, &req.local.to_string_lossy())
            .map_err(|e| classify(&req.id, e))
    }

    fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        self.runner
            .download_dir(&req.remote, &req.local)
            .map_err(|e| classify(&req.id, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::shared_contract_suite;
    use std::sync::Arc;
    use std::time::Duration;

    fn transport() -> OpenSshTransport {
        OpenSshTransport::new(SSHRunner::new("compute-eda-42"))
    }

    /// The bound the design requires. Locks it in at compile time for the real
    /// backend, not just the fake.
    #[test]
    fn open_ssh_transport_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenSshTransport>();
    }

    /// Dispatches through `Arc<dyn RemoteTransport>` — the shape business
    /// modules will hold once they are migrated.
    #[test]
    fn usable_behind_arc_dyn() {
        let t: Arc<dyn RemoteTransport> = Arc::new(transport());
        let past = Deadline(std::time::Instant::now() - Duration::from_secs(1));
        let req = CommandRequest::new("true", past);
        assert!(matches!(
            t.run_command(&req),
            Err(TransportError::QueueTimeout { .. })
        ));
    }

    #[test]
    fn passes_shared_contract_suite() {
        shared_contract_suite(&transport());
    }

    /// A deadline already in the past must fail with QueueTimeout rather than
    /// attempting to spawn ssh. Asserting the variant is what proves the
    /// contract is observable on the real backend.
    #[test]
    fn expired_deadline_reports_queue_timeout() {
        let t = transport();
        let past = Deadline(std::time::Instant::now() - Duration::from_secs(1));
        let req = CommandRequest::new("true", past);
        match t.run_command(&req) {
            Err(TransportError::QueueTimeout { after_secs, .. }) => assert_eq!(after_secs, 0),
            Err(other) => panic!("expected QueueTimeout, got {other:?}"),
            Ok(_) => panic!("expired deadline must not run"),
        }
    }

    #[test]
    fn forwarding_is_reported_not_panicked() {
        let t = transport();
        assert!(matches!(
            t.health(),
            Err(TransportError::UnsupportedOperation(_))
        ));
    }

    #[test]
    fn control_master_can_be_disabled_for_migration_parity() {
        let t = transport();
        assert!(*t.runner().use_control_master.lock().unwrap());
        let t = t.with_control_master_disabled();
        assert!(!*t.runner().use_control_master.lock().unwrap());
    }

    #[test]
    fn timeout_classifies_as_unproven_termination() {
        // Killing the local ssh client does not prove the remote command
        // stopped, so this must never be marked retryable.
        let e = classify(&RequestId::new(), VirtuosoError::Timeout(30));
        match e {
            TransportError::ExecutionTimeout {
                remote_terminated, ..
            } => {
                assert!(!remote_terminated);
                assert!(!e.retryable());
            }
            other => panic!("expected ExecutionTimeout, got {other:?}"),
        }
    }

    #[test]
    fn classification_does_not_read_message_text() {
        // Same variant, messages that would push a substring matcher in
        // opposite directions — classification must be identical.
        let a = classify(
            &RequestId::new(),
            VirtuosoError::Ssh("connection refused".into()),
        );
        let b = classify(
            &RequestId::new(),
            VirtuosoError::Ssh("safe to retry".into()),
        );
        assert_eq!(a.code(), b.code());
        assert_eq!(a.retryable(), b.retryable());
    }
}

//! The `RemoteTransport` contract.
//!
//! Business modules depend on this trait rather than on a concrete SSH
//! implementation, so the OpenSSH backend and the planned native backend can be
//! selected at runtime. This module owns the vocabulary of the contract —
//! request identity, deadlines, structured errors — and deliberately exposes no
//! russh, Tokio, SSH channel, or IPC types.
//!
//! The trait is defined before the native backend exists so that the contract is
//! provable against the backend that already works. See `OpenSshTransport` for
//! the current implementation.

#![allow(dead_code)]

use crate::error::VirtuosoError;
use crate::exit_codes;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ───────────────────────── request identity & timing ─────────────────────────

/// Identity of a single transport request.
///
/// Carried through to every structured error so that a caller can correlate a
/// failure with the request that produced it without parsing message strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

/// Absolute point in time after which a request stops waiting.
///
/// Queue time and execution time share one deadline: a request that has not
/// started when the deadline passes is reported as `QueueTimeout`, which proves
/// no remote operation began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline(pub Instant);

impl Deadline {
    pub fn from_now(timeout: Duration) -> Self {
        Self(Instant::now() + timeout)
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.0
    }

    /// Remaining budget, or zero once the deadline has passed.
    pub fn remaining(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }

    /// Seconds remaining, floored at 1 so that a caller building an SSH
    /// `ConnectTimeout` never passes zero by accident.
    pub fn remaining_secs(&self) -> u64 {
        self.remaining().as_secs().max(1)
    }
}

// ───────────────────────────────── requests ─────────────────────────────────

/// Execute a shell command on the remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub id: RequestId,
    pub deadline: Deadline,
    pub command: String,
    /// Execution timeout. `None` selects the transport's configured default.
    pub timeout: Option<Duration>,
}

/// Allowance added to an execution timeout to derive a request deadline.
///
/// Covers connection setup and queueing, neither of which is part of the
/// execution timeout the caller asked for.
pub const STARTUP_ALLOWANCE: Duration = Duration::from_secs(30);

impl CommandRequest {
    pub fn new(command: impl Into<String>, deadline: Deadline) -> Self {
        Self {
            id: RequestId::new(),
            deadline,
            command: command.into(),
            timeout: None,
        }
    }

    /// Build a request with an execution timeout and a deadline derived from it.
    ///
    /// The preferred constructor when migrating a call site that previously
    /// passed a bare `Some(secs)` timeout: it preserves the caller's intent and
    /// supplies the deadline the contract requires, so no call site has to
    /// re-derive the allowance.
    pub fn with_exec_timeout(command: impl Into<String>, timeout: Duration) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(timeout + STARTUP_ALLOWANCE),
            command: command.into(),
            timeout: Some(timeout),
        }
    }

    /// Build a request with no explicit execution timeout, letting the backend
    /// apply its configured default — the migration equivalent of passing
    /// `None`.
    ///
    /// The deadline is only a start-by bound here: with no execution timeout
    /// the runtime is whatever the backend is configured with, which the
    /// contract cannot see, so it is kept generous enough never to preempt a
    /// legitimate long-running command.
    pub fn untimed(command: impl Into<String>) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(UNTIMED_DEADLINE),
            command: command.into(),
            timeout: None,
        }
    }
}

/// Deadline applied when a request specifies no execution timeout.
pub const UNTIMED_DEADLINE: Duration = Duration::from_secs(300);

/// Outcome of a [`CommandRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_status: i32,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub duration: Duration,
}

/// Stream a local file to a remote path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadFileRequest {
    pub id: RequestId,
    pub deadline: Deadline,
    pub local: PathBuf,
    pub remote: String,
}

impl UploadFileRequest {
    pub fn untimed(local: impl Into<PathBuf>, remote: impl Into<String>) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(UNTIMED_DEADLINE),
            local: local.into(),
            remote: remote.into(),
        }
    }
}

/// Publish a text payload to a remote path atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTextRequest {
    pub id: RequestId,
    pub deadline: Deadline,
    pub text: String,
    pub remote: String,
}

impl UploadTextRequest {
    pub fn untimed(text: impl Into<String>, remote: impl Into<String>) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(UNTIMED_DEADLINE),
            text: text.into(),
            remote: remote.into(),
        }
    }
}

/// Fetch a remote file to a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFileRequest {
    pub id: RequestId,
    pub deadline: Deadline,
    pub remote: String,
    pub local: PathBuf,
}

impl DownloadFileRequest {
    pub fn untimed(remote: impl Into<String>, local: impl Into<PathBuf>) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(UNTIMED_DEADLINE),
            remote: remote.into(),
            local: local.into(),
        }
    }
}

/// Stream a remote directory into a local directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadDirRequest {
    pub id: RequestId,
    pub deadline: Deadline,
    pub remote: String,
    pub local: PathBuf,
}

impl DownloadDirRequest {
    pub fn untimed(remote: impl Into<String>, local: impl Into<PathBuf>) -> Self {
        Self {
            id: RequestId::new(),
            deadline: Deadline::from_now(UNTIMED_DEADLINE),
            remote: remote.into(),
            local: local.into(),
        }
    }
}

// ────────────────────────────────── errors ──────────────────────────────────

/// Structured failure at the transport boundary.
///
/// The variants carry enough information to decide retry behaviour without
/// inspecting message text. That is the whole point of the enum: the previous
/// code path classified failures by substring-matching stderr, which is exactly
/// the pattern this replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Configuration(String),
    DaemonUnavailable,
    ProtocolMismatch {
        expected: String,
        actual: String,
    },
    AuthenticationFailed(String),
    InteractionRequired,
    HostKeyUnknown {
        host: String,
        fingerprint: String,
    },
    HostKeyChanged {
        host: String,
    },
    HostKeyPolicyUnsupported(String),
    ProxyFailed(String),
    JumpFailed(String),
    ConnectionFailed(String),
    /// The deadline passed before the request acquired the right to run.
    /// Proves that no remote operation began.
    QueueTimeout {
        request: RequestId,
        after_secs: u64,
    },
    /// The remote operation started but did not finish in time.
    /// `remote_terminated` records whether termination was proven; when it is
    /// false the failure carries unknown-outcome semantics.
    ExecutionTimeout {
        request: RequestId,
        after_secs: u64,
        remote_terminated: bool,
    },
    /// The remote command ran to completion and exited non-zero.
    RemoteExit {
        status: i32,
        stderr: String,
    },
    /// The operation may have executed. Never retry automatically.
    OutcomeUnknown {
        request: RequestId,
        reason: String,
    },
    TransferInterrupted {
        request: RequestId,
        reason: String,
    },
    IntegrityMismatch {
        expected: String,
        actual: String,
    },
    LocalIo(String),
    RemoteIo(String),
    Cancelled {
        request: RequestId,
    },
    RestartRequired(String),
    UnsupportedOperation(&'static str),
    /// The selected backend was not compiled into this binary.
    UnsupportedBackend,
}

impl TransportError {
    /// Stable machine-readable code. Distinct from the display string so that
    /// callers never have to parse messages.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::ProtocolMismatch { .. } => "protocol_mismatch",
            Self::AuthenticationFailed(_) => "authentication_failed",
            Self::InteractionRequired => "interaction_required",
            Self::HostKeyUnknown { .. } => "host_key_unknown",
            Self::HostKeyChanged { .. } => "host_key_changed",
            Self::HostKeyPolicyUnsupported(_) => "host_key_policy_unsupported",
            Self::ProxyFailed(_) => "proxy_failed",
            Self::JumpFailed(_) => "jump_failed",
            Self::ConnectionFailed(_) => "connection_failed",
            Self::QueueTimeout { .. } => "queue_timeout",
            Self::ExecutionTimeout { .. } => "execution_timeout",
            Self::RemoteExit { .. } => "remote_exit",
            Self::OutcomeUnknown { .. } => "outcome_unknown",
            Self::TransferInterrupted { .. } => "transfer_interrupted",
            Self::IntegrityMismatch { .. } => "integrity_mismatch",
            Self::LocalIo(_) => "local_io",
            Self::RemoteIo(_) => "remote_io",
            Self::Cancelled { .. } => "cancelled",
            Self::RestartRequired(_) => "restart_required",
            Self::UnsupportedOperation(_) => "unsupported_operation",
            Self::UnsupportedBackend => "unsupported_backend",
        }
    }

    /// Whether a caller may resubmit this request automatically.
    ///
    /// The decisive distinction is whether the remote operation provably did not
    /// start. `QueueTimeout` proves it did not. `OutcomeUnknown` means it may
    /// have executed and is therefore never retryable, no matter what the
    /// message says.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::QueueTimeout { .. }
                | Self::ConnectionFailed(_)
                | Self::ExecutionTimeout {
                    remote_terminated: true,
                    ..
                }
        )
    }

    /// Process exit code, aligned with `VirtuosoError::exit_code` so that the
    /// public mapping preserves the CLI contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Configuration(_) | Self::UnsupportedBackend => exit_codes::USAGE_ERROR,
            Self::AuthenticationFailed(_) => exit_codes::USAGE_ERROR,
            _ => exit_codes::GENERAL_ERROR,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(m) => write!(f, "transport misconfigured: {m}"),
            Self::DaemonUnavailable => write!(f, "transport daemon is not running"),
            Self::ProtocolMismatch { expected, actual } => {
                write!(
                    f,
                    "transport protocol mismatch: expected {expected}, got {actual}"
                )
            }
            Self::AuthenticationFailed(m) => write!(f, "authentication failed: {m}"),
            Self::InteractionRequired => {
                write!(
                    f,
                    "interactive input required but this session is not interactive"
                )
            }
            Self::HostKeyUnknown { host, fingerprint } => {
                write!(f, "unknown host key for {host} ({fingerprint})")
            }
            Self::HostKeyChanged { host } => write!(f, "host key for {host} changed"),
            Self::HostKeyPolicyUnsupported(m) => write!(f, "unsupported host key policy: {m}"),
            Self::ProxyFailed(m) => write!(f, "proxy connection failed: {m}"),
            Self::JumpFailed(m) => write!(f, "jump host connection failed: {m}"),
            Self::ConnectionFailed(m) => write!(f, "connection failed: {m}"),
            Self::QueueTimeout { after_secs, .. } => {
                write!(f, "queued for {after_secs}s without starting")
            }
            Self::ExecutionTimeout {
                after_secs,
                remote_terminated,
                ..
            } => {
                if *remote_terminated {
                    write!(f, "timed out after {after_secs}s and was terminated")
                } else {
                    write!(f, "timed out after {after_secs}s; termination unproven")
                }
            }
            Self::RemoteExit { status, stderr } => {
                write!(f, "remote command exited with {status}: {stderr}")
            }
            Self::OutcomeUnknown { reason, .. } => {
                write!(f, "outcome unknown, the operation may have run: {reason}")
            }
            Self::TransferInterrupted { reason, .. } => write!(f, "transfer interrupted: {reason}"),
            Self::IntegrityMismatch { expected, actual } => {
                write!(
                    f,
                    "integrity check failed: expected {expected}, got {actual}"
                )
            }
            Self::LocalIo(m) => write!(f, "local io error: {m}"),
            Self::RemoteIo(m) => write!(f, "remote io error: {m}"),
            Self::Cancelled { .. } => write!(f, "cancelled"),
            Self::RestartRequired(m) => write!(f, "transport restart required: {m}"),
            Self::UnsupportedOperation(op) => write!(f, "operation not supported: {op}"),
            Self::UnsupportedBackend => {
                write!(
                    f,
                    "this build was compiled without the selected ssh backend"
                )
            }
        }
    }
}

impl std::error::Error for TransportError {}

impl From<TransportError> for VirtuosoError {
    fn from(e: TransportError) -> Self {
        match e {
            TransportError::Configuration(m) => VirtuosoError::Config(m),
            TransportError::AuthenticationFailed(m) => VirtuosoError::Auth(m),
            TransportError::ConnectionFailed(m) => VirtuosoError::Connection(m),
            TransportError::QueueTimeout { after_secs, .. } => VirtuosoError::Timeout(after_secs),
            TransportError::ExecutionTimeout { after_secs, .. } => {
                VirtuosoError::Timeout(after_secs)
            }
            TransportError::RemoteExit { stderr, .. } => VirtuosoError::Execution(stderr),
            TransportError::OutcomeUnknown { reason, .. } => VirtuosoError::Execution(reason),
            TransportError::LocalIo(m) | TransportError::RemoteIo(m) => VirtuosoError::Ssh(m),
            other => VirtuosoError::Ssh(other.to_string()),
        }
    }
}

// ────────────────────────────── the contract ────────────────────────────────

/// Transport semantics available to business modules.
///
/// `Send + Sync` is a hard requirement, not a convenience. Parallel Spectre work
/// shares one handle across scoped threads, and a bare trait object satisfies
/// neither bound. The bound is declared on the trait so that every
/// implementation inherits it and `Arc<dyn RemoteTransport>` is usable
/// everywhere.
///
/// A transport never replays an operation. Reconnection re-establishes the path;
/// it does not re-issue work. See the design's "SKILL request retry policy".
pub trait RemoteTransport: Send + Sync {
    /// Probe reachability. Returns `false` rather than an error when the host
    /// answered but the probe failed.
    fn test_connection(&self, deadline: Deadline) -> Result<bool, TransportError>;

    fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError>;

    fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError>;

    fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError>;

    fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError>;

    fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError>;

    /// Open a local listener that reaches a remote endpoint.
    ///
    /// Deliberately unsupported by default: the OpenSSH backend establishes its
    /// forward through a separate code path that owns process state, and folding
    /// it in here is a later increment. Implementations report the gap rather
    /// than panicking, so a caller can detect it structurally.
    fn start_local_forward(&self, _req: &ForwardRequest) -> Result<ForwardHandle, TransportError> {
        Err(TransportError::UnsupportedOperation("start_local_forward"))
    }

    fn stop_local_forward(&self, _id: &ForwardId) -> Result<(), TransportError> {
        Err(TransportError::UnsupportedOperation("stop_local_forward"))
    }

    fn health(&self) -> Result<TransportHealth, TransportError> {
        Err(TransportError::UnsupportedOperation("health"))
    }

    fn shutdown(&self) -> Result<(), TransportError> {
        Err(TransportError::UnsupportedOperation("shutdown"))
    }
}

/// Request to open a local forward. Defined with the contract so the signature
/// is stable before any implementation provides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRequest {
    pub id: RequestId,
    pub listen: String,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardHandle {
    pub id: ForwardId,
    pub local_port: u16,
}

/// Endpoint state, mirroring the lifecycle the design describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportHealth {
    Starting,
    Authenticating,
    Ready,
    Reconnecting,
    Degraded,
    PermanentFailure,
    Stopping,
    Stopped,
}

// ───────────────────────────── contract tests ───────────────────────────────

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared behavioural suite plus an in-memory transport.
    //!
    //! The suite is written against `&dyn RemoteTransport` so that the same
    //! assertions run against every backend. It only covers what is observable
    //! without a live host; heavier properties (atomic publication, path
    //! safety, transfer interruption) are added when there is an implementation
    //! that can violate them.

    use super::*;
    use std::sync::Mutex;

    /// An in-memory transport that records requests and never touches a network.
    pub struct FakeTransport {
        pub command_result: CommandResult,
        pub fail_with: Option<TransportError>,
        pub deadline_expired: bool,
        pub commands: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        pub fn ok() -> Self {
            Self {
                command_result: CommandResult {
                    exit_status: 0,
                    stdout: "ok".into(),
                    stderr: String::new(),
                    success: true,
                    duration: Duration::from_millis(1),
                },
                fail_with: None,
                deadline_expired: false,
                commands: Mutex::new(Vec::new()),
            }
        }
    }

    impl RemoteTransport for FakeTransport {
        fn test_connection(&self, _deadline: Deadline) -> Result<bool, TransportError> {
            Ok(true)
        }
        fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
            self.commands.lock().unwrap().push(req.command.clone());
            if self.deadline_expired || req.deadline.is_expired() {
                return Err(TransportError::QueueTimeout {
                    request: req.id.clone(),
                    after_secs: 0,
                });
            }
            match &self.fail_with {
                Some(e) => Err(e.clone()),
                None => Ok(self.command_result.clone()),
            }
        }
        fn upload_file(&self, _req: &UploadFileRequest) -> Result<(), TransportError> {
            Ok(())
        }
        fn upload_text(&self, _req: &UploadTextRequest) -> Result<(), TransportError> {
            Ok(())
        }
        fn download_file(&self, _req: &DownloadFileRequest) -> Result<(), TransportError> {
            Ok(())
        }
        fn download_dir(&self, _req: &DownloadDirRequest) -> Result<(), TransportError> {
            Ok(())
        }
    }

    /// Assertions every implementation must satisfy. Runs without a network.
    pub fn shared_contract_suite(t: &dyn RemoteTransport) {
        // An expired deadline fails before any remote work is attempted, and the
        // failure is classified as QueueTimeout — which is what makes it safe
        // to retry.
        let expired = Deadline(Instant::now() - Duration::from_secs(1));
        let req = CommandRequest::new("true", expired);
        match t.run_command(&req) {
            Err(TransportError::QueueTimeout { .. }) => {}
            Err(other) => panic!("expired deadline must report QueueTimeout, got {other:?}"),
            Ok(_) => panic!("expired deadline must not execute"),
        }

        // Forwarding is not yet provided by any shipping backend; the contract
        // requires it to be reported, not panicked on.
        assert!(matches!(
            t.health(),
            Err(TransportError::UnsupportedOperation(_))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{shared_contract_suite, FakeTransport};
    use super::*;
    use std::sync::Arc;

    /// The bound the design calls a hard requirement. If this stops compiling,
    /// the contract has regressed.
    #[test]
    fn contract_requires_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FakeTransport>();
    }

    #[test]
    fn trait_object_is_usable_behind_arc() {
        let t: Arc<dyn RemoteTransport> = Arc::new(FakeTransport::ok());
        let req = CommandRequest::new("true", Deadline::from_now(Duration::from_secs(5)));
        assert!(t.run_command(&req).is_ok());
    }

    #[test]
    fn fake_transport_passes_shared_suite() {
        shared_contract_suite(&FakeTransport::ok());
    }

    #[test]
    fn request_ids_are_distinct() {
        assert_ne!(RequestId::new(), RequestId::new());
    }

    #[test]
    fn with_exec_timeout_derives_deadline_and_keeps_timeout() {
        let req = CommandRequest::with_exec_timeout("true", Duration::from_secs(60));
        assert_eq!(req.timeout, Some(Duration::from_secs(60)));
        // Deadline must outlast the execution timeout by the startup
        // allowance, so a request is never doomed by its own timeout.
        let remaining = req.deadline.remaining();
        assert!(remaining > Duration::from_secs(60), "got {remaining:?}");
        assert!(remaining <= Duration::from_secs(60) + STARTUP_ALLOWANCE);
    }

    #[test]
    fn deadline_reports_expiry_and_remaining() {
        let past = Deadline(Instant::now() - Duration::from_secs(10));
        assert!(past.is_expired());
        assert_eq!(past.remaining(), Duration::ZERO);
        assert_eq!(past.remaining_secs(), 1, "seconds floor at 1, never 0");

        let future = Deadline::from_now(Duration::from_secs(30));
        assert!(!future.is_expired());
        assert!(future.remaining() > Duration::from_secs(25));
    }

    // ── error classification: the property that replaces message matching ──

    #[test]
    fn queue_timeout_is_retryable_because_nothing_started() {
        let e = TransportError::QueueTimeout {
            request: RequestId::new(),
            after_secs: 5,
        };
        assert!(e.retryable(), "proves no remote operation began");
    }

    #[test]
    fn outcome_unknown_is_never_retryable() {
        let e = TransportError::OutcomeUnknown {
            request: RequestId::new(),
            reason: "connection lost mid-command".into(),
        };
        assert!(!e.retryable(), "the operation may have executed");
    }

    #[test]
    fn execution_timeout_retryable_only_when_termination_proven() {
        let req = RequestId::new();
        let unproven = TransportError::ExecutionTimeout {
            request: req.clone(),
            after_secs: 5,
            remote_terminated: false,
        };
        let proven = TransportError::ExecutionTimeout {
            request: req,
            after_secs: 5,
            remote_terminated: true,
        };
        assert!(!unproven.retryable());
        assert!(proven.retryable());
    }

    #[test]
    fn retry_classification_ignores_message_text() {
        // Two OutcomeUnknown errors with contradictory messages must classify
        // identically — that is what "no message-string matching" means.
        let a = TransportError::OutcomeUnknown {
            request: RequestId::new(),
            reason: "connection reset".into(),
        };
        let b = TransportError::OutcomeUnknown {
            request: RequestId::new(),
            reason: "everything is fine, definitely retry".into(),
        };
        assert_eq!(a.retryable(), b.retryable());
        assert!(!a.retryable());
    }

    #[test]
    fn error_codes_are_stable_and_unique() {
        let variants: Vec<TransportError> = vec![
            TransportError::Configuration("x".into()),
            TransportError::DaemonUnavailable,
            TransportError::ProtocolMismatch {
                expected: "1".into(),
                actual: "2".into(),
            },
            TransportError::AuthenticationFailed("x".into()),
            TransportError::InteractionRequired,
            TransportError::HostKeyUnknown {
                host: "h".into(),
                fingerprint: "f".into(),
            },
            TransportError::HostKeyChanged { host: "h".into() },
            TransportError::HostKeyPolicyUnsupported("x".into()),
            TransportError::ProxyFailed("x".into()),
            TransportError::JumpFailed("x".into()),
            TransportError::ConnectionFailed("x".into()),
            TransportError::QueueTimeout {
                request: RequestId::new(),
                after_secs: 1,
            },
            TransportError::ExecutionTimeout {
                request: RequestId::new(),
                after_secs: 1,
                remote_terminated: false,
            },
            TransportError::RemoteExit {
                status: 1,
                stderr: "x".into(),
            },
            TransportError::OutcomeUnknown {
                request: RequestId::new(),
                reason: "x".into(),
            },
            TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: "x".into(),
            },
            TransportError::IntegrityMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            TransportError::LocalIo("x".into()),
            TransportError::RemoteIo("x".into()),
            TransportError::Cancelled {
                request: RequestId::new(),
            },
            TransportError::RestartRequired("x".into()),
            TransportError::UnsupportedOperation("x"),
            TransportError::UnsupportedBackend,
        ];
        let codes: Vec<&str> = variants.iter().map(|v| v.code()).collect();
        let unique: std::collections::HashSet<&str> = codes.iter().copied().collect();
        assert_eq!(
            codes.len(),
            unique.len(),
            "every variant needs its own code"
        );
        assert_eq!(codes.len(), 23, "matches the design's error model count");
    }

    #[test]
    fn virtuoso_error_mapping_preserves_exit_codes() {
        // Configuration-ish failures stay usage errors; everything else stays a
        // general error, matching VirtuosoError's existing classification.
        assert_eq!(
            TransportError::Configuration("x".into()).exit_code(),
            exit_codes::USAGE_ERROR
        );
        assert_eq!(
            TransportError::UnsupportedBackend.exit_code(),
            exit_codes::USAGE_ERROR
        );
        assert_eq!(
            TransportError::ConnectionFailed("x".into()).exit_code(),
            exit_codes::GENERAL_ERROR
        );

        let mapped: VirtuosoError = TransportError::ConnectionFailed("x".into()).into();
        assert_eq!(mapped.exit_code(), exit_codes::GENERAL_ERROR);
        let mapped: VirtuosoError = TransportError::Configuration("x".into()).into();
        assert_eq!(mapped.exit_code(), exit_codes::USAGE_ERROR);
    }
}

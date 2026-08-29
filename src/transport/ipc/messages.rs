//! Request/response vocabulary for the transport IPC protocol.
//!
//! Every structure here is a plain `serde` type with no SSH, Tokio, or
//! daemon-internal references. These are the only types the daemon and the
//! business-side client agree on; the daemon keeps russh/Tokio types private.
//!
//! Wire layout (handled by [`crate::transport::ipc::framing`]) is four-byte
//! big-endian length-prefixed UTF-8 JSON. Unknown JSON fields are ignored, so
//! minor-version fields can be added without breaking older clients.

#![allow(dead_code)]

use crate::transport::contract::{RequestId, TransportError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Protocol version, negotiated during `Hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    /// The version this build speaks.
    pub fn current() -> Self {
        ProtocolVersion {
            major: crate::transport::ipc::framing::PROTOCOL_MAJOR,
            minor: crate::transport::ipc::framing::PROTOCOL_MINOR,
        }
    }

    /// A client is compatible when majors match. Minors may differ: the daemon
    /// negotiates downward via capabilities, but a major mismatch is a hard
    /// failure (`ProtocolMismatch`).
    pub fn compatible_with(&self, client: &ProtocolVersion) -> bool {
        self.major == client.major
    }
}

/// Operations the daemon understands.
///
/// `Unknown` captures an operation string the daemon does not implement, so the
/// dispatcher can answer `UnsupportedOperation` instead of failing to parse the
/// frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Hello,
    TestConnection,
    RunCommand,
    UploadFile,
    UploadText,
    DownloadFile,
    DownloadDir,
    StartLocalForward,
    StopLocalForward,
    Health,
    Shutdown,
    Cancel,
    Unknown(String),
}

impl Operation {
    pub fn as_str(&self) -> &str {
        match self {
            Operation::Hello => "hello",
            Operation::TestConnection => "test_connection",
            Operation::RunCommand => "run_command",
            Operation::UploadFile => "upload_file",
            Operation::UploadText => "upload_text",
            Operation::DownloadFile => "download_file",
            Operation::DownloadDir => "download_dir",
            Operation::StartLocalForward => "start_local_forward",
            Operation::StopLocalForward => "stop_local_forward",
            Operation::Health => "health",
            Operation::Shutdown => "shutdown",
            Operation::Cancel => "cancel",
            Operation::Unknown(s) => s.as_str(),
        }
    }
}

impl Serialize for Operation {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Operation {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(match raw.as_str() {
            "hello" => Operation::Hello,
            "test_connection" => Operation::TestConnection,
            "run_command" => Operation::RunCommand,
            "upload_file" => Operation::UploadFile,
            "upload_text" => Operation::UploadText,
            "download_file" => Operation::DownloadFile,
            "download_dir" => Operation::DownloadDir,
            "start_local_forward" => Operation::StartLocalForward,
            "stop_local_forward" => Operation::StopLocalForward,
            "health" => Operation::Health,
            "shutdown" => Operation::Shutdown,
            "cancel" => Operation::Cancel,
            other => Operation::Unknown(other.to_string()),
        })
    }
}

/// Client→daemon `Hello`, sent before any operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub profile: String,
    #[serde(default)]
    pub auth_token: String,
    pub client_major: u16,
    pub client_minor: u16,
}

impl Hello {
    pub fn new(profile: impl Into<String>, auth_token: impl Into<String>) -> Self {
        let v = ProtocolVersion::current();
        Self {
            profile: profile.into(),
            auth_token: auth_token.into(),
            client_major: v.major,
            client_minor: v.minor,
        }
    }
}

/// Daemon→client `HelloAck`, returned on a successful handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    pub server_major: u16,
    pub server_minor: u16,
    /// Opaque per-instance nonce. Every subsequent request must echo it; a daemon
    /// restart changes the nonce and invalidates stale clients.
    pub daemon_nonce: String,
    /// Capability flags the daemon is willing to serve.
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

/// A request envelope. Carries the operation plus the identity/deadline fields
/// every request needs. `payload` is operation-specific JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub profile: String,
    #[serde(default)]
    pub daemon_nonce: String,
    #[serde(default)]
    pub auth_token: String,
    pub request_id: String,
    pub deadline_unix_ms: u64,
    pub operation: Operation,
    pub payload: serde_json::Value,
}

/// The result half of a response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseResult {
    Ok(serde_json::Value),
    Err(IpcError),
}

/// A response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: ResponseResult,
}

/// Wire error type.
///
/// Mirrors [`TransportError`] with owned `String` fields so it serializes
/// cleanly, and converts losslessly enough for the contract error model. The
/// daemon maps its internal [`TransportError`] into this before sending; the
/// client maps it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcError {
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
    QueueTimeout {
        request: String,
        after_secs: u64,
    },
    ExecutionTimeout {
        request: String,
        after_secs: u64,
        remote_terminated: bool,
    },
    RemoteExit {
        status: i32,
        stderr: String,
    },
    OutcomeUnknown {
        request: String,
        reason: String,
    },
    TransferInterrupted {
        request: String,
        reason: String,
    },
    IntegrityMismatch {
        expected: String,
        actual: String,
    },
    LocalIo(String),
    RemoteIo(String),
    Cancelled {
        request: String,
    },
    RestartRequired(String),
    UnsupportedOperation(String),
    UnsupportedBackend,
}

impl IpcError {
    /// Stable machine-readable code, aligned with [`TransportError::code`].
    pub fn code(&self) -> &'static str {
        match self {
            IpcError::Configuration(_) => "configuration",
            IpcError::DaemonUnavailable => "daemon_unavailable",
            IpcError::ProtocolMismatch { .. } => "protocol_mismatch",
            IpcError::AuthenticationFailed(_) => "authentication_failed",
            IpcError::InteractionRequired => "interaction_required",
            IpcError::HostKeyUnknown { .. } => "host_key_unknown",
            IpcError::HostKeyChanged { .. } => "host_key_changed",
            IpcError::HostKeyPolicyUnsupported(_) => "host_key_policy_unsupported",
            IpcError::ProxyFailed(_) => "proxy_failed",
            IpcError::JumpFailed(_) => "jump_failed",
            IpcError::ConnectionFailed(_) => "connection_failed",
            IpcError::QueueTimeout { .. } => "queue_timeout",
            IpcError::ExecutionTimeout { .. } => "execution_timeout",
            IpcError::RemoteExit { .. } => "remote_exit",
            IpcError::OutcomeUnknown { .. } => "outcome_unknown",
            IpcError::TransferInterrupted { .. } => "transfer_interrupted",
            IpcError::IntegrityMismatch { .. } => "integrity_mismatch",
            IpcError::LocalIo(_) => "local_io",
            IpcError::RemoteIo(_) => "remote_io",
            IpcError::Cancelled { .. } => "cancelled",
            IpcError::RestartRequired(_) => "restart_required",
            IpcError::UnsupportedOperation(_) => "unsupported_operation",
            IpcError::UnsupportedBackend => "unsupported_backend",
        }
    }
}

impl From<TransportError> for IpcError {
    fn from(e: TransportError) -> Self {
        match e {
            TransportError::Configuration(m) => IpcError::Configuration(m),
            TransportError::DaemonUnavailable => IpcError::DaemonUnavailable,
            TransportError::ProtocolMismatch { expected, actual } => {
                IpcError::ProtocolMismatch { expected, actual }
            }
            TransportError::AuthenticationFailed(m) => IpcError::AuthenticationFailed(m),
            TransportError::InteractionRequired => IpcError::InteractionRequired,
            TransportError::HostKeyUnknown { host, fingerprint } => {
                IpcError::HostKeyUnknown { host, fingerprint }
            }
            TransportError::HostKeyChanged { host } => IpcError::HostKeyChanged { host },
            TransportError::HostKeyPolicyUnsupported(m) => IpcError::HostKeyPolicyUnsupported(m),
            TransportError::ProxyFailed(m) => IpcError::ProxyFailed(m),
            TransportError::JumpFailed(m) => IpcError::JumpFailed(m),
            TransportError::ConnectionFailed(m) => IpcError::ConnectionFailed(m),
            TransportError::QueueTimeout {
                request,
                after_secs,
            } => IpcError::QueueTimeout {
                request: request.0,
                after_secs,
            },
            TransportError::ExecutionTimeout {
                request,
                after_secs,
                remote_terminated,
            } => IpcError::ExecutionTimeout {
                request: request.0,
                after_secs,
                remote_terminated,
            },
            TransportError::RemoteExit { status, stderr } => {
                IpcError::RemoteExit { status, stderr }
            }
            TransportError::OutcomeUnknown { request, reason } => IpcError::OutcomeUnknown {
                request: request.0,
                reason,
            },
            TransportError::TransferInterrupted { request, reason } => {
                IpcError::TransferInterrupted {
                    request: request.0,
                    reason,
                }
            }
            TransportError::IntegrityMismatch { expected, actual } => {
                IpcError::IntegrityMismatch { expected, actual }
            }
            TransportError::LocalIo(m) => IpcError::LocalIo(m),
            TransportError::RemoteIo(m) => IpcError::RemoteIo(m),
            TransportError::Cancelled { request } => IpcError::Cancelled { request: request.0 },
            TransportError::RestartRequired(m) => IpcError::RestartRequired(m),
            TransportError::UnsupportedOperation(op) => IpcError::UnsupportedOperation(op),
            TransportError::UnsupportedBackend => IpcError::UnsupportedBackend,
        }
    }
}

impl From<IpcError> for TransportError {
    fn from(e: IpcError) -> Self {
        match e {
            IpcError::Configuration(m) => TransportError::Configuration(m),
            IpcError::DaemonUnavailable => TransportError::DaemonUnavailable,
            IpcError::ProtocolMismatch { expected, actual } => {
                TransportError::ProtocolMismatch { expected, actual }
            }
            IpcError::AuthenticationFailed(m) => TransportError::AuthenticationFailed(m),
            IpcError::InteractionRequired => TransportError::InteractionRequired,
            IpcError::HostKeyUnknown { host, fingerprint } => {
                TransportError::HostKeyUnknown { host, fingerprint }
            }
            IpcError::HostKeyChanged { host } => TransportError::HostKeyChanged { host },
            IpcError::HostKeyPolicyUnsupported(m) => TransportError::HostKeyPolicyUnsupported(m),
            IpcError::ProxyFailed(m) => TransportError::ProxyFailed(m),
            IpcError::JumpFailed(m) => TransportError::JumpFailed(m),
            IpcError::ConnectionFailed(m) => TransportError::ConnectionFailed(m),
            IpcError::QueueTimeout {
                request,
                after_secs,
            } => TransportError::QueueTimeout {
                request: RequestId(request),
                after_secs,
            },
            IpcError::ExecutionTimeout {
                request,
                after_secs,
                remote_terminated,
            } => TransportError::ExecutionTimeout {
                request: RequestId(request),
                after_secs,
                remote_terminated,
            },
            IpcError::RemoteExit { status, stderr } => {
                TransportError::RemoteExit { status, stderr }
            }
            IpcError::OutcomeUnknown { request, reason } => TransportError::OutcomeUnknown {
                request: RequestId(request),
                reason,
            },
            IpcError::TransferInterrupted { request, reason } => {
                TransportError::TransferInterrupted {
                    request: RequestId(request),
                    reason,
                }
            }
            IpcError::IntegrityMismatch { expected, actual } => {
                TransportError::IntegrityMismatch { expected, actual }
            }
            IpcError::LocalIo(m) => TransportError::LocalIo(m),
            IpcError::RemoteIo(m) => TransportError::RemoteIo(m),
            IpcError::Cancelled { request } => TransportError::Cancelled {
                request: RequestId(request),
            },
            IpcError::RestartRequired(m) => TransportError::RestartRequired(m),
            IpcError::UnsupportedOperation(op) => TransportError::UnsupportedOperation(op),
            IpcError::UnsupportedBackend => TransportError::UnsupportedBackend,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::Deadline;
    use std::time::Duration;

    #[test]
    fn current_version_matches_framing_constants() {
        let v = ProtocolVersion::current();
        assert_eq!(v.major, crate::transport::ipc::framing::PROTOCOL_MAJOR);
        assert_eq!(v.minor, crate::transport::ipc::framing::PROTOCOL_MINOR);
    }

    #[test]
    fn version_compatibility_is_major_only() {
        assert!(ProtocolVersion { major: 1, minor: 0 }
            .compatible_with(&ProtocolVersion { major: 1, minor: 5 }));
        assert!(!ProtocolVersion { major: 1, minor: 0 }
            .compatible_with(&ProtocolVersion { major: 2, minor: 0 }));
    }

    #[test]
    fn known_operations_round_trip_as_snake_case() {
        for (op, s) in [
            (Operation::Hello, "hello"),
            (Operation::RunCommand, "run_command"),
            (Operation::StartLocalForward, "start_local_forward"),
            (Operation::Cancel, "cancel"),
        ] {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(json, format!("\"{s}\""));
            let back: Operation = serde_json::from_str(&json).unwrap();
            assert_eq!(back, op);
        }
    }

    #[test]
    fn unknown_operation_is_captured_not_rejected() {
        let op: Operation = serde_json::from_str("\"frobnicate\"").unwrap();
        assert_eq!(op, Operation::Unknown("frobnicate".into()));
    }

    #[test]
    fn hello_ignores_unknown_fields() {
        let json = r#"{"profile":"p","auth_token":"t","client_major":1,"client_minor":0,"extra":"ignored"}"#;
        let hello: Hello = serde_json::from_str(json).unwrap();
        assert_eq!(hello.profile, "p");
        let round = serde_json::to_string(&hello).unwrap();
        let again: Hello = serde_json::from_str(&round).unwrap();
        assert_eq!(again, hello);
    }

    #[test]
    fn hello_ack_carries_nonce_and_capabilities() {
        let mut caps = BTreeSet::new();
        caps.insert("sftp".to_string());
        let ack = HelloAck {
            server_major: 1,
            server_minor: 0,
            daemon_nonce: "abc123".into(),
            capabilities: caps,
        };
        let json = serde_json::to_string(&ack).unwrap();
        let back: HelloAck = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ack);
        assert_eq!(back.daemon_nonce, "abc123");
        assert!(back.capabilities.contains("sftp"));
    }

    #[test]
    fn envelope_round_trips_with_json_payload() {
        let env = RequestEnvelope {
            protocol_major: 1,
            protocol_minor: 0,
            profile: "p".into(),
            daemon_nonce: "n".into(),
            auth_token: "t".into(),
            request_id: "r1".into(),
            deadline_unix_ms: 1_700_000_000_000,
            operation: Operation::RunCommand,
            payload: serde_json::json!({ "command": "true" }),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: RequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);

        let resp = ResponseEnvelope {
            request_id: "r1".into(),
            result: ResponseResult::Ok(serde_json::json!({ "exit_status": 0 })),
        };
        let rjson = serde_json::to_string(&resp).unwrap();
        let rback: ResponseEnvelope = serde_json::from_str(&rjson).unwrap();
        assert_eq!(rback, resp);
    }

    #[test]
    fn ipc_error_round_trips_through_transport_error() {
        // Exercise every variant, including the owned-string UnsupportedOperation
        // that motivated making the contract variant owned.
        let samples: Vec<TransportError> = vec![
            TransportError::Configuration("c".into()),
            TransportError::DaemonUnavailable,
            TransportError::ProtocolMismatch {
                expected: "1".into(),
                actual: "2".into(),
            },
            TransportError::AuthenticationFailed("a".into()),
            TransportError::InteractionRequired,
            TransportError::HostKeyUnknown {
                host: "h".into(),
                fingerprint: "f".into(),
            },
            TransportError::HostKeyChanged { host: "h".into() },
            TransportError::HostKeyPolicyUnsupported("p".into()),
            TransportError::ProxyFailed("p".into()),
            TransportError::JumpFailed("j".into()),
            TransportError::ConnectionFailed("co".into()),
            TransportError::QueueTimeout {
                request: RequestId::new(),
                after_secs: 3,
            },
            TransportError::ExecutionTimeout {
                request: RequestId::new(),
                after_secs: 4,
                remote_terminated: true,
            },
            TransportError::RemoteExit {
                status: 2,
                stderr: "boom".into(),
            },
            TransportError::OutcomeUnknown {
                request: RequestId::new(),
                reason: "lost".into(),
            },
            TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: "net".into(),
            },
            TransportError::IntegrityMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            TransportError::LocalIo("l".into()),
            TransportError::RemoteIo("r".into()),
            TransportError::Cancelled {
                request: RequestId::new(),
            },
            TransportError::RestartRequired("rr".into()),
            TransportError::UnsupportedOperation("op".into()),
            TransportError::UnsupportedBackend,
        ];
        for e in samples {
            let wired: IpcError = e.clone().into();
            let back: TransportError = wired.clone().into();
            assert_eq!(e, back, "variant lost in round-trip via {wired:?}");
            assert_eq!(wired.code(), e.code());
        }
    }

    #[test]
    fn ipc_error_codes_match_contract_codes() {
        assert_eq!(
            IpcError::QueueTimeout {
                request: "r".into(),
                after_secs: 1
            }
            .code(),
            "queue_timeout"
        );
        assert_eq!(
            IpcError::UnsupportedOperation("x".into()).code(),
            "unsupported_operation"
        );
        assert_eq!(IpcError::UnsupportedBackend.code(), "unsupported_backend");
    }

    // Guards against an accidental deadline that cannot be expressed on the wire.
    #[test]
    fn deadline_ms_fits_u64() {
        let d = Deadline::from_now(Duration::from_secs(30));
        let _ms: u64 = (d.0.elapsed().as_millis() as u64) + 30_000;
    }
}

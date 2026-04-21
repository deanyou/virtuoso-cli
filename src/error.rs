use crate::exit_codes;
use crate::output::CliError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum VirtuosoError {
    #[error("connection failed: {0}")]
    Connection(String),

    #[error("execution failed: {0}")]
    Execution(String),

    #[error("ssh error: {0}")]
    Ssh(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("timeout after {0}s")]
    Timeout(u64),

    #[allow(dead_code)]
    #[error("daemon not ready: {0}")]
    DaemonNotReady(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),
}

impl VirtuosoError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => exit_codes::USAGE_ERROR,
            Self::NotFound(_) => exit_codes::NOT_FOUND,
            Self::Conflict(_) => exit_codes::CONFLICT,
            Self::Connection(_) | Self::Ssh(_) | Self::Timeout(_) | Self::DaemonNotReady(_) => {
                exit_codes::GENERAL_ERROR
            }
            Self::Execution(_) | Self::Io(_) | Self::Json(_) => exit_codes::GENERAL_ERROR,
        }
    }

    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Connection(_) => "connection_failed",
            Self::Execution(_) => "execution_failed",
            Self::Ssh(_) => "ssh_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::Timeout(_) => "timeout",
            Self::DaemonNotReady(_) => "daemon_not_ready",
            Self::Config(_) => "config_error",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Connection(_) | Self::Timeout(_) | Self::DaemonNotReady(_)
        )
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            Self::Config(msg) if msg.contains("VB_REMOTE_HOST") => {
                Some("Run: virtuoso init".into())
            }
            Self::DaemonNotReady(_) | Self::Connection(_) => {
                Some("Run: virtuoso tunnel start".into())
            }
            Self::Timeout(secs) => Some(format!("Retry with --timeout {}", secs * 2)),
            Self::Ssh(msg) if msg.contains("authentication") => {
                Some("Check SSH keys: ssh-add -l".into())
            }
            _ => None,
        }
    }

    pub fn to_cli_error(&self) -> CliError {
        CliError {
            error: self.error_type().to_string(),
            message: self.to_string(),
            suggestion: self.suggestion(),
            retryable: self.retryable(),
        }
    }
}

pub type Result<T> = std::result::Result<T, VirtuosoError>;

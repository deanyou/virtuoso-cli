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

    /// Timeout with an attached context string (e.g. `remote_dir`).
    /// Display must include the timeout seconds AND the context so that
    /// operators can correlate the timeout with the operation that produced it.
    /// Exit-code / error_type / retryable / suggestion semantics match `Timeout`
    /// exactly so a downstream classification layer that already handles
    /// `Timeout` does not need to learn a second variant.
    #[error("timeout after {0}s; context: {1}")]
    TimeoutWithContext(u64, String),

    #[error("config error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("auth error: {0}")]
    Auth(String),
}

impl VirtuosoError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) | Self::Auth(_) => exit_codes::USAGE_ERROR,
            Self::NotFound(_) => exit_codes::NOT_FOUND,
            Self::Conflict(_) => exit_codes::CONFLICT,
            Self::Connection(_)
            | Self::Ssh(_)
            | Self::Timeout(_)
            | Self::TimeoutWithContext(_, _) => exit_codes::GENERAL_ERROR,
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
            Self::Timeout(_) | Self::TimeoutWithContext(_, _) => "timeout",
            Self::Config(_) => "config_error",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Auth(_) => "auth_error",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Connection(_) | Self::Timeout(_) | Self::TimeoutWithContext(_, _)
        )
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            Self::Config(msg) if msg.contains("VB_REMOTE_HOST") => {
                Some("Run: virtuoso init".into())
            }
            Self::Connection(_) => Some("Run: virtuoso tunnel start".into()),
            Self::Timeout(secs) => Some(format!("Retry with --timeout {}", secs * 2)),
            Self::TimeoutWithContext(secs, _) => Some(format!("Retry with --timeout {}", secs * 2)),
            Self::Ssh(msg) if msg.contains("authentication") => {
                Some("Check SSH keys: ssh-add -l".into())
            }
            Self::Execution(msg) if msg.ends_with(": nil") || msg.contains("unbound") => {
                Some("SKILL returned nil — check if a cellview/session is open".into())
            }
            Self::Execution(msg)
                if msg.contains("maeGetSetup") || msg.contains("maeGetSession") =>
            {
                Some("No ADE session active — open Maestro or run a simulation first".into())
            }
            Self::Execution(msg) if msg.contains("ddGetObj") || msg.contains("dbOpen") => {
                Some("Cellview not found — check lib/cell/view names".into())
            }
            Self::Execution(msg) if msg.contains("strmin") || msg.contains("ihdl") => {
                Some("Import failed — check GDS/Verilog paths and PDK libraries".into())
            }
            Self::NotFound(_) => Some("Use 'vcli session list' to see active sessions".into()),
            _ => None,
        }
    }

    /// Returns detailed diagnostic info for debugging SKILL failures.
    /// Includes context like session state, cellview status, etc.
    pub fn diagnostic_context(&self) -> Option<String> {
        match self {
            Self::Execution(msg) => {
                let mut hints = Vec::new();

                // SKILL nil detection
                if msg.ends_with(": nil") || msg.contains(" returns nil") {
                    hints.push("SKILL returned nil — variable or object not found");
                }

                // Unbound variable
                if msg.contains("unbound") {
                    hints.push("Unbound variable — check if object exists before accessing");
                }

                // mae* function failures (Maestro/ADE related)
                if msg.contains("maeGetSetup") || msg.contains("maeGetSession") {
                    hints.push("Run 'vcli maestro list' to check active sessions");
                }

                // db* function failures (database related)
                if msg.contains("ddGetObj") || msg.contains("dbOpen") {
                    hints.push("Use 'vcli window list' to check open cellviews");
                }

                // Connection/daemon issues
                if msg.contains("daemon") || msg.contains("port") {
                    hints.push("Restart Virtuoso daemon: reload the setup script in CIW");
                }

                // Timeout
                if msg.contains("timeout") {
                    hints.push("Increase timeout: --timeout 120 or higher");
                }

                if hints.is_empty() {
                    None
                } else {
                    Some(format!("Diagnostic hints:\n  • {}", hints.join("\n  • ")))
                }
            }
            _ => None,
        }
    }

    pub fn to_cli_error(&self) -> CliError {
        CliError {
            error: self.error_type().to_string(),
            message: self.to_string(),
            suggestion: self.suggestion(),
            diagnostic: self.diagnostic_context(),
            retryable: self.retryable(),
        }
    }
}

pub type Result<T> = std::result::Result<T, VirtuosoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_with_context_display_includes_seconds_and_context() {
        let e = VirtuosoError::TimeoutWithContext(42, "downloading /tmp/foo".into());
        let s = e.to_string();
        assert!(
            s.contains("42"),
            "display must include timeout seconds: {s}"
        );
        assert!(
            s.contains("downloading /tmp/foo"),
            "display must include context: {s}"
        );
    }

    #[test]
    fn timeout_with_context_exit_code_matches_timeout() {
        let plain = VirtuosoError::Timeout(42);
        let ctx = VirtuosoError::TimeoutWithContext(42, "any context".into());
        assert_eq!(
            ctx.exit_code(),
            plain.exit_code(),
            "TimeoutWithContext must share Timeout's exit code"
        );
    }

    #[test]
    fn timeout_with_context_error_type_is_timeout() {
        let ctx = VirtuosoError::TimeoutWithContext(7, "ctx".into());
        assert_eq!(
            ctx.error_type(),
            "timeout",
            "TimeoutWithContext must classify as 'timeout' so downstream parsers match Timeout"
        );
    }

    #[test]
    fn timeout_with_context_is_retryable() {
        let ctx = VirtuosoError::TimeoutWithContext(7, "ctx".into());
        assert!(
            ctx.retryable(),
            "TimeoutWithContext must be retryable exactly like Timeout"
        );
    }

    #[test]
    fn timeout_with_context_suggestion_uses_seconds() {
        let ctx = VirtuosoError::TimeoutWithContext(7, "ctx".into());
        let suggestion = ctx.suggestion();
        assert!(
            suggestion.is_some(),
            "TimeoutWithContext must provide a suggestion like Timeout"
        );
        let s = suggestion.unwrap();
        assert!(
            s.contains("14"),
            "suggestion must suggest doubling the timeout (7 * 2 = 14): {s}"
        );
    }

    #[test]
    fn timeout_with_context_classification_matches_timeout() {
        // A downstream classifier that branches on (exit_code, error_type,
        // retryable) for `Timeout` must produce the same answer for
        // `TimeoutWithContext`. This guards against accidental drift.
        let plain = VirtuosoError::Timeout(3);
        let ctx = VirtuosoError::TimeoutWithContext(3, "x".into());
        assert_eq!(plain.exit_code(), ctx.exit_code());
        assert_eq!(plain.error_type(), ctx.error_type());
        assert_eq!(plain.retryable(), ctx.retryable());
    }
}

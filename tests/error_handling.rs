//! Integration tests for error handling and diagnostics.
//!
//! Tests the error types and diagnostic context provided by virtuoso-cli.

use std::collections::HashMap;
use virtuoso_cli::error::VirtuosoError;
use virtuoso_cli::models::{ExecutionStatus, VirtuosoResult};

/// Test that VirtuosoError variants have correct exit codes.
#[test]
fn test_error_exit_codes() {
    // Execution errors should map to exit code 1 (GENERAL_ERROR)
    let err = VirtuosoError::Execution("test failed".to_string());
    assert_eq!(err.exit_code(), 1);
    assert!(err.to_string().contains("test failed"));

    // NotFound errors (exit code 3)
    let err = VirtuosoError::NotFound("CellView".to_string());
    assert_eq!(err.exit_code(), 3);
    assert!(err.to_string().contains("CellView"));

    // Config errors (exit code 2)
    let err = VirtuosoError::Config("invalid port".to_string());
    assert_eq!(err.exit_code(), 2);
}

/// Test that execution errors include skill context.
#[test]
fn test_execution_error_context() {
    let err = VirtuosoError::Execution("dbOpenCellViewByType: nil returned".to_string());

    let msg = err.to_string();
    assert!(msg.contains("dbOpenCellViewByType"));
    assert!(msg.contains("nil returned"));
}

/// Test VirtuosoError Debug format.
#[test]
fn test_error_debug_format() {
    let err = VirtuosoError::Connection("refused".to_string());
    let debug = format!("{:?}", err);

    assert!(debug.contains("Connection"));
    assert!(debug.contains("refused"));
}

/// Test VirtuosoError Display format.
#[test]
fn test_error_display_format() {
    let err = VirtuosoError::Timeout(30);
    let display = format!("{}", err);

    assert!(display.contains("30"));
    assert!(display.contains("timeout"));
}

/// Test that errors can be chained from std::io::Error.
#[test]
fn test_error_from_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test file");
    let err = VirtuosoError::from(io_err);

    assert!(matches!(err, VirtuosoError::Io(_)));
}

/// Test VirtuosoResult::ok() vs skill_ok().
#[test]
fn test_virtuoso_result_types() {
    // Create a mock result where SKILL returned nil (but transport succeeded)
    let result = VirtuosoResult {
        status: ExecutionStatus::Success,
        output: "nil".to_string(),
        errors: vec!["nil returned".to_string()],
        warnings: Vec::new(),
        execution_time: None,
        metadata: HashMap::new(),
    };

    // Transport layer succeeded
    assert!(result.ok());
    // But SKILL returned nil
    assert!(!result.skill_ok());
}

/// Test VirtuosoResult with successful execution.
#[test]
fn test_virtuoso_result_success() {
    let result = VirtuosoResult {
        status: ExecutionStatus::Success,
        output: "\"cellView\"".to_string(),
        errors: Vec::new(),
        warnings: Vec::new(),
        execution_time: Some(0.5),
        metadata: HashMap::new(),
    };

    assert!(result.ok());
    assert!(result.skill_ok());
    assert_eq!(result.output_unquoted(), "cellView");
}

/// Test VirtuosoResult with nil return value.
#[test]
fn test_virtuoso_result_nil() {
    let result = VirtuosoResult {
        status: ExecutionStatus::Success,
        output: "nil".to_string(),
        errors: vec!["nil".to_string()],
        warnings: Vec::new(),
        execution_time: None,
        metadata: HashMap::new(),
    };

    assert!(result.ok());
    assert!(!result.skill_ok());
}

/// Test VirtuosoResult with transport failure.
#[test]
fn test_virtuoso_result_transport_failure() {
    let result = VirtuosoResult {
        status: ExecutionStatus::Error,
        output: String::new(),
        errors: vec!["connection closed".to_string()],
        warnings: Vec::new(),
        execution_time: None,
        metadata: HashMap::new(),
    };

    assert!(!result.ok());
    assert!(!result.skill_ok());
}

/// Test VirtuosoResult::ok_or_exec() propagates errors.
#[test]
fn test_virtuoso_result_ok_or_exec() {
    let result = VirtuosoResult {
        status: ExecutionStatus::Success,
        output: "nil".to_string(),
        errors: vec!["cell not found".to_string()],
        warnings: Vec::new(),
        execution_time: None,
        metadata: HashMap::new(),
    };

    let exec_result = result.ok_or_exec("dbOpenCellViewByType");
    assert!(exec_result.is_err());
    let err = exec_result.unwrap_err();
    assert!(err.to_string().contains("dbOpenCellViewByType"));
    assert!(err.to_string().contains("nil"));
}

/// Test VirtuosoResult::success() factory.
#[test]
fn test_virtuoso_result_success_factory() {
    let result = VirtuosoResult::success("test output");

    assert!(result.ok());
    assert!(result.skill_ok());
    assert_eq!(result.output, "test output");
}

/// Test VirtuosoResult::error() factory.
#[test]
fn test_virtuoso_result_error_factory() {
    let result = VirtuosoResult::error(vec!["error 1".to_string(), "error 2".to_string()]);

    assert!(!result.ok());
    assert!(!result.skill_ok());
    assert_eq!(result.errors.len(), 2);
}

/// Test error_type method for VirtuosoError uses snake_case.
#[test]
fn test_error_type_method() {
    let err = VirtuosoError::Connection("test".to_string());
    assert_eq!(err.error_type(), "connection_failed");

    let err = VirtuosoError::Execution("test".to_string());
    assert_eq!(err.error_type(), "execution_failed");

    let err = VirtuosoError::NotFound("test".to_string());
    assert_eq!(err.error_type(), "not_found");

    let err = VirtuosoError::Config("test".to_string());
    assert_eq!(err.error_type(), "config_error");
}

/// Test VirtuosoError suggestion method.
#[test]
fn test_error_suggestion() {
    let err = VirtuosoError::Connection("refused".to_string());
    let suggestion = err.suggestion();

    // Connection errors should have suggestions
    assert!(suggestion.is_some());
}

/// Test timeout error format.
#[test]
fn test_timeout_error() {
    let err = VirtuosoError::Timeout(120);

    let display = format!("{}", err);
    assert!(display.contains("120"));
    assert!(display.contains("timeout"));
    assert_eq!(err.exit_code(), 1); // GENERAL_ERROR
}

/// Test conflict error exit code (5).
#[test]
fn test_conflict_error() {
    let err = VirtuosoError::Conflict("cell already exists".to_string());

    assert!(err.to_string().contains("cell already exists"));
    assert_eq!(err.exit_code(), 5); // CONFLICT
}

/// Test auth error format.
#[test]
fn test_auth_error() {
    let err = VirtuosoError::Auth("token expired".to_string());

    assert!(err.to_string().contains("token expired"));
    assert_eq!(err.exit_code(), 2); // USAGE_ERROR
}

/// Test retryable() method.
#[test]
fn test_error_retryable() {
    let conn_err = VirtuosoError::Connection("refused".to_string());
    assert!(conn_err.retryable());

    let timeout_err = VirtuosoError::Timeout(30);
    assert!(timeout_err.retryable());

    let exec_err = VirtuosoError::Execution("failed".to_string());
    assert!(!exec_err.retryable());
}

//! Integration tests for SKILL string escaping.
//!
//! Tests the escape_skill_string function used to safely embed user input
//! into SKILL expressions sent to Virtuoso.

use virtuoso_cli::client::bridge::escape_skill_string;

/// Test basic strings pass through unchanged.
#[test]
fn test_escape_basic_string() {
    assert_eq!(escape_skill_string("myCell"), "myCell");
    assert_eq!(escape_skill_string("Simple123"), "Simple123");
    assert_eq!(escape_skill_string("with_underscore"), "with_underscore");
}

/// Test double quotes are escaped.
#[test]
fn test_escape_double_quotes() {
    assert_eq!(escape_skill_string(r#"say "hello""#), r#"say \"hello\""#);
}

/// Test backslash is escaped.
#[test]
fn test_escape_backslash() {
    assert_eq!(escape_skill_string(r#"path\to\file"#), r#"path\\to\\file"#);
    assert_eq!(escape_skill_string(r#"a\b"#), r#"a\\b"#);
}

/// Test complex strings with multiple special chars.
#[test]
fn test_escape_complex() {
    // String with quotes and backslashes
    let input = r#"my"Lib\tool""#;
    let escaped = escape_skill_string(input);
    // Backslash first, then quote
    assert!(escaped.contains(r#"\\"#));
    assert!(escaped.contains(r#"\""#));
}

/// Test empty string.
#[test]
fn test_escape_empty() {
    assert_eq!(escape_skill_string(""), "");
}

/// Test whitespace is preserved.
#[test]
fn test_escape_whitespace() {
    assert_eq!(
        escape_skill_string("cell name with spaces"),
        "cell name with spaces"
    );
    assert_eq!(
        escape_skill_string("  leading trailing  "),
        "  leading trailing  "
    );
}

/// Test unicode characters (should pass through in Rust).
#[test]
fn test_escape_unicode() {
    // Unicode letters should be preserved
    assert_eq!(escape_skill_string("cell_α"), "cell_α");
    assert_eq!(escape_skill_string("模块"), "模块");
}

/// Test parentheses are preserved (not escaped by our function).
#[test]
fn test_escape_parentheses_preserved() {
    // Note: escape_skill_string only escapes " and \
    // Parentheses are not escaped by this function
    assert_eq!(escape_skill_string("(group)"), "(group)");
    assert_eq!(escape_skill_string("a(b)c"), "a(b)c");
}

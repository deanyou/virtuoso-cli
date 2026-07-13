use crate::error::{Result, VirtuosoError};
use crate::models::VirtuosoResult;

pub(crate) fn escape_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub(crate) fn string_literal(value: &str) -> String {
    format!("\"{}\"", escape_string(value))
}

pub(crate) fn require_transport<'a>(
    result: &'a VirtuosoResult,
    action: &str,
) -> Result<&'a str> {
    if result.ok() {
        return Ok(result.output.trim());
    }

    let detail = if result.errors.is_empty() {
        "transport failed".to_string()
    } else {
        result.errors.join("; ")
    };
    Err(VirtuosoError::Execution(format!("{action}: {detail}")))
}

pub(crate) fn require_non_nil<'a>(
    result: &'a VirtuosoResult,
    action: &str,
) -> Result<&'a str> {
    let output = require_transport(result, action)?;
    if output == "nil" {
        return Err(VirtuosoError::Execution(format!(
            "{action}: SKILL returned nil"
        )));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literal_escapes_skill_control_characters() {
        assert_eq!(
            string_literal("a\\b\"c\nd\re"),
            "\"a\\\\b\\\"c\\nd\\re\""
        );
    }

    #[test]
    fn string_literal_handles_empty_string() {
        assert_eq!(string_literal(""), "\"\"");
    }

    #[test]
    fn require_transport_accepts_data_nil() {
        let result = VirtuosoResult::success("nil");
        assert_eq!(require_transport(&result, "measure").unwrap(), "nil");
    }

    #[test]
    fn require_non_nil_rejects_bare_nil() {
        let result = VirtuosoResult::success("  nil\n");
        let error = require_non_nil(&result, "open design").unwrap_err();
        assert!(error.to_string().contains("open design"));
    }

    #[test]
    fn require_non_nil_accepts_string_nil() {
        let result = VirtuosoResult::success("\"nil\"");
        assert_eq!(
            require_non_nil(&result, "read value").unwrap(),
            "\"nil\""
        );
    }

    #[test]
    fn require_transport_maps_result_error() {
        let result = VirtuosoResult::error(vec!["daemon rejected request".into()]);
        let error = require_transport(&result, "run analysis").unwrap_err();
        assert!(error.to_string().contains("daemon rejected request"));
    }
}

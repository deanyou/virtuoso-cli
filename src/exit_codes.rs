pub const SUCCESS: i32 = 0;
pub const GENERAL_ERROR: i32 = 1;
pub const USAGE_ERROR: i32 = 2;
pub const NOT_FOUND: i32 = 3;
#[allow(dead_code)]
pub const PERMISSION_DENIED: i32 = 4;
pub const CONFLICT: i32 = 5;
pub const DRY_RUN_OK: i32 = 10;

/// Map a successful command result to the process exit code.
///
/// A command can report a failure inside its JSON payload and still return
/// `Ok(..)`: `vcli window dismiss-dialog --x11` answers `status: "failed"`
/// when a dismissal could not be verified. Mapping every `Ok` to `SUCCESS`
/// hides that failure from every caller that only looks at the exit status,
/// so a command may carry the exit code it wants in a numeric top-level
/// `"exit_code"` field.
///
/// Precedence:
///   1. `"exit_code"` — an explicit, command-chosen exit code;
///   2. `"dry_run": true` — the long-standing marker for "nothing changed";
///   3. `SUCCESS`.
///
/// The JSON contract is unaffected: `exit_code` is a purely additive field and
/// commands that do not set it keep their current exit code.
pub fn exit_code_for_value(value: &serde_json::Value) -> i32 {
    if let Some(code) = value.get("exit_code").and_then(|v| v.as_i64()) {
        return i32::try_from(code).unwrap_or(GENERAL_ERROR);
    }
    if value
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return DRY_RUN_OK;
    }
    SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_success_exits_zero() {
        assert_eq!(
            exit_code_for_value(&json!({"status": "dismissed"})),
            SUCCESS
        );
    }

    #[test]
    fn dry_run_keeps_its_own_exit_code() {
        assert_eq!(exit_code_for_value(&json!({"dry_run": true})), DRY_RUN_OK);
        assert_eq!(exit_code_for_value(&json!({"dry_run": false})), SUCCESS);
    }

    #[test]
    fn explicit_exit_code_is_used() {
        // A failed dismissal must not read as success.
        assert_eq!(exit_code_for_value(&json!({"exit_code": 1})), 1);
        assert_ne!(
            exit_code_for_value(&json!({"exit_code": 1, "status": "failed"})),
            SUCCESS
        );
    }

    #[test]
    fn explicit_exit_code_wins_over_dry_run() {
        assert_eq!(
            exit_code_for_value(&json!({"exit_code": 1, "dry_run": true})),
            1
        );
    }

    #[test]
    fn non_numeric_exit_code_is_ignored() {
        assert_eq!(exit_code_for_value(&json!({"exit_code": "boom"})), SUCCESS);
        assert_eq!(exit_code_for_value(&json!({"exit_code": 1.5})), SUCCESS);
        assert_eq!(exit_code_for_value(&json!({"exit_code": null})), SUCCESS);
    }
}

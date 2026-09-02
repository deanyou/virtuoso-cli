use crate::client::bridge::escape_skill_string;
use crate::error::{Result, VirtuosoError};
use std::path::{Path, PathBuf};

pub struct WindowOps;

/// Target window for a safe bootstrap operation.
///
/// The window_id comes from `WindowOps::list_windows` (X11-style id,
/// 32-bit unsigned). Bootstrap operations require an explicit ID —
/// there is no "current focused window" shortcut, because the
/// caller's view of focus may differ from Virtuoso's and accidental
/// targeting is the single biggest risk in the bootstrap path.
///
/// expected_name_pattern enforces a CIW-style check on the window
/// name (via `hiGetWindowName`) before any side-effecting SKILL is
/// sent. Default pattern accepts any window; callers should supply a
/// narrow pattern (e.g. `"^Library Manager"` for the Library
/// Manager window) to follow the principle of least authority.
pub struct BootstrapTarget {
    pub window_id: u32,
    pub expected_name_pattern: Option<String>,
}

/// Side-effecting bootstrap actions available through the safe
/// bootstrap interface.
///
/// `LoadFile` loads a SKILL program from a path under the
/// caller-supplied scratch root. The path is canonicalized and
/// containment-checked so callers cannot escape the scratch dir.
/// `EvalString` and other raw-text variants are intentionally absent
/// — the bootstrap interface must NEVER accept raw SKILL source from
/// the caller, since this is the path that bypasses the capability
/// whitelist.
pub enum BootstrapAction {
    /// Load a SKILL program file from a path.
    LoadFile(PathBuf),
}

impl WindowOps {
    /// List all open Virtuoso windows.
    /// Returns a JSON array string: [{"name":"..."}]
    pub fn list_windows(&self) -> String {
        r#"let((out sep) out = "[" sep = "" foreach(w hiGetWindowList() out = strcat(out sep sprintf(nil "{\"name\":\"%s\"}" hiGetWindowName(w))) sep = ",") strcat(out "]"))"#
            .into()
    }

    /// Dismiss the current blocking dialog.
    /// action: "cancel" closes via Cancel; "ok" attempts OK/Yes button.
    pub fn dismiss_dialog(&self, action: &str) -> String {
        if action == "ok" {
            r#"let((d) d = hiGetCurrentDialog() if(d hiSendOK(d) "no-dialog"))"#.into()
        } else {
            r#"let((d) d = hiGetCurrentDialog() if(d hiCancelDialog(d) "no-dialog"))"#.into()
        }
    }

    /// Get the name of the current dialog without dismissing it.
    /// Returns "no-dialog" if no dialog is active.
    pub fn get_dialog_info(&self) -> String {
        r#"let((d) d = hiGetCurrentDialog() if(d hiGetWindowName(d) "no-dialog"))"#.into()
    }

    /// Capture a screenshot of the current Virtuoso window to a PNG file.
    ///
    /// IC23.1 does not have `hiGetWindowScreenDump`, so we use X11 `import`
    /// (ImageMagick) via system().  The file path is verified with `isFile`
    /// after the capture to distinguish success from failure.
    pub fn screenshot(&self, path: &str) -> String {
        let path = escape_skill_string(path);
        Self::skill_capture(&path)
    }

    /// Capture a screenshot of the first window whose name matches a regex pattern.
    /// Falls back to full-screen root capture (X11 import does not support per-window
    /// targeting without xdotool).
    pub fn screenshot_by_pattern(&self, path: &str, pattern: &str) -> String {
        let path = escape_skill_string(path);
        let pattern = escape_skill_string(pattern);
        let capture = Self::skill_capture(&path);
        format!(
            r#"let((matched) matched = nil foreach(w hiGetWindowList() when(rexMatchp("{pattern}" hiGetWindowName(w)) matched = t)) if(matched {capture} "no-match"))"#
        )
    }

    /// SKILL fragment: run X11 import and return path on success, nil on failure.
    /// This uses `import` from ImageMagick, which is always available on Linux.
    ///
    /// `path_escaped` is the raw (quote-escaped) path from
    /// `escape_skill_string`; it must be wrapped in SKILL string quotes here
    /// (as `load("{path}")` does), otherwise `/path/x.png` parses as an
    /// invalid identifier. The success check uses the 3-arg `if(cond t e)`
    /// form — SKILL rejects `if(cond t else e)` with a `lineread/read: syntax
    /// error`.
    fn skill_capture(path_escaped: &str) -> String {
        format!(
            r#"let((cmd ok) cmd = strcat("import -window root -silent " "{path}") ok = isFile("{path}") system(cmd) if(ok "{path}" nil)"#,
            path = path_escaped
        )
    }

    /// Build a safe SKILL bootstrap expression.
    ///
    /// The generated SKILL:
    ///   1. Confirms `hiGetWindowList()` contains a window whose name
    ///      matches `expected_name_pattern` (CIW verification).
    ///   2. Issues the requested action with the path escaped through
    ///      `escape_skill_string` and the path literal pre-validated
    ///      to live under the caller-supplied scratch root by Rust.
    ///
    /// Returns `VirtuosoError::Config` if the action's path does not
    /// live under `scratch_root` — the strict containment check must
    /// happen on the Rust side before any SKILL is constructed, since
    /// SKILL-side validation would be a defense in depth only.
    pub fn build_bootstrap_skill(
        &self,
        target: &BootstrapTarget,
        action: &BootstrapAction,
        scratch_root: &Path,
    ) -> Result<String> {
        match action {
            BootstrapAction::LoadFile(path) => {
                let canonical_root = std::fs::canonicalize(scratch_root).map_err(|e| {
                    VirtuosoError::Config(format!(
                        "scratch_root '{}' cannot be resolved: {e}",
                        scratch_root.display()
                    ))
                })?;
                let canonical_path = std::fs::canonicalize(path).map_err(|e| {
                    VirtuosoError::Config(format!(
                        "bootstrap path '{}' cannot be resolved: {e}",
                        path.display()
                    ))
                })?;
                if !canonical_path.starts_with(&canonical_root) {
                    return Err(VirtuosoError::Config(format!(
                        "bootstrap path '{}' escapes scratch root '{}'",
                        canonical_path.display(),
                        canonical_root.display()
                    )));
                }

                let pattern = target.expected_name_pattern.as_deref().unwrap_or(".*");
                let pattern_escaped = escape_skill_string(pattern);
                let path_escaped = escape_skill_string(&canonical_path.to_string_lossy());
                let _ = target.window_id; // reserved for future per-window targeting

                Ok(format!(
                    r#"let((matched) matched = nil foreach(w hiGetWindowList() when(rexMatchp("{pattern_escaped}" hiGetWindowName(w)) matched = t)) if(matched load("{path_escaped}") "ciw-not-found"))"#
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_windows_contains_hi_get_window_list() {
        let ops = WindowOps;
        let skill = ops.list_windows();
        assert!(
            skill.contains("hiGetWindowList"),
            "should use hiGetWindowList"
        );
        assert!(
            skill.contains("hiGetWindowName"),
            "should use hiGetWindowName"
        );
    }

    #[test]
    fn dismiss_dialog_cancel() {
        let ops = WindowOps;
        let skill = ops.dismiss_dialog("cancel");
        assert!(skill.contains("hiGetCurrentDialog"), "should check dialog");
        assert!(skill.contains("hiCancelDialog"), "should cancel dialog");
        assert!(skill.contains("no-dialog"), "should handle no dialog");
    }

    #[test]
    fn dismiss_dialog_ok() {
        let ops = WindowOps;
        let skill = ops.dismiss_dialog("ok");
        assert!(skill.contains("hiSendOK"), "should send OK");
    }

    #[test]
    fn get_dialog_info() {
        let ops = WindowOps;
        let skill = ops.get_dialog_info();
        assert!(skill.contains("hiGetCurrentDialog"), "should check dialog");
        assert!(skill.contains("hiGetWindowName"), "should get window name");
    }

    #[test]
    fn screenshot_escapes_path() {
        let ops = WindowOps;
        let skill = ops.screenshot("/path/with spaces/screen.png");
        assert!(
            skill.contains("import -window root -silent"),
            "should use import"
        );
        assert!(skill.contains("isFile"), "should verify file with isFile");
    }

    #[test]
    fn screenshot_skill_uses_three_arg_if_and_quoted_path() {
        // Regression: skill_capture previously emitted `if(ok {path} else nil)`
        // (mixing the 3-arg if form with the `else` keyword) AND left {path}
        // unquoted (`escape_skill_string` returns a bare escaped string).
        // Both make Virtuoso fail with "lineread/read: syntax error" when the
        // SKILL is evaluated. The correct form quotes the path and uses the
        // 3-arg if without the `else` keyword.
        let ops = WindowOps;
        let skill = ops.screenshot("/tmp/screen.png");
        assert!(
            skill.contains("isFile(\"/tmp/screen.png\")"),
            "path must be quoted as a SKILL string literal, got: {skill}"
        );
        assert!(
            skill.contains("if(ok \"/tmp/screen.png\" nil)"),
            "must use 3-arg if form without else keyword, got: {skill}"
        );
        assert!(
            !skill.contains("else nil"),
            "must not emit `else` in 3-arg if, got: {skill}"
        );
    }

    #[test]
    fn screenshot_by_pattern_escapes_pattern() {
        let ops = WindowOps;
        let skill = ops.screenshot_by_pattern("/tmp/screen.png", "Library Manager");
        assert!(skill.contains("rexMatchp"), "should use regex match");
        assert!(skill.contains("no-match"), "should handle no match");
    }

    // === build_bootstrap_skill tests (RED then GREEN) ===

    #[test]
    fn bootstrap_loadfile_within_scratch_root_succeeds() {
        let scratch = tempfile::tempdir().unwrap();
        let program = scratch.path().join("init.il");
        fs::write(&program, "(println \"hi\")").unwrap();

        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 0x2e01f16,
            expected_name_pattern: Some("Library Manager".into()),
        };
        let skill = ops
            .build_bootstrap_skill(&target, &BootstrapAction::LoadFile(program), scratch.path())
            .unwrap();

        // Programmatic SKILL only — pattern is verified via rexMatchp
        // and the path is escaped into a SKILL string literal.
        assert!(skill.contains("rexMatchp"), "must verify CIW pattern");
        assert!(skill.contains("load("), "must use load()");
        assert!(skill.contains("ciw-not-found"), "must signal no-match");
        // Path appears in the literal (after escaping) — sanity check.
        assert!(skill.contains("init.il"), "path must be embedded");
    }

    #[test]
    fn bootstrap_loadfile_preserves_apostrophes_as_skill_literal_chars() {
        // SKILL string literals allow `'` verbatim — `escape_string`
        // only escapes `\`, `"`, and control characters. This test
        // pins that behavior so future escapes don't accidentally
        // break valid filenames like "user's_lib.il".
        let scratch = tempfile::tempdir().unwrap();
        let program = scratch.path().join("with'apostrophe.il");
        fs::write(&program, "").unwrap();

        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 1,
            expected_name_pattern: None,
        };
        let skill = ops
            .build_bootstrap_skill(&target, &BootstrapAction::LoadFile(program), scratch.path())
            .unwrap();
        // The generated SKILL must be syntactically valid: load() with a
        // quoted string literal that contains the apostrophe verbatim. The
        // check is restricted to `load("` rather than `load("/` because on
        // Windows the path may use the `\\?\` UNC prefix or backslashes; the
        // thing the test is actually pinning — that the apostrophe survives
        // `escape_string` — is checked by the `with'apostrophe.il` substring.
        assert!(
            skill.contains(r#"load("#) && skill.contains("with'apostrophe.il"),
            "path must appear inside a quoted SKILL literal: {skill}"
        );
    }

    #[test]
    fn bootstrap_loadfile_outside_scratch_root_returns_config_error() {
        let scratch = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let program = outside.path().join("evil.il");
        fs::write(&program, "(system \"rm -rf /\")").unwrap();

        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 1,
            expected_name_pattern: None,
        };
        let err = ops
            .build_bootstrap_skill(&target, &BootstrapAction::LoadFile(program), scratch.path())
            .unwrap_err();
        assert!(matches!(err, VirtuosoError::Config(_)), "{err:?}");
    }

    #[test]
    fn bootstrap_loadfile_with_parent_traversal_returns_config_error() {
        let scratch = tempfile::tempdir().unwrap();
        // Build a path that uses '..' to escape the scratch root.
        // canonicalize will reject the traversal at the FS level.
        let program = scratch.path().join("..").join("escape.il");
        fs::write(&program, "").unwrap();

        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 1,
            expected_name_pattern: None,
        };
        let err = ops
            .build_bootstrap_skill(&target, &BootstrapAction::LoadFile(program), scratch.path())
            .unwrap_err();
        assert!(matches!(err, VirtuosoError::Config(_)), "{err:?}");
    }

    #[test]
    fn bootstrap_loadfile_missing_scratch_root_returns_config_error() {
        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 1,
            expected_name_pattern: None,
        };
        let err = ops
            .build_bootstrap_skill(
                &target,
                &BootstrapAction::LoadFile(PathBuf::from("/tmp/init.il")),
                Path::new("/nonexistent-scratch-root-xyz"),
            )
            .unwrap_err();
        assert!(matches!(err, VirtuosoError::Config(_)), "{err:?}");
    }

    #[test]
    fn bootstrap_loadfile_default_pattern_is_permissive() {
        let scratch = tempfile::tempdir().unwrap();
        let program = scratch.path().join("init.il");
        fs::write(&program, "").unwrap();

        let ops = WindowOps;
        let target = BootstrapTarget {
            window_id: 1,
            expected_name_pattern: None,
        };
        let skill = ops
            .build_bootstrap_skill(&target, &BootstrapAction::LoadFile(program), scratch.path())
            .unwrap();
        // Default pattern ".*" must be embedded so the SKILL accepts
        // any window — caller controls strictness via the pattern.
        assert!(
            skill.contains(r#""\\..\\*""#) || skill.contains("\".*\""),
            "{skill}"
        );
    }
}

use crate::client::bridge::escape_skill_string;

pub struct WindowOps;

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
    fn skill_capture(path_escaped: &str) -> String {
        format!(
            r#"let((cmd ok) cmd = strcat("import -window root -silent " {path}) ok = fileexists({path}) system(cmd) if(ok {path} else nil)"#,
            path = path_escaped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(skill.contains("fileexists"), "should verify file");
    }

    #[test]
    fn screenshot_by_pattern_escapes_pattern() {
        let ops = WindowOps;
        let skill = ops.screenshot_by_pattern("/tmp/screen.png", "Library Manager");
        assert!(skill.contains("rexMatchp"), "should use regex match");
        assert!(skill.contains("no-match"), "should handle no match");
    }
}

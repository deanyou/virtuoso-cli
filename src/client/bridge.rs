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
            session_id: resolved_session_id,
            whitelist: EvalstringWhitelist::default(),
            capabilities: CapabilitySet::from_env(),
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

    /// Query the daemon's Unix `$USER` via `getShellEnvVar`.
    ///
    /// Best-effort identity check used to detect SSH-tunnel-to-wrong-user
    /// misconfigurations (see `daemon_user_check`).
    /// Uses `execute_skill_unchecked` so the check works without the
    /// Admin capability — the SKILL payload is a fixed literal.
    ///
    /// Returns:
    /// - `Ok(Some(user))` when the daemon returned a non-nil string
    /// - `Ok(None)` when the daemon returned `nil` or empty (no user set)
    /// - `Err(_)` on transport failure (caller decides whether to surface)
    pub fn get_daemon_user(&self) -> Result<Option<String>> {
        const SKILL: &str =
            r#"let((u) u = getShellEnvVar("USER") if(u && u != "" then u else nil))"#;
        let r = self.execute_skill_unchecked(SKILL, Some(5))?;
        if !r.skill_ok() {
            // nil/empty = no USER env var on daemon — treat as unknown, not error
            return Ok(None);
        }
        // output is already unquoted by SKILL when string returned
        let user = r.output.trim().trim_matches('"').to_string();
        if user.is_empty() || user == "nil" {
            Ok(None)
        } else {
            Ok(Some(user))
        }
    }

    /// Probe the daemon with a short SKILL expression. Returns true if the
    /// daemon answered (STX) AND the response was non-nil. Used to detect
    /// "port-open-but-daemon-stuck" states that the plain TCP liveness check
    /// misses.
    ///
    /// Uses a no-op `(+ 1 1)` instead of `ipcIsProcessRunning()` because the
    /// latter requires a specific process-handle argument and returns nil
    /// (falsy) when called without one.
    pub fn daemon_alive(&self) -> bool {
        const SKILL: &str = r#"plus(1 1)"#;
        match self.execute_skill_unchecked(SKILL, Some(3)) {
            Ok(r) => r.skill_ok(),
            Err(_) => false,
        }
    }

    pub fn load_il(&self, local_path: &str) -> Result<VirtuosoResult> {
        let filename = std::path::Path::new(local_path)
            .file_name()
            .ok_or_else(|| VirtuosoError::Config(format!("invalid path: {local_path}")))?
            .to_string_lossy();
        // Scope remote scratch by client_id to avoid name collisions when
        // multiple local machines share one remote Unix account.
        let client_id = resolve_client_id();
        let remote_path = format!("/tmp/virtuoso_bridge/{client_id}/{filename}");

        // Best-effort: ensure the per-client dir exists on the remote host
        // (no-op for local mode since upload_file uses std::fs::copy).
        self.ensure_remote_dir(&format!("/tmp/virtuoso_bridge/{client_id}"))?;

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

    /// Best-effort mkdir of a remote directory. In local mode the std::fs::copy
    /// in `upload_file` will create the parent implicitly; this just no-ops.
    /// Over SSH it issues a `mkdir -p` via the tunnel's SSH runner.
    pub fn ensure_remote_dir(&self, dir: &str) -> Result<()> {
        if let Some(ref tunnel) = self.tunnel {
            let runner = &tunnel.runner;
            runner
                .run_command(&format!("mkdir -p {dir}"), None)
                .map_err(|e| VirtuosoError::Connection(format!("mkdir {dir}: {e}")))?;
        }
        Ok(())
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
    pub fn tunnel(&self) -> Option<&SSHClient> {
        self.tunnel.as_ref()
    }

    /// Detect the Virtuoso IC version by querying the daemon.
    pub fn version(&self) -> Result<VirtuosoVersion> {
        crate::version::detect_version(self)
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
    let lower = code.to_lowercase();
    if (lower.contains("(system") || lower.contains("(sh"))
        && (lower.contains("find /") || lower.contains("find \"/"))
    {
        return Some(
            "Blocked: system()/sh() with recursive 'find /' can hang the SKILL daemon. \
             Use a specific directory instead (e.g., find /home/...)."
                .into(),
        );
    }
    None
}

/// Returns true for stale `"sync_N"` responses queued from a previous session.
fn is_stale_sync(payload: &str) -> bool {
    let p = payload.trim().trim_matches('"');
    p.starts_with("sync_") && p[5..].parse::<u32>().is_ok()
}

pub fn escape_skill_string(s: &str) -> String {
    // Escape backslash first, then other special characters
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\0' => result.push_str("\\000"),
            c if c.is_control() => {
                result.push('\\');
                result.push(c);
            }
            c => result.push(c),
        }
    }
    result
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

/// Read a file from the remote filesystem via SKILL's infile/gets channel.
///
/// This is the CORRECT way to read file contents in Virtuoso SKILL — NOT via
/// `system("cat file")` or `run_shell_command`, which only return the system()
/// status token ("t" for success) in the output, NOT the actual file content.
///
/// ## Why not `run_shell_command("tail file")`?
///
/// `run_shell_command` (SKILL `system()`) returns only the exit status in
/// `.output`, not the stdout. On Unix, system() returns 0 for success, and
/// the actual output goes to the parent process's stdout — invisible to the
/// SKILL bridge.
///
/// ## The correct pattern
///
/// Use SKILL's `infile`/`gets` to read the file, which routes through the
/// `execute_skill` return channel:
///
/// ```rust,ignore
/// use virtuoso_cli::client::bridge::{skill_read_file, decode_skill_string};
/// let skill = skill_read_file("/path/to/log.txt");
/// let result = client.execute_skill(&skill, None)?;
/// let content = decode_skill_string(&result.output);
/// ```
pub fn skill_read_file(path: &str) -> String {
    let escaped = escape_skill_string(path);
    format!(
        r#"let((p line body)
  p = infile("{escaped}")
  body = ""
  when(p
    while(gets(line p) body = strcat(body line))
    close(p))
  body)"#
    )
}

/// Decode a SKILL-returned string: strip outer quotes, unescape \\n and \\".
///
/// SKILL strings returned through execute_skill come wrapped in quotes with
/// escaped characters (\\n for newline, \\" for quote, \\\\ for backslash).
pub fn decode_skill_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    }
}

/// Wait for a completion marker in a log file, polling until found or timeout.
///
/// This pattern is essential for operations that write files asynchronously
/// (e.g., strmin GDS import) where the file exists from time 0 (stale content)
/// and we must wait for the new content to be written.
///
/// ## Usage
///
/// ```rust,ignore
/// use virtuoso_cli::client::bridge::poll_log_completion;
/// let (fail_reason, completed) = poll_log_completion(
///     client,
///     "/path/to/strmIn.log",
///     "XSTRM-234",  // completion marker
///     600,          // timeout seconds
///     3,            // poll interval seconds
/// )?;
/// if fail_reason.is_some() {
///     // Handle error
/// }
/// if completed {
///     // Safe to read bbox or verify results
/// }
/// ```
#[allow(dead_code)]
pub fn poll_log_completion(
    client: &VirtuosoClient,
    log_path: &str,
    completion_marker: &str,
    timeout_s: u64,
    poll_interval_s: u64,
) -> Result<(Option<String>, bool)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    let read_skill = skill_read_file(log_path);

    loop {
        let result = client.execute_skill_unchecked(&read_skill, None)?;
        let content = decode_skill_string(&result.output);

        // Check for failure markers
        let fail_reason = if content.contains("ERROR") || content.contains("failed") {
            Some("Log contains error indicators".to_string())
        } else {
            None
        };

        // Check for completion marker
        let completed = content.contains(completion_marker);

        if fail_reason.is_some() || completed {
            return Ok((fail_reason, completed));
        }

        if std::time::Instant::now() >= deadline {
            return Ok((Some("Timeout waiting for completion".to_string()), false));
        }

        std::thread::sleep(std::time::Duration::from_secs(poll_interval_s));
    }
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
        // SKILL syntax uses (system "command")
        assert!(check_blocking_skill("(system \"find /\")").is_some());
        assert!(check_blocking_skill("(sh \"find /\")").is_some());
    }

    #[test]
    fn blocking_skill_find_absolute_path_blocked() {
        // Any system()/sh() with "find /" (absolute path) is blocked, not just root
        assert!(check_blocking_skill("(system \"find /home/meow\")").is_some());
        assert!(check_blocking_skill("(system \"find /tmp\")").is_some());
    }

    #[test]
    fn blocking_skill_find_relative_path_allowed() {
        // Relative paths without "/" don't match "find /"
        assert!(check_blocking_skill("(system \"find . -name foo\")").is_none());
        assert!(check_blocking_skill("(system \"find sim -name *.psf\")").is_none());
    }

    #[test]
    fn blocking_skill_no_system_call_is_allowed() {
        assert!(check_blocking_skill("1 + 1").is_none());
        assert!(check_blocking_skill("getVersion()").is_none());
        assert!(check_blocking_skill("maeGetSessions()").is_none());
    }

    #[test]
    fn skill_read_file_generates_valid_skill() {
        let skill = skill_read_file("/path/to/log.txt");
        assert!(skill.contains("infile("));
        assert!(skill.contains("gets(line p)"));
        assert!(skill.contains("close(p)"));
        // Path should be escaped (backslashes before /)
        assert!(
            skill.contains("\\/path\\/to\\/log.txt") || skill.contains("/path/to/log.txt"),
            "Path should be escaped or present in skill: {}",
            skill
        );
    }

    #[test]
    fn decode_skill_string_with_quotes() {
        // SKILL returns strings wrapped in quotes
        assert_eq!(decode_skill_string(r#""hello world""#), "hello world");
    }

    #[test]
    fn decode_skill_string_with_escapes() {
        assert_eq!(decode_skill_string(r#""line1\nline2""#), "line1\nline2");
        assert_eq!(decode_skill_string(r#""say \"hi\"""#), "say \"hi\"");
        assert_eq!(decode_skill_string(r#""path\\to\\file""#), "path\\to\\file");
    }

    #[test]
    fn decode_skill_string_no_quotes() {
        // Already unquoted
        assert_eq!(decode_skill_string("plain text"), "plain text");
    }

    #[test]
    fn decode_skill_string_mixed() {
        // Multiple escape sequences
        assert_eq!(
            decode_skill_string(r#""first\nsecond\"third\\fourth""#),
            "first\nsecond\"third\\fourth"
        );
    }
}

// =============================================================================
// Client identity (used to scope remote scratch paths)
// =============================================================================

/// Resolve a stable per-client identifier used to scope the remote
/// `/tmp/virtuoso_bridge/{client_id}/` scratch directory. Avoids collisions
/// when multiple local machines share one remote Unix account.
///
/// Priority:
/// 1. `VB_CLIENT_ID` env var (explicit override)
/// 2. Profile name from `VB_PROFILE` (set by `--profile` flag)
/// 3. Local hostname (via `gethostname()`) — last-resort, still unique
#[doc(hidden)]
pub fn resolve_client_id() -> String {
    if let Ok(id) = std::env::var("VB_CLIENT_ID") {
        let id = id.trim();
        if !id.is_empty() {
            return sanitize_client_id(id);
        }
    }
    if let Ok(profile) = std::env::var("VB_PROFILE") {
        let p = profile.trim();
        if !p.is_empty() {
            return sanitize_client_id(p);
        }
    }
    // Fallback: hostname (or "default" if even that fails).
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            // Use libc gethostname when available without pulling a crate.
            let mut buf = [0u8; 256];
            #[cfg(unix)]
            unsafe {
                let ret = libc_gethostname(buf.as_mut_ptr() as *mut _, buf.len());
                if ret == 0 {
                    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    return std::str::from_utf8(&buf[..nul]).ok().map(String::from);
                }
            }
            None
        })
        .unwrap_or_else(|| "default".to_string());
    sanitize_client_id(&host)
}

/// Strip filesystem-unsafe characters from a client id. Conservative: keep
/// alphanumerics, dash, underscore, dot; replace everything else with `_`.
#[doc(hidden)]
pub fn sanitize_client_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(unix)]
extern "C" {
    fn gethostname(buf: *mut std::ffi::c_char, len: usize) -> i32;
}

#[cfg(unix)]
unsafe fn libc_gethostname(buf: *mut std::ffi::c_char, len: usize) -> i32 {
    gethostname(buf, len)
}

/// Return the canonical remote scratch root for this client.
///
/// Public so tests and other code can construct the same path the bridge uses.
#[allow(dead_code)]
pub fn remote_scratch_root() -> String {
    format!("/tmp/virtuoso_bridge/{}", resolve_client_id())
}

#[cfg(test)]
mod client_id_tests {
    use super::*;
    use std::sync::Mutex;

    // `std::env::set_var` is process-wide; serialize these tests with a
    // global mutex so they don't race with each other when cargo runs them
    // in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_env() {
        std::env::remove_var("VB_CLIENT_ID");
        std::env::remove_var("VB_PROFILE");
        std::env::remove_var("HOSTNAME");
    }

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize_client_id("abc-DEF_1.2"), "abc-DEF_1.2");
    }

    #[test]
    fn sanitize_replaces_path_separators() {
        assert_eq!(sanitize_client_id("a/b\\c:d"), "a_b_c_d");
    }

    #[test]
    fn sanitize_drops_unicode_replaces_with_underscore() {
        // We only preserve ASCII alphanumeric; non-ASCII chars (including
        // CJK ideographs) get replaced with '_'. The 2-byte UTF-8 sequence
        // for '主' is 3 bytes, so "主机" (2 chars) yields 2 underscores.
        assert_eq!(sanitize_client_id("meow-主机"), "meow-__");
    }

    #[test]
    fn remote_scratch_root_format() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("VB_CLIENT_ID", "test-client");
        let root = remote_scratch_root();
        assert_eq!(root, "/tmp/virtuoso_bridge/test-client");
        clear_env();
    }

    #[test]
    fn resolve_client_id_precedence() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("VB_PROFILE", "myprofile");
        // No VB_CLIENT_ID, has VB_PROFILE → use profile
        assert_eq!(resolve_client_id(), "myprofile");
        std::env::set_var("VB_CLIENT_ID", "explicit");
        // VB_CLIENT_ID wins over VB_PROFILE
        assert_eq!(resolve_client_id(), "explicit");
        clear_env();
    }

    #[test]
    fn resolve_client_id_empty_falls_through() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("VB_CLIENT_ID", "  ");
        std::env::set_var("VB_PROFILE", "fallback-prof");
        // Empty VB_CLIENT_ID falls through to VB_PROFILE
        assert_eq!(resolve_client_id(), "fallback-prof");
        clear_env();
    }
}

use crate::capability::CapabilitySet;
use crate::client::layout_ops::LayoutOps;
use crate::client::maestro_ops::MaestroOps;
use crate::client::schematic_ops::SchematicOps;
use crate::client::whitelist::EvalstringWhitelist;
use crate::client::window_ops::WindowOps;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, SessionInfo, VirtuosoResult};
use crate::transport::contract::CommandRequest;
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
    read_timeout: u64,
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
            read_timeout: 120,
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

    /// Returns the configured read timeout in seconds.
    /// Used for read-heavy operations (list_instances, list_nets, etc.)
    /// that may take longer than the default timeout on large schematics.
    pub fn read_timeout(&self) -> u64 {
        self.read_timeout
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
        Self::from_config(cfg, None)
    }

    /// Build a client from an already-resolved CommandContext (P0-A). The
    /// config is parsed exactly once in `main()`; this path never re-reads the
    /// environment for host/port/backend. Session selection still follows
    /// `--session`/`VB_SESSION` and is validated against the context's target.
    pub fn from_context(ctx: &crate::context::CommandContext) -> Result<Self> {
        Self::from_config(ctx.config().clone(), ctx.target_id().map(str::to_string))
    }

    fn from_config(cfg: Config, target_id: Option<String>) -> Result<Self> {
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
        let (port, resolved_session_id, resolved_session) =
            if let Some(base_port) = tunnel.as_ref().and_then(|t| t.saved_port()) {
                (base_port, None, None)
            } else if let Ok(session_id) = std::env::var("VB_SESSION") {
                match crate::models::SessionInfo::load(&session_id) {
                    Ok(s) => {
                        tracing::info!("connecting to session '{}' on port {}", s.id, s.port);
                        (s.port, Some(s.id.clone()), Some(s))
                    }
                    Err(error) => {
                        return Err(crate::error::VirtuosoError::Config(format!(
                            "session '{session_id}' not found: {error}. Run `vcli session list`."
                        )));
                    }
                }
            } else {
                let live_sessions = crate::models::SessionInfo::list_alive();
                match live_sessions.len() {
                    1 => {
                        let s = live_sessions.into_iter().next().unwrap();
                        tracing::info!("auto-selected session '{}' on port {}", s.id, s.port);
                        (s.port, Some(s.id.clone()), Some(s))
                    }
                    n if n > 1 => {
                        let ids: Vec<&str> = live_sessions.iter().map(|s| s.id.as_str()).collect();
                        return Err(crate::error::VirtuosoError::Config(format!(
                        "multiple Virtuoso sessions active: {}. Use --session <id> to select one.",
                        ids.join(", ")
                    )));
                    }
                    _ => (cfg.port, None, None), // 0 live sessions → use VB_PORT
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

        // Cross-user daemon guard: verify the daemon was started by the same Unix
        // user we expect before connecting. This prevents accidentally connecting
        // to a daemon started by a different user (e.g., a colleague's session).
        // Skipped if daemon_user is not yet known (populated lazily by `session show`).
        if let Some(ref session) = resolved_session {
            guard_cross_user(session)?;
        }

        // P0-A ownership (F05): when a target is selected, a session recorded
        // against a different host is an ownership violation — switching targets
        // must not silently reuse the wrong session.
        if let (Some(tid), Some(session)) = (target_id.as_deref(), resolved_session.as_ref()) {
            let target_host = cfg.remote_host.as_deref().unwrap_or("");
            if !target_host.is_empty() && session.host != target_host {
                return Err(crate::error::VirtuosoError::Config(format!(
                    "session '{}' belongs to host '{}' but target '{tid}' resolves to '{}'; \
                     refusing to reuse the wrong session (run `vcli session list`)",
                    session.id, session.host, target_host
                )));
            }
        }

        Ok(Self {
            host: "127.0.0.1".into(),
            port,
            timeout: cfg.timeout,
            read_timeout: cfg.read_timeout,
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
        self.execute_skill_with_bypass(skill_code, timeout, false)
    }

    /// Execute a SKILL expression with optional whitelist bypass.
    /// `skip_whitelist` should only be true when the caller holds Admin capability —
    /// it is the caller's responsibility to enforce that precondition.
    ///
    /// Uses [`RetryPolicy::Never`]: the request is transmitted exactly once.
    /// Explicitly idempotent callers use [`Self::execute_skill_idempotent_probe`].
    pub(crate) fn execute_skill_with_bypass(
        &self,
        skill_code: &str,
        timeout: Option<u64>,
        skip_whitelist: bool,
    ) -> Result<VirtuosoResult> {
        self.execute_skill_with_policy(skill_code, timeout, skip_whitelist, RetryPolicy::Never)
    }

    /// Execute a SKILL expression that the caller has *proven* idempotent —
    /// a read-only health probe such as `1+1` or `getVersion()`.
    ///
    /// This is the only path allowed to observe a queued-ticket marker
    /// (`sync_N`) and transmit again, and only within the original timeout
    /// budget. Anything that can mutate state (transaction commit above all)
    /// must go through [`Self::execute_skill`], which returns
    /// [`VirtuosoError::OutcomeUnknown`] instead of resending.
    pub(crate) fn execute_skill_idempotent_probe(
        &self,
        skill_code: &str,
        timeout: Option<u64>,
    ) -> Result<VirtuosoResult> {
        self.execute_skill_with_policy(skill_code, timeout, true, RetryPolicy::IdempotentProbe)
    }

    fn execute_skill_with_policy(
        &self,
        skill_code: &str,
        timeout: Option<u64>,
        skip_whitelist: bool,
        policy: RetryPolicy,
    ) -> Result<VirtuosoResult> {
        // Explicit readonly mode takes precedence even on Admin execution paths.
        let skip_whitelist = skip_whitelist && !self.whitelist.is_sandbox();
        // Phase 0: evalstring whitelist check — bypassed only by authorized callers.
        if !skip_whitelist {
            if let Some(warning) = self.whitelist.check(skill_code) {
                return Err(VirtuosoError::Execution(warning));
            }
        }
        // Guard: block SKILL expressions that can hang the daemon
        // Bypassed when skip_whitelist=true (Admin capability).
        if !skip_whitelist {
            if let Some(warning) = check_blocking_skill(skill_code) {
                return Err(VirtuosoError::Execution(warning));
            }
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

            // A queued-ticket marker: the daemon answered with `sync_N`. The
            // client cannot prove whether the ticket belongs to *this*
            // request or is stale from a previous session — so whether it may
            // transmit again is a policy decision, not an implementation
            // detail (design: "SKILL request retry policy").
            if status_byte == STX && is_stale_sync(&payload) {
                match policy {
                    RetryPolicy::Never => {
                        // Transmitted once; outcome unproven. Never resent —
                        // a non-idempotent SKILL (commit!) must not replay.
                        return Err(VirtuosoError::OutcomeUnknown(
                            "observed a queued-ticket marker (sync_N); the request was \
                             transmitted once and will not be resent because it is not \
                             marked idempotent"
                                .into(),
                        ));
                    }
                    RetryPolicy::IdempotentProbe => {
                        // Explicitly idempotent: resending cannot corrupt
                        // state. Still bounded by the original timeout
                        // budget — one logical probe must not open an
                        // unbounded number of connections.
                        let budget = std::time::Duration::from_secs(timeout);
                        if start.elapsed() >= budget {
                            return Err(VirtuosoError::Timeout(timeout));
                        }
                        continue;
                    }
                }
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

    /// Execute raw SKILL as Admin. Explicit readonly mode retains pattern checks.
    /// External callers should use this; internal callers use `execute_skill_unchecked`.
    pub fn execute_skill(&self, skill_code: &str, timeout: Option<u64>) -> Result<VirtuosoResult> {
        self.require_raw_skill_access()?;
        self.execute_skill_with_bypass(skill_code, timeout, true)
    }

    fn require_raw_skill_access(&self) -> Result<()> {
        // Auth check — validate API key if auth is enabled
        crate::auth::Auth::init();
        crate::auth::check_auth(None)?;

        // Capability check — block raw SKILL exec unless Admin
        if let Some(warning) = self.check_capability("") {
            return Err(VirtuosoError::Execution(warning));
        }
        Ok(())
    }

    /// Compatibility entry point for authorized raw SKILL execution.
    ///
    /// Used internally for ops that legitimately need `system()` (e.g. sed-based
    /// netlist injection). Requires Admin capability, checked here as well as at
    /// the caller. Explicit readonly mode remains active.
    pub fn execute_skill_admin(
        &self,
        skill_code: &str,
        timeout: Option<u64>,
    ) -> Result<VirtuosoResult> {
        self.execute_skill(skill_code, timeout)
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
        // `1+1` is a read-only probe: idempotent by construction, so it may
        // drain a stale ticket and retry within the budget.
        let result = self.execute_skill_idempotent_probe("1+1", timeout)?;
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
        // Use unchecked — capability check done at RPC dispatch level
        self.execute_skill_unchecked(&skill, None)
    }

    pub fn save_current_cellview(&self) -> Result<VirtuosoResult> {
        // Use unchecked — capability check done at RPC dispatch level
        self.execute_skill_unchecked("geSaveEdit()", None)
    }

    pub fn close_current_cellview(&self) -> Result<VirtuosoResult> {
        // Use unchecked — capability check done at RPC dispatch level
        self.execute_skill_unchecked("geCloseEdit()", None)
    }

    pub fn get_current_design(&self) -> Result<(String, String, String)> {
        let result = self.execute_skill_unchecked(
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

    /// Query the daemon-side version string (e.g. `"0.4.0-alpha.5"`).
    ///
    /// The daemon stores its version in the SKILL global `RBDVersion`, which
    /// `RBIpcErrHandler` populates by parsing the `VERSION:x.x.x` line the
    /// Rust daemon prints to stderr on startup. Used by `vcli session show`
    /// to detect CLI/daemon version skew.
    ///
    /// Uses `execute_skill_unchecked` so the query works without the Admin
    /// capability — the SKILL payload is a fixed literal reading a known
    /// global.
    ///
    /// Returns:
    /// - `Ok(Some(version))` when the daemon reported a non-empty version
    /// - `Ok(None)` when the global is unbound, empty, or equal to `"?"`
    ///   (the placeholder `ramic_bridge.il` uses when VERSION: line was
    ///   not seen — see `RBDVersion = ""` default at the top of the .il)
    /// - `Err(_)` on transport failure (caller decides whether to surface)
    pub fn get_daemon_version(&self) -> Result<Option<String>> {
        // SKILL: read the global RBDVersion. Defensive: boundp() guards
        // against SKILL that has never set it (e.g. very old .il without
        // the RBDVersion default initializer).
        const SKILL: &str = r#"let((v) v = if(boundp('RBDVersion) then RBDVersion else nil) \
            if(v && v != "" && v != "?" then v else nil))"#;
        // Idempotent probe: fixed read-only literal, may drain a stale ticket.
        let r = self.execute_skill_idempotent_probe(SKILL, Some(5))?;
        if !r.skill_ok() {
            return Ok(None);
        }
        let ver = r.output.trim().trim_matches('"').to_string();
        if ver.is_empty() || ver == "nil" {
            Ok(None)
        } else {
            Ok(Some(ver))
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
        // Idempotent probe: fixed read-only literal, may drain a stale ticket.
        let r = self.execute_skill_idempotent_probe(SKILL, Some(5))?;
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
        // Explicitly idempotent probe: it must survive a stale queued ticket,
        // which is exactly the stuck-state it exists to detect.
        match self.execute_skill_idempotent_probe(SKILL, Some(3)) {
            Ok(r) => r.skill_ok(),
            Err(_) => false,
        }
    }

    pub fn load_il(&self, local_path: &str, skillpp: bool) -> Result<VirtuosoResult> {
        // Loading executes arbitrary code: authorize before filesystem/SSH side effects.
        self.require_raw_skill_access()?;
        if self.whitelist.is_sandbox() {
            return Err(VirtuosoError::Execution(
                "SKILL file loading is not permitted in readonly mode".into(),
            ));
        }

        // Do not resolve the final symlink: its .ils suffix selects SKILL++ mode.
        let path = std::path::absolute(local_path)?;
        let metadata = std::fs::metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                VirtuosoError::NotFound(format!("SKILL file not found: {local_path}"))
            } else {
                VirtuosoError::Io(error)
            }
        })?;
        if !metadata.is_file() {
            return Err(VirtuosoError::Config(format!(
                "SKILL path is not a file: {local_path}"
            )));
        }

        // --skillpp: if the file is .il but contains SKILL++ code, copy to a
        // temp .ils file so Virtuoso's load() selects SKILL++ mode. The .il
        // extension forces standard SKILL mode, where globalProc/defclass/
        // defmethod are undefined. .ils files are already SKILL++ — no copy needed.
        let effective_path = if skillpp && path.extension().and_then(|e| e.to_str()) == Some("il") {
            let tmp = std::env::temp_dir().join(format!(
                "vcli_skillpp_{}.ils",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill")
            ));
            std::fs::copy(&path, &tmp).map_err(VirtuosoError::Io)?;
            tmp
        } else {
            path.clone()
        };

        let loaded_path = if let Some(tunnel) = &self.tunnel {
            let transport = tunnel.transport();
            super::skill_loading::stage_remote_file(transport.as_ref(), &effective_path)?
        } else {
            effective_path
                .to_str()
                .ok_or_else(|| VirtuosoError::Config("SKILL path must be UTF-8".into()))?
                .to_owned()
        };
        let skill = format!(r#"load("{}")"#, escape_skill_string(&loaded_path));
        let mut result = self
            .execute_skill(&skill, None)?
            .ok_or_exec(&format!("load SKILL file {loaded_path}"))?;
        result.metadata.insert("loaded_path".into(), loaded_path);
        if skillpp {
            result.metadata.insert("skillpp_mode".into(), "true".into());
        }
        Ok(result)
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

    /// Create a directory on the execution host, propagating creation failures.
    ///
    /// The `dir` argument is shell-quoted defensively even though current
    /// callers only ever pass a `sanitize_client_id()`-output path
    /// (which is already alnum + `-_.`). If a future caller passes a
    /// user-controlled path, the quoting still prevents shell injection.
    pub fn ensure_remote_dir(&self, dir: &str) -> Result<()> {
        if let Some(ref tunnel) = self.tunnel {
            let transport = tunnel.transport();
            let quoted = crate::transport::ssh::shell_quote(dir);
            let result = transport
                .run_command(&CommandRequest::untimed(format!("mkdir -p {quoted}")))
                .map_err(|e| VirtuosoError::Connection(format!("mkdir {dir}: {e}")))?;
            if !result.success || result.exit_status != 0 {
                return Err(VirtuosoError::Ssh(format!(
                    "mkdir {dir} failed (exit {}): {}",
                    result.exit_status,
                    result.stderr.trim()
                )));
            }
        } else {
            std::fs::create_dir_all(dir)?;
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
    ///
    /// Uses `plus(1 1)` as a no-op probe because `ipcIsProcessRunning()` (the
    /// previously-used probe) requires a specific process-handle argument and
    /// returns nil/empty when called without one — causing every ping to
    /// fail on a live daemon. See `daemon_alive()` for the same pattern.
    pub fn ping(&self) -> Result<()> {
        let skill = "plus(1 1)";
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

/// Guard: verify the daemon's Unix user matches VB_REMOTE_USER before connecting.
///
/// This runs in `VirtuosoClient::from_env()` after session resolution, preventing
/// accidental connection to a daemon started by a different Unix user. The check
/// only applies when `session.daemon_user` is already known (populated by a prior
/// `vcli session show` call). If `daemon_user` is `None`, the check is skipped
/// with a debug log (the user will be unknown until first `session show`).
///
/// Set `VB_ALLOW_CROSS_USER_DAEMON=1` to disable this guard intentionally.
fn guard_cross_user(session: &SessionInfo) -> Result<()> {
    // Get expected user from VB_REMOTE_USER[<profile>] or plain VB_REMOTE_USER
    let profile = std::env::var("VB_PROFILE").ok();
    let expected = std::env::var(format!(
        "VB_REMOTE_USER{}",
        profile
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|p| format!("_{p}"))
            .unwrap_or_default()
    ))
    .ok()
    .or_else(|| std::env::var("VB_REMOTE_USER").ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());

    // If no expected user configured, skip the check
    let expected = match expected {
        Some(e) => e,
        None => return Ok(()),
    };

    // If daemon_user is not yet known (None), we can't perform the check
    let daemon_user = match session.daemon_user.as_deref() {
        Some(u) => u,
        None => {
            tracing::debug!(
                "cross-user guard: daemon_user unknown for session '{}', skipping check",
                session.id
            );
            return Ok(());
        }
    };

    // If users match, allow
    if daemon_user == expected {
        return Ok(());
    }

    // Allow override via VB_ALLOW_CROSS_USER_DAEMON=1
    if std::env::var("VB_ALLOW_CROSS_USER_DAEMON")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
    {
        tracing::warn!(
            "cross-user guard: VB_ALLOW_CROSS_USER_DAEMON=1 set, allowing daemon user '{}' (expected '{}')",
            daemon_user,
            expected
        );
        return Ok(());
    }

    Err(VirtuosoError::Connection(format!(
        "daemon Unix user '{}' does not match configured VB_REMOTE_USER '{}' for session '{}'. \
         Set VB_ALLOW_CROSS_USER_DAEMON=1 to override if you intentionally want to connect.",
        daemon_user, expected, session.id
    )))
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

/// Whether, and under which proof, the client may transmit a SKILL request
/// again after observing a queued-ticket marker (`sync_N`).
///
/// Design invariant ("SKILL request retry policy"): a request is transmitted
/// once. Only callers that have *proven* the request idempotent may observe a
/// ticket and send again, and only within the original timeout budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryPolicy {
    /// Default. One transmission; an unprovable outcome returns
    /// [`VirtuosoError::OutcomeUnknown`].
    Never,
    /// Callers may re-transmit: the request is a read-only probe, so a
    /// resend cannot change remote state.
    IdempotentProbe,
}

/// Returns true for stale `"sync_N"` responses queued from a previous session.
fn is_stale_sync(payload: &str) -> bool {
    let p = payload.trim().trim_matches('"');
    p.starts_with("sync_") && p[5..].parse::<u32>().is_ok()
}

pub fn escape_skill_string(s: &str) -> String {
    crate::client::skill_runtime::escape_string(s)
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

    /// Mock daemon that answers `answers[i]` on the i-th connection. Each
    /// answer is the raw response bytes (status byte + payload). Returns the
    /// bound port and a handle whose drop shuts the listener down.
    fn spawn_mock_daemon(answers: Vec<Vec<u8>>) -> (u16, std::sync::Arc<()>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::sync::Arc::new(());
        let h2 = std::sync::Arc::clone(&handle);
        std::thread::spawn(move || {
            let _keep = h2;
            for (i, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let answer = answers.get(i).cloned();
                // Drain the request (bounded) so the client's write succeeds.
                let mut buf = [0u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                match answer {
                    Some(bytes) => {
                        let _ = stream.write_all(&bytes);
                    }
                    None => break, // no more scripted answers
                }
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        (port, handle)
    }

    #[test]
    fn never_policy_returns_outcome_unknown_on_a_ticket_marker() {
        let (port, _keep) = spawn_mock_daemon(vec![vec![STX]
            .into_iter()
            .chain(b"sync_1".iter().copied())
            .collect()]);
        let client = VirtuosoClient::new("127.0.0.1", port, 5);
        // Bypass the auth gate directly (the policy under test lives below
        // it); this is the exact path `execute_skill` takes after its check.
        let err = client
            .execute_skill_with_bypass("hiSetPoint(1 2)", Some(5), true)
            .expect_err("non-idempotent request must not be resent");
        assert!(
            matches!(err, VirtuosoError::OutcomeUnknown(_)),
            "got {err:?}"
        );
        // And it must not look retryable to generic retry machinery.
        assert!(!err.retryable());
    }

    #[test]
    fn idempotent_probe_may_resend_within_the_budget() {
        let ticket = [vec![STX], b"sync_1".to_vec()].concat();
        let ok = [vec![STX], b"2".to_vec()].concat();
        let (port, _keep) = spawn_mock_daemon(vec![ticket, ok]);
        let client = VirtuosoClient::new("127.0.0.1", port, 5);
        let r = client
            .execute_skill_idempotent_probe("1+1", Some(5))
            .expect("idempotent probe may drain a stale ticket and resend");
        assert_eq!(r.output, "2");
    }

    #[test]
    fn outcome_unknown_is_not_a_connection_error() {
        // The error_type must be distinct so operators (and scripts keyed on
        // JSON error_type) can tell "unproven" from "transport down".
        let e = VirtuosoError::OutcomeUnknown("x".into());
        assert_eq!(e.error_type(), "outcome_unknown");
        assert_eq!(e.exit_code(), crate::exit_codes::GENERAL_ERROR);
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
        .or_else(gethostname_fallback)
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

/// Read the hostname via libc, without pulling in a crate for it.
///
/// Unix-only. Off Unix there is no `gethostname`, so the caller's fallback
/// chain simply ends at the literal "default".
#[cfg(unix)]
fn gethostname_fallback() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a 256-byte stack buffer and `gethostname` writes at
    // most `buf.len()` bytes into it.
    unsafe {
        let ret = libc_gethostname(buf.as_mut_ptr() as *mut _, buf.len());
        if ret == 0 {
            let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return std::str::from_utf8(&buf[..nul]).ok().map(String::from);
        }
    }
    None
}

/// Non-Unix counterpart of [`gethostname_fallback`]: no `gethostname` to
/// call, so always defer to the caller's "default" fallback.
#[cfg(not(unix))]
fn gethostname_fallback() -> Option<String> {
    None
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
    use serial_test::serial;

    fn clear_env() {
        std::env::remove_var("VB_CLIENT_ID");
        std::env::remove_var("VB_PROFILE");
        std::env::remove_var("HOSTNAME");
        std::env::remove_var("VB_SESSION");
        std::env::remove_var("VB_REMOTE_HOST");
        std::env::remove_var("VB_JUMP_HOST");
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
    #[serial]
    fn remote_scratch_root_format() {
        clear_env();
        std::env::set_var("VB_CLIENT_ID", "test-client");
        let root = remote_scratch_root();
        assert_eq!(root, "/tmp/virtuoso_bridge/test-client");
        clear_env();
    }

    #[test]
    #[serial]
    fn resolve_client_id_precedence() {
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
    #[serial]
    fn resolve_client_id_empty_falls_through() {
        clear_env();
        std::env::set_var("VB_CLIENT_ID", "  ");
        std::env::set_var("VB_PROFILE", "fallback-prof");
        // Empty VB_CLIENT_ID falls through to VB_PROFILE
        assert_eq!(resolve_client_id(), "fallback-prof");
        clear_env();
    }

    #[test]
    #[serial]
    fn from_env_rejects_unknown_bridge_session() {
        clear_env();
        std::env::set_var(
            "VB_SESSION",
            format!("missing-session-{}", std::process::id()),
        );
        std::env::set_var("VB_PORT", "65534");

        let error = match VirtuosoClient::from_env() {
            Ok(_) => panic!("unknown session must be rejected"),
            Err(error) => error,
        };
        assert!(
            matches!(error, VirtuosoError::Config(_)),
            "expected config error, got: {error}"
        );

        std::env::remove_var("VB_PORT");
        clear_env();
    }
}

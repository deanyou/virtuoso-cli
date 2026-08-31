use crate::error::{Result, VirtuosoError};
use crate::transport::contract::{CommandRequest, RemoteTransport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    Success,
    Failure,
    Partial,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtuosoResult {
    pub status: ExecutionStatus,
    pub output: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub execution_time: Option<f64>,
    pub metadata: HashMap<String, String>,
}

impl VirtuosoResult {
    /// Transport-level success: bridge returned STX (not NAK/timeout).
    /// Does NOT mean the SKILL call succeeded — SKILL functions return "nil"
    /// on failure via STX. Use skill_ok() to check SKILL-level success.
    pub fn ok(&self) -> bool {
        self.status == ExecutionStatus::Success
    }

    /// True when the bridge succeeded AND SKILL returned a non-nil value.
    /// Use this whenever a SKILL function signals failure by returning nil
    /// (e.g. design(), dbOpenCellViewByType(), getData()).
    pub fn skill_ok(&self) -> bool {
        self.status == ExecutionStatus::Success && self.output.trim() != "nil"
    }

    /// Propagate a SKILL-level failure as `Err(VirtuosoError::Execution)`.
    /// `context` is the operation name; the error message becomes `"{context} failed: {detail}"`.
    /// When output is empty (NAK transport error), falls back to the first error in `errors`.
    pub fn ok_or_exec(self, context: &str) -> Result<Self> {
        if self.skill_ok() {
            Ok(self)
        } else {
            let detail = if self.output.is_empty() {
                self.errors.first().cloned().unwrap_or_default()
            } else {
                self.output.clone()
            };
            Err(VirtuosoError::Execution(format!(
                "{context} failed: {detail}"
            )))
        }
    }

    /// Return the output string with surrounding SKILL double-quotes stripped.
    pub fn output_unquoted(&self) -> &str {
        self.output.trim_matches('"')
    }

    pub fn success(output: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Success,
            output: output.into(),
            errors: Vec::new(),
            warnings: Vec::new(),
            execution_time: None,
            metadata: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn error(errors: Vec<String>) -> Self {
        Self {
            status: ExecutionStatus::Error,
            output: String::new(),
            errors,
            warnings: Vec::new(),
            execution_time: None,
            metadata: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn save_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)
    }
}

/// A scalar value extracted from PSF operating-point blocks (e.g., M0:vth, M0:region).
/// Ordered to serialize cleanly as JSON without needing complex enum tagging.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Float(f64),
    String(String),
    Integer(i64),
}

#[allow(dead_code)]
impl ScalarValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ScalarValue::Float(v) => Some(*v),
            ScalarValue::Integer(v) => Some(*v as f64),
            ScalarValue::String(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ScalarValue::String(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub status: ExecutionStatus,
    pub tool_version: Option<String>,
    /// Signal waveforms / sweep data: signal_name -> values.
    pub data: HashMap<String, Vec<f64>>,
    /// Scalar operating points extracted from PSF STRUCT/OP blocks
    /// (e.g., M0:gm, M0:vth, M0:region).
    pub operating_points: HashMap<String, ScalarValue>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub metadata: HashMap<String, String>,
}

#[allow(dead_code)]
impl SimulationResult {
    pub fn ok(&self) -> bool {
        self.status == ExecutionStatus::Success
    }

    pub fn save_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTaskResult {
    pub success: bool,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub remote_dir: Option<String>,
    pub error: Option<String>,
    pub timings: HashMap<String, f64>,
}

fn default_version() -> u32 {
    1
}

/// Runtime metrics written by the daemon to `/tmp/.ramic_stats_{port}` after each request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStats {
    pub calls: u64,
    pub errors: u64,
    pub uptime_secs: u64,
}

impl DaemonStats {
    /// Returns the path to the daemon stats file.
    /// Uses the system cache directory (e.g., ~/.cache/virtuoso_bridge/).
    fn cache_dir() -> std::path::PathBuf {
        crate::runtime_paths::cache_subdir::<&str>(&[])
    }

    pub fn path(port: u16) -> String {
        Self::cache_dir()
            .join(format!(".ramic_stats_{port}"))
            .to_string_lossy()
            .into_owned()
    }

    pub fn load(port: u16) -> Option<Self> {
        let json = std::fs::read_to_string(Self::path(port)).ok()?;
        serde_json::from_str(&json).ok()
    }
}

/// Registration record written by bridge.il when a Virtuoso session starts.
/// Lives at ~/.cache/virtuoso_bridge/sessions/<id>.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub port: u16,
    pub pid: u32,
    pub host: String,
    pub user: String,
    pub created: String,
    /// Backward-compat field for the daemon-side Unix `$USER`.
    ///
    /// Populated lazily by `vcli session show` (which queries the daemon via
    /// `getShellEnvVar("USER")` and writes the result back to the session
    /// file). When `None`, either the user has not yet run `session show`
    /// OR the query failed/returned nil.
    ///
    /// Older ramic_bridge.il versions never write this key, so the field is
    /// `#[serde(default, skip_serializing_if = "Option::is_none")]` for
    /// backward compatibility with legacy session files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_user: Option<String>,

    /// Backward-compat field for the daemon-side version string (e.g. `"0.4.0-alpha.5"`).
    ///
    /// Populated lazily by `vcli session show` (which queries the daemon's
    /// `RBDVersion` SKILL global). The `RBDVersion` global is set by
    /// `RBIpcErrHandler` parsing the `VERSION:x.x.x` line the Rust daemon
    /// prints to stderr on startup. When `None`, either the user has not
    /// yet run `session show` OR the daemon did not emit a version line
    /// (very old daemon binaries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_version: Option<String>,
}

impl SessionInfo {
    pub(crate) fn sessions_dir() -> std::path::PathBuf {
        crate::runtime_paths::cache_subdir(&["sessions"])
    }

    pub fn load(id: &str) -> std::io::Result<Self> {
        let path = Self::sessions_dir().join(format!("{id}.json"));
        let json = std::fs::read_to_string(&path)
            .map_err(|e| std::io::Error::new(e.kind(), format!("session '{id}' not found: {e}")))?;
        serde_json::from_str(&json).map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn list() -> std::io::Result<Vec<Self>> {
        let dir = Self::sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(json) = std::fs::read_to_string(&path) {
                    if let Ok(s) = serde_json::from_str::<Self>(&json) {
                        sessions.push(s);
                    }
                }
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }

    /// List sessions on a remote host via SSH.
    /// Reads all session JSON files from `~/.cache/virtuoso_bridge/sessions/`.
    pub fn list_remote(runner: &dyn RemoteTransport) -> std::io::Result<Vec<Self>> {
        let script = r#"for f in "$HOME"/.cache/virtuoso_bridge/sessions/*.json; do [ -f "$f" ] && echo "---SESSION---" && cat "$f"; done"#;
        let result = runner
            .run_command(&CommandRequest::untimed(script))
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let mut sessions = Vec::new();
        for chunk in result.stdout.split("---SESSION---") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            if let Ok(s) = serde_json::from_str::<Self>(chunk) {
                sessions.push(s);
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }

    /// Fetch remote sessions and sync them to the local sessions directory.
    /// Returns the number of sessions synced.
    pub fn sync_from_remote(runner: &dyn RemoteTransport) -> std::io::Result<usize> {
        let remote = Self::list_remote(runner)?;
        let dir = Self::sessions_dir();
        std::fs::create_dir_all(&dir)?;
        let mut count = 0;
        for s in &remote {
            let path = dir.join(format!("{}.json", s.id));
            let json = serde_json::to_string_pretty(s)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            std::fs::write(path, json)?;
            count += 1;
        }
        Ok(count)
    }

    /// Check if the daemon is still alive by checking if the port is bound.
    pub fn is_alive(&self) -> bool {
        use std::net::TcpStream;
        use std::time::Duration;
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.port).parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
    }

    /// Return only sessions whose daemon is currently alive.
    pub fn list_alive() -> Vec<Self> {
        Self::list()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.is_alive())
            .collect()
    }

    /// Best-effort write-back of an augmented session record to the per-session
    /// JSON file (the same path `load(id)` reads from). Used to persist
    /// Rust-only metadata (e.g. `daemon_user`) that the SKILL side never writes.
    ///
    /// Creates the parent directory if it does not exist (the SKILL side
    /// normally creates it on first session start, but Rust-only callers
    /// like a cold `vcli session show` may need to create it themselves).
    ///
    /// Errors are swallowed because the caller (e.g. `vcli session show`)
    /// prefers to still display fresh data over aborting on a disk failure.
    pub fn save_to_session_file(&self) {
        let dir = Self::sessions_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.json", self.id));
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// State of the SSH/tunnel process, persisted to `state.json` (see
/// [`TunnelState::state_path`]). Both the OpenSSH backend and the planned
/// native backend write this file; `tunnel stop`, `tunnel status`,
/// `tunnel diagnose`, and the TUI read it.
///
/// # Versioning
///
/// * **v1** (legacy): `{version, port, pid, remote_host, setup_path}`. A v1
///   file on disk is always treated as the OpenSSH backend, because the native
///   backend did not exist when v1 was the only shape.
/// * **v2** (this revision): adds the optional fields below. Every write made
///   after this change is v2 — including writes from the OpenSSH backend — so
///   the file never oscillates between shapes. Fields that the OpenSSH backend
///   does not yet populate (`daemon_nonce`, `ipc_endpoint`, `token_path`,
///   `executable_path`, `start_identity`, …) stay `None`; the native daemon
///   fills them in when it ships. All new fields are `Option`-typed with
///   `#[serde(default)]` so a v1 file still deserializes without a custom
///   deserializer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelState {
    /// Schema version. Defaults to 1 for legacy files that predate this field.
    #[serde(default = "default_version")]
    pub version: u32,
    pub port: u16,
    pub pid: u32,
    pub remote_host: String,
    pub setup_path: Option<String>,

    // --- v2 fields (all optional for v1 backward compatibility) ---
    /// Profile this tunnel belongs to (`None` for the default profile).
    #[serde(default)]
    pub profile: Option<String>,
    /// Selected transport backend. `None` on a v1 file means OpenSSH.
    #[serde(default)]
    pub backend: Option<String>,
    /// Per-daemon nonce issued at Hello; rotating it invalidates stale clients.
    /// Native daemon only; `None` under OpenSSH.
    #[serde(default)]
    pub daemon_nonce: Option<String>,
    /// Absolute path of the tunnel process executable (two-tier PID check).
    #[serde(default)]
    pub executable_path: Option<String>,
    /// OS start marker used to confirm the live process still matches the
    /// recorded one before `tunnel stop` signals it (PIDs are reused).
    /// Platform-specific, matching
    /// [`crate::transport::identity::ProcessIdentity::start_identity`]:
    /// Linux `/proc` starttime ticks, macOS epoch seconds, Windows creation time.
    #[serde(default)]
    pub start_identity: Option<u64>,
    /// IPC endpoint (UDS path / named pipe / TCP addr) the native daemon listens on.
    #[serde(default)]
    pub ipc_endpoint: Option<String>,
    /// Path to the current-user-only auth-token file.
    #[serde(default)]
    pub token_path: Option<String>,
    /// Human-readable summary of the forward endpoints (e.g. `L*:<port>`).
    #[serde(default)]
    pub local_forward: Option<String>,
    /// Unix epoch milliseconds when the tunnel was established.
    #[serde(default)]
    pub start_time_unix_ms: Option<u64>,
    /// Last health-probe result string.
    #[serde(default)]
    pub health: Option<String>,
    /// Digest of the resolved config, for `tunnel status` drift detection.
    #[serde(default)]
    pub config_digest: Option<String>,

    // --- v2.1 fields: tunnel lifecycle mode ---
    /// `"deployed"` (a fresh daemon was started via `tunnel start`) or
    /// `"attached"` (the tunnel piggybacks on a daemon launched by Virtuoso).
    /// `None` on legacy v2 files — treated as `"deployed"` for backwards
    /// compatibility. Plain string instead of an enum because `==` on a
    /// literal is simpler than a serde-tagged variant and the surface is
    /// narrow enough that a typo would be caught by a test.
    #[serde(default)]
    pub mode: Option<String>,
    /// Remote daemon port this tunnel forwards to. Populated only when
    /// `mode == "attached"`; `None` for deployed tunnels (the port there is
    /// the daemon's own listening port, identical to `port`).
    #[serde(default)]
    pub attached_remote_port: Option<u16>,
    /// Bridge session id this attach resolved to. Mirrors
    /// `SessionInfo::id` for the daemon we discovered via
    /// `~/.cache/virtuoso_bridge/sessions/*.json`.
    #[serde(default)]
    pub attached_session_id: Option<String>,
}

/// Mode value written to [`TunnelState::mode`] by `tunnel start`.
pub const TUNNEL_MODE_DEPLOYED: &str = "deployed";
/// Mode value written to [`TunnelState::mode`] by `tunnel attach`.
pub const TUNNEL_MODE_ATTACHED: &str = "attached";

/// Current `TunnelState` schema version written by this build.
pub const CURRENT_STATE_VERSION: u32 = 2;

impl TunnelState {
    /// Backend this state file describes. A v1 file (no `backend` field) is
    /// always OpenSSH, because the native backend did not exist when v1 was
    /// the only shape on disk.
    #[allow(dead_code)]
    pub fn backend_or_openssh(&self) -> &str {
        self.backend.as_deref().unwrap_or("openssh")
    }

    /// Whether this is a v2 (or newer) state file.
    #[allow(dead_code)]
    pub fn is_v2(&self) -> bool {
        self.version >= CURRENT_STATE_VERSION
    }

    fn state_path(profile: Option<&str>) -> std::path::PathBuf {
        // Default to state_root (XDG_STATE_HOME) and fall back to the legacy
        // ~/.cache/virtuoso_bridge/state_*.json path so older daemon
        // processes keep finding their state files after a refactor.
        let primary = crate::runtime_paths::state_root()
            .join(crate::runtime_paths::APP_DIR)
            .join(match profile {
                Some(p) if !p.is_empty() => format!("state_{p}.json"),
                _ => "state.json".into(),
            });
        let legacy = crate::runtime_paths::legacy_state_file(profile);
        if primary.exists() {
            primary
        } else if legacy.exists() {
            legacy
        } else {
            // Default to primary for new writes; create the dir on demand.
            let _ = std::fs::create_dir_all(primary.parent().unwrap_or(&primary));
            primary
        }
    }

    pub fn save_with_profile(&self, profile: Option<&str>) -> std::io::Result<()> {
        let path = Self::state_path(profile);
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_with_profile(std::env::var("VB_PROFILE").ok().as_deref())
    }

    pub fn load_with_profile(profile: Option<&str>) -> std::io::Result<Option<Self>> {
        let path = Self::state_path(profile);
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn load() -> std::io::Result<Option<Self>> {
        Self::load_with_profile(std::env::var("VB_PROFILE").ok().as_deref())
    }

    pub fn clear_with_profile(profile: Option<&str>) -> std::io::Result<()> {
        let path = Self::state_path(profile);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Clear the state file for the ambient profile taken from `VB_PROFILE`.
    ///
    /// For callers that hold no [`crate::config::Config`] — currently
    /// `tunnel detach`, which only drops the local side of an attached tunnel.
    /// Anything that already has a `Config` should prefer
    /// [`Self::clear_with_profile`] with `cfg.profile`.
    pub fn clear() -> std::io::Result<()> {
        Self::clear_with_profile(std::env::var("VB_PROFILE").ok().as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v1 state file written by an older build: no `version` field, no
    /// `backend` field, and no v2 fields at all. It must parse, be treated as
    /// the OpenSSH backend, and preserve the four original fields.
    const V1_JSON: &str = r#"{
        "port": 20022,
        "pid": 4242,
        "remote_host": "eda-host",
        "setup_path": "/home/u/.cache/virtuoso_bridge/setup"
    }"#;

    #[test]
    fn v1_file_parses_and_defaults_to_openssh() {
        let s: TunnelState = serde_json::from_str(V1_JSON).unwrap();
        assert_eq!(s.version, 1, "legacy file without `version` defaults to 1");
        assert_eq!(s.port, 20022);
        assert_eq!(s.pid, 4242);
        assert_eq!(s.remote_host, "eda-host");
        assert_eq!(
            s.setup_path.as_deref(),
            Some("/home/u/.cache/virtuoso_bridge/setup")
        );
        // v1 has no backend field → OpenSSH, and is not v2.
        assert_eq!(s.backend_or_openssh(), "openssh");
        assert!(!s.is_v2());
        // v2-only fields default to None so v1 never fails to deserialize.
        assert!(s.daemon_nonce.is_none());
        assert!(s.ipc_endpoint.is_none());
        assert!(s.executable_path.is_none());
    }

    #[test]
    fn v2_round_trips_with_backend_and_new_fields() {
        let s = TunnelState {
            version: CURRENT_STATE_VERSION,
            port: 20023,
            pid: 9999,
            remote_host: "eda-host-2".into(),
            setup_path: Some("/p/setup".into()),
            profile: Some("prod".into()),
            backend: Some("openssh".into()),
            daemon_nonce: Some("n0nce".into()),
            executable_path: Some("/usr/bin/ssh".into()),
            start_identity: Some(1767225600),
            ipc_endpoint: Some("/run/vb.sock".into()),
            token_path: Some("/run/vb.token".into()),
            local_forward: Some("L*:20023".into()),
            start_time_unix_ms: Some(1_700_000_000_000),
            health: Some("ok".into()),
            config_digest: Some("deadbeef".into()),
            mode: None,
            attached_remote_port: None,
            attached_session_id: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: TunnelState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, CURRENT_STATE_VERSION);
        assert!(back.is_v2());
        assert_eq!(back.backend_or_openssh(), "openssh");
        assert_eq!(back.profile.as_deref(), Some("prod"));
        assert_eq!(back.daemon_nonce.as_deref(), Some("n0nce"));
        assert_eq!(back.executable_path.as_deref(), Some("/usr/bin/ssh"));
        assert_eq!(back.start_identity, Some(1767225600));
        assert_eq!(back.ipc_endpoint.as_deref(), Some("/run/vb.sock"));
        assert_eq!(back.token_path.as_deref(), Some("/run/vb.token"));
        assert_eq!(back.local_forward.as_deref(), Some("L*:20023"));
        assert_eq!(back.start_time_unix_ms, Some(1_700_000_000_000));
        assert_eq!(back.health.as_deref(), Some("ok"));
        assert_eq!(back.config_digest.as_deref(), Some("deadbeef"));
    }

    /// The OpenSSH backend writes v2 with `backend = "openssh"`; unknown v2
    /// fields on a v1 reader are ignored, so an old binary still sees the
    /// original four fields. This mirrors the `SSHClient::save_state` contract.
    #[test]
    fn openssh_save_shape_is_v2_with_openssh_backend() {
        let s = TunnelState {
            version: CURRENT_STATE_VERSION,
            port: 20024,
            pid: 0,
            remote_host: "h".into(),
            setup_path: Some("/p".into()),
            profile: None,
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
            mode: None,
            attached_remote_port: None,
            attached_session_id: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        // An old reader that only knows v1 fields must still accept it.
        let legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(legacy["port"], 20024);
        assert_eq!(legacy["pid"], 0);
        assert_eq!(legacy["remote_host"], "h");
        assert_eq!(legacy["backend"], "openssh");
        assert!(legacy.get("daemon_nonce").unwrap().is_null());
    }
}

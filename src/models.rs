use crate::error::{Result, VirtuosoError};
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
    /// `context` is the operation name; the error message becomes `"{context} failed: {output}"`.
    pub fn ok_or_exec(self, context: &str) -> Result<Self> {
        if self.skill_ok() {
            Ok(self)
        } else {
            Err(VirtuosoError::Execution(format!(
                "{context} failed: {}",
                self.output
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub status: ExecutionStatus,
    pub tool_version: Option<String>,
    pub data: HashMap<String, Vec<f64>>,
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
}

impl SessionInfo {
    pub(crate) fn sessions_dir() -> std::path::PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("virtuoso_bridge")
            .join("sessions")
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
    pub fn list_remote(runner: &crate::transport::ssh::SSHRunner) -> std::io::Result<Vec<Self>> {
        let script = r#"for f in "$HOME"/.cache/virtuoso_bridge/sessions/*.json; do [ -f "$f" ] && echo "---SESSION---" && cat "$f"; done"#;
        let result = runner
            .run_command(script, None)
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
    pub fn sync_from_remote(runner: &crate::transport::ssh::SSHRunner) -> std::io::Result<usize> {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelState {
    #[serde(default = "default_version")]
    pub version: u32,
    pub port: u16,
    pub pid: u32,
    pub remote_host: String,
    pub setup_path: Option<String>,
}

impl TunnelState {
    fn state_path(profile: Option<&str>) -> std::path::PathBuf {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("virtuoso_bridge");
        let _ = std::fs::create_dir_all(&cache_dir);
        let filename = match profile {
            Some(p) if !p.is_empty() => format!("state_{p}.json"),
            _ => "state.json".into(),
        };
        cache_dir.join(filename)
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

    pub fn clear() -> std::io::Result<()> {
        Self::clear_with_profile(std::env::var("VB_PROFILE").ok().as_deref())
    }
}

use crate::client::layout_ops::LayoutOps;
use crate::client::maestro_ops::MaestroOps;
use crate::client::schematic_ops::SchematicOps;
use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, VirtuosoResult};
use crate::transport::tunnel::SSHClient;
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
    pub layout: LayoutOps,
    pub maestro: MaestroOps,
    pub schematic: SchematicOps,
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
        }
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
        // 2. No session specified → auto-select if exactly one session exists
        // 3. Fallback to VB_PORT / config.port for backward compat
        let port = if let Some(base_port) = tunnel.as_ref().and_then(|t| t.saved_port()) {
            base_port
        } else if let Ok(session_id) = std::env::var("VB_SESSION") {
            // VB_SESSION may be a Maestro session name (e.g. "fnxSession8") rather than
            // a bridge session ID — Maestro sessions don't have session files.
            // Fall back to VB_PORT in that case.
            match crate::models::SessionInfo::load(&session_id) {
                Ok(s) => {
                    tracing::info!("connecting to session '{}' on port {}", s.id, s.port);
                    s.port
                }
                Err(_) => {
                    tracing::debug!("session '{}' not a bridge session (no file), using VB_PORT", session_id);
                    cfg.port
                }
            }
        } else {
            // No session specified — try auto-discovery
            match crate::models::SessionInfo::list() {
                Ok(sessions) if sessions.len() == 1 => {
                    let s = &sessions[0];
                    tracing::info!("auto-selected session '{}' on port {}", s.id, s.port);
                    s.port
                }
                Ok(sessions) if sessions.len() > 1 => {
                    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
                    return Err(crate::error::VirtuosoError::Config(format!(
                        "multiple Virtuoso sessions active: {}. Use --session <id> to select one.",
                        ids.join(", ")
                    )));
                }
                _ => cfg.port, // 0 sessions or list failed → use VB_PORT
            }
        };

        Ok(Self {
            host: "127.0.0.1".into(),
            port,
            timeout: cfg.timeout,
            tunnel,
            layout: LayoutOps::new(),
            maestro: MaestroOps,
            schematic: SchematicOps::new(),
        })
    }

    pub fn local(host: &str, port: u16, timeout: u64) -> Self {
        Self::new(host, port, timeout)
    }

    pub fn execute_skill(&self, skill_code: &str, timeout: Option<u64>) -> Result<VirtuosoResult> {
        // Guard: block SKILL expressions that can hang the daemon
        if let Some(warning) = check_blocking_skill(skill_code) {
            return Err(VirtuosoError::Execution(warning));
        }

        let timeout = timeout.unwrap_or(self.timeout);
        let start = Instant::now();

        let addr: std::net::SocketAddr = format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|e| VirtuosoError::Connection(format!("invalid address: {e}")))?;
        let mut stream = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(timeout))
            .map_err(|e| VirtuosoError::Connection(e.to_string()))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(timeout)))
            .ok();

        let req = serde_json::json!({
            "skill": skill_code,
            "timeout": timeout
        });
        let req_bytes = serde_json::to_string(&req).map_err(VirtuosoError::Json)?;
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

        let elapsed = start.elapsed().as_secs_f64();

        if data.is_empty() {
            return Err(VirtuosoError::Execution(
                "empty response from daemon".into(),
            ));
        }

        let status_byte = data[0];
        let payload = String::from_utf8_lossy(&data[1..]).to_string();

        let mut result = VirtuosoResult {
            status: ExecutionStatus::Success,
            output: String::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            execution_time: Some(elapsed),
            metadata: Default::default(),
        };

        if status_byte == STX {
            if payload.contains("TimeoutError") {
                result.status = ExecutionStatus::Error;
                result.errors.push(payload.clone());
            } else {
                result.output = payload;
            }
        } else if status_byte == NAK {
            result.status = ExecutionStatus::Error;
            result.errors.push(payload);
        } else {
            result.output = String::from_utf8_lossy(&data).to_string();
            result.warnings.push("non-standard response marker".into());
        }

        // Log command execution
        let truncated = if skill_code.len() > 200 {
            format!("{}...", &skill_code[..200])
        } else {
            skill_code.to_string()
        };
        crate::command_log::log_command("SKILL", &truncated, Some(start.elapsed().as_millis()));

        Ok(result)
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
        let cleaned = result.output.trim().trim_matches(|c| c == '(' || c == ')');
        let parts: Vec<&str> = cleaned.split_whitespace().collect();
        if parts.len() >= 3 {
            let strip = |s: &str| s.trim_matches('"').to_string();
            Ok((strip(parts[0]), strip(parts[1]), strip(parts[2])))
        } else {
            Err(VirtuosoError::Execution(
                "failed to get current design".into(),
            ))
        }
    }

    pub fn load_il(&self, local_path: &str) -> Result<VirtuosoResult> {
        let remote_path = format!("/tmp/virtuoso_bridge/{}", {
            std::path::Path::new(local_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        });

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

    pub fn ciw_print(&self, message: &str) -> Result<VirtuosoResult> {
        let skill = format!(
            r#"printf("[virtuoso-cli] {}\n")"#,
            escape_skill_string(message)
        );
        self.execute_skill(&skill, None)
    }

    pub fn run_shell_command(&self, cmd: &str) -> Result<VirtuosoResult> {
        let cmd = escape_skill_string(cmd);
        let skill = format!(r#"(csh "{cmd}")"#);
        self.execute_skill(&skill, None)
    }

    pub fn tunnel(&self) -> Option<&SSHClient> {
        self.tunnel.as_ref()
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

pub fn escape_skill_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

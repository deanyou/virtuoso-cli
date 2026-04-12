use crate::models::{SessionInfo, TunnelState};
use crate::spectre::jobs::Job;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum ActiveTab {
    Sessions,
    Jobs,
    Config,
}

/// A single .env config field
pub struct ConfigField {
    pub key: String,
    pub value: String,
    pub hint: &'static str,
}

pub struct TuiState {
    pub sessions: Vec<SessionInfo>,
    pub jobs: Vec<Job>,
    pub tunnel_state: Option<TunnelState>,
    pub selected_session: usize,
    pub selected_job: usize,
    pub active_tab: ActiveTab,
    pub status_msg: Option<(String, Instant)>,
    pub show_log: bool,
    pub log_lines: Vec<String>,
    pub log_scroll: usize,
    pub spinner_frame: usize,
    // Config tab
    pub config_fields: Vec<ConfigField>,
    pub config_selected: usize,
    pub config_editing: bool,
    pub config_edit_buf: String,
}

impl TuiState {
    pub fn new() -> Self {
        let sessions = SessionInfo::list().unwrap_or_default();
        let mut jobs = Job::list_all().unwrap_or_default();
        for j in &mut jobs {
            let _ = j.refresh();
        }
        let tunnel_state = TunnelState::load().ok().flatten();
        let config_fields = Self::load_config_fields();

        Self {
            sessions,
            jobs,
            tunnel_state,
            selected_session: 0,
            selected_job: 0,
            active_tab: ActiveTab::Sessions,
            status_msg: None,
            show_log: false,
            log_lines: Vec::new(),
            log_scroll: 0,
            spinner_frame: 0,
            config_fields,
            config_selected: 0,
            config_editing: false,
            config_edit_buf: String::new(),
        }
    }

    fn load_config_fields() -> Vec<ConfigField> {
        let fields_def: Vec<(&str, &str)> = vec![
            ("VB_REMOTE_HOST", "SSH remote hostname or alias"),
            ("VB_REMOTE_USER", "SSH login username"),
            ("VB_PORT", "Direct port (default: per-user hash)"),
            ("VB_TIMEOUT", "Timeout in seconds (default: 30)"),
            ("VB_JUMP_HOST", "Bastion/jump host address"),
            ("VB_JUMP_USER", "Jump host username"),
            ("VB_SSH_PORT", "SSH port (default: 22)"),
            (
                "VB_SSH_KEY",
                "SSH private key path (e.g. ~/.ssh/id_ed25519)",
            ),
            ("VB_SPECTRE_CMD", "Spectre binary path (default: spectre)"),
            ("VB_SPECTRE_ARGS", "Extra spectre arguments"),
            ("VB_KEEP_REMOTE_FILES", "Keep remote files (true/false)"),
            ("VB_PROFILE", "Config profile name"),
        ];

        // Read current .env file
        let env_path = std::env::current_dir().unwrap_or_default().join(".env");
        let env_content = std::fs::read_to_string(&env_path).unwrap_or_default();
        let mut env_map = std::collections::HashMap::new();
        for line in env_content.lines() {
            let line = line.trim();
            if line.starts_with('#') || !line.contains('=') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                env_map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        fields_def
            .into_iter()
            .map(|(key, hint)| {
                let value = env_map
                    .get(key)
                    .cloned()
                    .or_else(|| std::env::var(key).ok())
                    .unwrap_or_default();
                ConfigField {
                    key: key.to_string(),
                    value,
                    hint,
                }
            })
            .collect()
    }

    pub fn save_config(&self) -> std::io::Result<()> {
        let env_path = std::env::current_dir()?.join(".env");
        let mut lines = vec!["# Virtuoso CLI Configuration".to_string()];
        for f in &self.config_fields {
            if f.value.is_empty() {
                lines.push(format!("# {}=", f.key));
            } else {
                lines.push(format!("{}={}", f.key, f.value));
            }
        }
        lines.push(String::new());
        std::fs::write(env_path, lines.join("\n"))
    }

    pub fn refresh(&mut self) {
        self.sessions = SessionInfo::list().unwrap_or_default();
        let mut jobs = Job::list_all().unwrap_or_default();
        for j in &mut jobs {
            let _ = j.refresh();
        }
        self.jobs = jobs;
        self.tunnel_state = TunnelState::load().ok().flatten();
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_msg = Some((msg.to_string(), Instant::now()));
    }

    pub fn selected_session_info(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected_session)
    }
}

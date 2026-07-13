use crate::command_log;
use crate::models::{SessionInfo, TunnelState};
use crate::spectre::jobs::Job;
use crate::tui::app::state::{App, ConfigField};

pub(crate) fn initial_load(app: &mut App) {
    refresh(app);
    app.config_fields = load_config_fields();
}

pub(crate) fn refresh(app: &mut App) {
    app.sessions = SessionInfo::list().unwrap_or_default();
    let mut jobs = Job::list_all().unwrap_or_default();
    for j in &mut jobs {
        let _ = j.refresh();
    }
    app.jobs = jobs;
    app.tunnel_state = TunnelState::load().ok().flatten();
}

pub(crate) fn load_log_lines() -> Vec<String> {
    std::fs::read_to_string(command_log::log_path())
        .map(|c| c.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

pub(crate) fn save_config(app: &App) -> std::io::Result<()> {
    let env_path = std::env::current_dir()?.join(".env");
    let mut lines = vec!["# Virtuoso CLI Configuration".to_string()];
    for f in &app.config_fields {
        if f.value.is_empty() {
            lines.push(format!("# {}=", f.key));
        } else {
            lines.push(format!("{}={}", f.key, f.value));
        }
    }
    lines.push(String::new());
    std::fs::write(env_path, lines.join("\n"))
}

/// Load config fields from the current .env file, falling back to process env
/// vars. The field set is fixed — we don't introspect arbitrary VB_* values.
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

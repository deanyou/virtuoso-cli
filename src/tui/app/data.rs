use crate::command_log;
use crate::config_file::ConfigFile;
use crate::error::Result;
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

/// Persist the edited fields to `config.toml`.
///
/// Writes through [`ConfigFile`] — same type, same path the configuration
/// layer reads — so a saved value is actually in effect on the next run
/// rather than landing in a file nobody reads.
///
/// A field the user cleared is **removed** instead of being written as an
/// empty string, so the lookup falls through to the next layer instead of
/// shadowing it with a blank.
pub(crate) fn save_config(app: &App) -> Result<()> {
    let mut file = match ConfigFile::load() {
        Ok(Some(f)) => f,
        Ok(None) => ConfigFile::default(),
        // A file that exists but will not parse must not be silently
        // replaced: surfacing the error is the whole point of validating at
        // load time.
        Err(e) => return Err(e),
    };
    let profile = crate::profile::resolve_profile_info(None).profile;
    for f in &app.config_fields {
        if f.value.is_empty() {
            file.remove(profile.as_deref(), &f.key);
        } else {
            file.set(profile.as_deref(), &f.key, &f.value);
        }
    }
    let path = file.save()?;
    tracing::info!(
        "wrote {} fields to {}",
        app.config_fields.len(),
        path.display()
    );
    Ok(())
}

/// Which environment variable, if any, outranks the file for `key`.
///
/// Mirrors the top two layers of `config::Layers` so the UI can say "the value
/// you are looking at is not the one in effect" instead of silently showing a
/// dead field. `key` is the friendly file key (`remote_host`); the env var that
/// outranks it is the upper-case `VB_*` form.
fn env_override(key: &str, profile: Option<&str>) -> Option<(String, String)> {
    let env_name = crate::config_file::env_var_for(key);
    if let Some(p) = profile {
        let scoped = format!("{env_name}_{p}");
        if let Ok(v) = std::env::var(&scoped) {
            if !v.is_empty() {
                return Some((scoped, v));
            }
        }
    }
    std::env::var(&env_name)
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| (env_name.clone(), v))
}

/// Load the editable fields from `config.toml`.
///
/// The field set is fixed — we don't introspect arbitrary `VB_*` keys. Keys are
/// the **friendly** form (`remote_host`, not `VB_REMOTE_HOST`) so they match
/// exactly what `vcli init` writes and what the resolution layer maps the
/// env-var names to. `VB_PROFILE` is deliberately absent: it selects which
/// profile the rest of the keys come from, so persisting it here would create a
/// second, contradictory selector. Use `vcli profile bind --user` or export it.
fn load_config_fields() -> Vec<ConfigField> {
    let fields_def: Vec<(&str, &str)> = vec![
        ("remote_host", "SSH remote hostname or alias"),
        ("remote_user", "SSH login username"),
        ("port", "Direct port (default: per-user hash)"),
        ("timeout", "Timeout in seconds (default: 30)"),
        ("jump_host", "Bastion/jump host address"),
        ("jump_user", "Jump host username"),
        ("ssh_port", "SSH port (default: 22)"),
        ("ssh_key", "SSH private key path (e.g. ~/.ssh/id_ed25519)"),
        ("spectre_cmd", "Spectre binary path (default: spectre)"),
        ("spectre_args", "Extra spectre arguments"),
        ("keep_remote_files", "Keep remote files (true/false)"),
    ];

    let file = match ConfigFile::load() {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("{e}");
            None
        }
    };
    let profile = crate::profile::resolve_profile_info(None).profile;

    fields_def
        .into_iter()
        .map(|(key, hint)| {
            let value = file
                .as_ref()
                .and_then(|f| f.get(key, profile.as_deref()))
                .unwrap_or_default();
            ConfigField {
                key: key.to_string(),
                value,
                hint,
                env_override: env_override(key, profile.as_deref()),
            }
        })
        .collect()
}

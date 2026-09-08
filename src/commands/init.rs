use crate::config_file::{self, ConfigFile};
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

/// Written with every key commented out: an uncommented `remote_host = ""`
/// would set the value to an empty string rather than leave it unset, and the
/// user would have no way to tell "not configured" from "configured to blank".
const CONFIG_TEMPLATE: &str = r#"# Virtuoso CLI configuration
#
# Resolution order (highest wins):
#   1. <KEY>_<PROFILE> environment variable
#   2. <KEY> environment variable
#   3. [profile.<active>] section in this file
#   4. global section in this file
#   5. built-in default
#
# Uncomment a line to set it. A key left commented out is simply unset.

# SSH host to connect to (alias or hostname).
# remote_host = "eda-server"

# SSH login user (default: your local username).
# remote_user = ""

# Direct port (default: a stable per-user hash).
# port = 65432

# Command timeout in seconds (default: 30).
# timeout = 30

# Read timeout in seconds (default: 120).
# read_timeout = 120

# Bastion / jump host.
# jump_host = ""
# jump_user = ""

# SSH port (default: 22).
# ssh_port = 22

# SSH private key path.
# ssh_key = "~/.ssh/id_ed25519"

# Keep remote files after a tunnel stops (default: false).
# keep_remote_files = false

# Spectre binary (default: spectre) and extra arguments.
# spectre_cmd = "spectre"
# spectre_args = ""

# Per-profile overrides. Copy this block and rename it to match a profile
# selected with `vcli profile bind --user` or `export VB_PROFILE=<name>`.
# [profile.production]
# remote_host = "eda-prod"
"#;

pub fn run(if_not_exists: bool) -> Result<Value> {
    let path = config_file::path();

    if path.exists() {
        if if_not_exists {
            return Ok(json!({
                "status": "skipped",
                "reason": "config file already exists",
                "path": path,
            }));
        }
        return Err(VirtuosoError::Conflict(format!(
            "{} already exists (use --if-not-exists to skip)",
            path.display()
        )));
    }

    // Parse the template the same way a real config file is parsed: if the
    // shipped template is ever not valid TOML, `vcli init` fails here instead
    // of handing the user a file that breaks their next command.
    ConfigFile::parse(CONFIG_TEMPLATE, &path)?;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, CONFIG_TEMPLATE)?;

    Ok(json!({
        "status": "created",
        "path": path.display().to_string(),
        "next_step": format!(
            "Edit {} and uncomment remote_host, then run: vcli tunnel start",
            path.display()
        ),
    }))
}

//! Profile resolution system — hierarchical lookup of connection profiles.
//!
//! Resolution order (first match wins):
//! 1. Explicit `profile=` argument / CLI `-p/--profile`
//! 2. Process environment `VB_PROFILE`
//! 3. Virtualenv binding file (`$VIRTUAL_ENV/.vcli-profile`)
//! 4. User-level `~/.vcli/.env` `VB_PROFILE`
//! 5. `None` (legacy default behaviour)
//!
//! This mirrors virtuoso-bridge-lite's profile resolution ladder, adapted for vcli.

use std::path::PathBuf;
use std::{env, fs};

/// Profile binding filename inside a virtualenv.
const PROFILE_BINDING_FILENAME: &str = ".vcli-profile";

/// User-level config directory.
const USER_CONFIG_DIR: &str = ".vcli";

/// Result of profile resolution — includes the resolved profile and its source.
#[derive(Debug, Clone)]
pub struct ProfileResolution {
    /// The resolved profile name, or `None` for legacy default.
    pub profile: Option<String>,
    /// Where the profile came from: "explicit", "environment", "venv", "user_env", "default".
    pub source: &'static str,
    /// Path to the source file (for venv/user_env sources).
    pub path: Option<PathBuf>,
}

impl ProfileResolution {
    /// Get the profile name, defaulting to legacy behaviour.
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }
}

/// Clean a profile string: trim whitespace, return `None` if empty.
fn clean_profile(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Get the user-level config directory (`~/.vcli`).
fn user_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(USER_CONFIG_DIR)
}

/// Get the user-level .env file path (`~/.vcli/.env`).
fn user_env_path() -> PathBuf {
    user_config_dir().join(".env")
}

/// Get the profile binding file path for the active virtualenv.
/// Returns `None` if no virtualenv is active.
fn venv_profile_path() -> Option<PathBuf> {
    // Check VIRTUAL_ENV first
    if let Ok(venv) = env::var("VIRTUAL_ENV") {
        if !venv.is_empty() {
            return Some(PathBuf::from(&venv).join(PROFILE_BINDING_FILENAME));
        }
    }

    // Fallback: check for common venv patterns
    // If there's a .venv directory in cwd, assume that's the venv
    let cwd = std::env::current_dir().ok()?;
    let venv_marker = cwd.join(".venv");
    if venv_marker.exists() {
        return Some(cwd.join(PROFILE_BINDING_FILENAME));
    }

    None
}

/// Read the profile from a binding file.
fn read_profile_file(path: &PathBuf) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        // Skip comments
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(profile) = clean_profile(trimmed) {
            return Some(profile);
        }
    }
    None
}

/// Read the VB_PROFILE from a .env file using dotenv.
fn read_profile_from_env_file(path: &PathBuf) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Parse KEY=VALUE
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim() == "VB_PROFILE" {
                return clean_profile(value.trim());
            }
        }
    }
    None
}

/// Resolve the connection profile using the hierarchical resolution ladder.
///
/// This function implements the same resolution order as virtuoso-bridge-lite's
/// `resolve_profile`, adapted for vcli's environment.
pub fn resolve_profile(explicit: Option<&str>) -> Option<String> {
    resolve_profile_info(explicit).profile
}

/// Resolve the connection profile with full provenance information.
pub fn resolve_profile_info(explicit: Option<&str>) -> ProfileResolution {
    // 1. Explicit argument (CLI -p/--profile)
    if let Some(p) = explicit {
        if let Some(profile) = clean_profile(p) {
            return ProfileResolution {
                profile: Some(profile),
                source: "explicit",
                path: None,
            };
        }
    }

    // 2. Process environment VB_PROFILE
    if let Ok(v) = env::var("VB_PROFILE") {
        if let Some(profile) = clean_profile(&v) {
            return ProfileResolution {
                profile: Some(profile),
                source: "environment",
                path: None,
            };
        }
    }

    // 3. Runtime --env file (check for VCLI_ENV_PATH)
    if let Ok(env_path) = env::var("VCLI_ENV_PATH") {
        let path = PathBuf::from(&env_path);
        if let Some(profile) = read_profile_from_env_file(&path) {
            return ProfileResolution {
                profile: Some(profile),
                source: "runtime_env",
                path: Some(path),
            };
        }
    }

    // 4. Virtualenv binding file ($VIRTUAL_ENV/.vcli-profile)
    if let Some(venv_path) = venv_profile_path() {
        if let Some(profile) = read_profile_file(&venv_path) {
            return ProfileResolution {
                profile: Some(profile),
                source: "venv",
                path: Some(venv_path),
            };
        }
    }

    // 5. User-level ~/.vcli/.env VB_PROFILE
    let user_env = user_env_path();
    if let Some(profile) = read_profile_from_env_file(&user_env) {
        return ProfileResolution {
            profile: Some(profile),
            source: "user_env",
            path: Some(user_env),
        };
    }

    // 6. Default (legacy behaviour)
    ProfileResolution {
        profile: None,
        source: "default",
        path: None,
    }
}

/// Bind the active virtualenv to a connection profile.
///
/// Creates `$VIRTUAL_ENV/.vcli-profile` containing the profile name.
pub fn bind_venv_profile(profile: &str) -> std::io::Result<PathBuf> {
    let cleaned = clean_profile(profile).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "profile must be non-empty",
        )
    })?;

    let path = venv_profile_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No active virtualenv. Set VIRTUAL_ENV or run from an activated venv.",
        )
    })?;

    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, format!("{cleaned}\n"))?;
    Ok(path)
}

/// Clear the virtualenv profile binding.
pub fn clear_venv_profile() -> std::io::Result<PathBuf> {
    let path = venv_profile_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No active virtualenv. Set VIRTUAL_ENV or run from an activated venv.",
        )
    })?;

    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(path)
}

/// Read the current virtualenv profile binding.
pub fn read_venv_profile() -> (Option<PathBuf>, Option<String>) {
    let path = venv_profile_path();
    let profile = path.as_ref().and_then(read_profile_file);
    (path, profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_profile_empty() {
        assert!(clean_profile("").is_none());
        assert!(clean_profile("   ").is_none());
    }

    #[test]
    fn test_clean_profile_valid() {
        assert_eq!(
            clean_profile("t28_digital"),
            Some("t28_digital".to_string())
        );
        assert_eq!(clean_profile("  prod  "), Some("prod".to_string()));
    }

    #[test]
    fn test_clean_profile_comments_skipped() {
        // This is tested via read_profile_file
        let content = "# comment\nt28_digital\n";
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if let Some(profile) = clean_profile(trimmed) {
                assert_eq!(profile, "t28_digital");
                return;
            }
        }
        panic!("Expected to find profile");
    }
}

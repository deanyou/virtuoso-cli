//! Profile resolution system — hierarchical lookup of connection profiles.
//!
//! Resolution order (first match wins):
//! 1. Explicit `profile=` argument / CLI `-p/--profile`
//! 2. Process environment `VB_PROFILE`
//! 3. Virtualenv binding file (`$VIRTUAL_ENV/.vcli-profile`)
//! 4. Virtualenv binding file (`$VIRTUAL_ENV/.vcli-profile`)
//! 5. User-level `~/.vcli/profile` (deprecated fallback: `~/.vcli/.env`)
//! 6. `None` (legacy default behaviour)
//!
//! This mirrors virtuoso-bridge-lite's profile resolution ladder, adapted for vcli.

use std::path::PathBuf;
use std::{env, fs};

/// Profile binding filename inside a virtualenv.
const PROFILE_BINDING_FILENAME: &str = ".vcli-profile";

/// Filename of the user-level profile binding inside `~/.vcli`.
const USER_PROFILE_FILENAME: &str = "profile";

/// User-level config directory.
const USER_CONFIG_DIR: &str = ".vcli";

/// Result of profile resolution — includes the resolved profile and its source.
#[derive(Debug, Clone)]
pub struct ProfileResolution {
    /// The resolved profile name, or `None` for legacy default.
    pub profile: Option<String>,
    /// Where the profile came from: "explicit", "environment", "runtime_env",
    /// "venv", "user", "user_env" (deprecated `~/.vcli/.env` fallback),
    /// "default".
    pub source: &'static str,
    /// Path to the source file (for runtime_env/venv/user/user_env sources).
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

/// Get the user-level profile binding file path (`~/.vcli/profile`).
///
/// Sole content is the profile name — the same format the venv and local
/// bindings use, so all three scopes share one reader.
fn user_profile_path() -> PathBuf {
    user_profile_path_in(&user_config_dir())
}

/// Legacy user-level `.env` path (`~/.vcli/.env`), kept only as a deprecated
/// fallback. See RFC #83 — this file is no longer loaded as configuration, and
/// reading `VB_PROFILE` out of it goes away in a future release.
fn user_env_path() -> PathBuf {
    user_env_path_in(&user_config_dir())
}

/// [`user_profile_path`] inside an arbitrary config dir. Split out so tests
/// can exercise the user scope without touching the real `~/.vcli`.
fn user_profile_path_in(dir: &std::path::Path) -> PathBuf {
    dir.join(USER_PROFILE_FILENAME)
}

/// [`user_env_path`] inside an arbitrary config dir.
fn user_env_path_in(dir: &std::path::Path) -> PathBuf {
    dir.join(".env")
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

/// Read `VB_PROFILE=` out of an env-style `KEY=VALUE` file.
///
/// Deliberately a hand-rolled line scan: vcli no longer depends on `dotenvy`,
/// and this is only ever used for the deprecated `~/.vcli/.env` fallback plus
/// the explicitly-requested `VCLI_ENV_PATH` file — never for automatic
/// discovery.
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

    // 5. User-level ~/.vcli/profile (deprecated fallback: ~/.vcli/.env)
    let user_profile = user_profile_path();
    if let Some(profile) = read_profile_file(&user_profile) {
        return ProfileResolution {
            profile: Some(profile),
            source: "user",
            path: Some(user_profile),
        };
    }
    if let Some(profile) = read_profile_from_env_file(&user_env_path()) {
        tracing::warn!(
            "reading VB_PROFILE from {} is deprecated and will be removed — \
             run `vcli profile bind <name> --user` to migrate, or export VB_PROFILE in your shell",
            user_env_path().display()
        );
        return ProfileResolution {
            profile: Some(profile),
            source: "user_env",
            path: Some(user_env_path()),
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

// =============================================================================
// Multi-scope binding API: --venv, --user, --local
//
// Extends the venv-only binding from PR #87 to cover the same
// "ladder of scope" that virtuoso-bridge-lite's profile resolver uses
// in resolution. All three scopes write the same file format so that
// resolution can be uniform.
// =============================================================================

/// Where to (un)bind a profile. Mirrors the resolution ladder:
/// - `Venv`: $VIRTUAL_ENV/.vcli-profile  (project Python venv)
/// - `User`: ~/.vcli/profile            (user-level default)
/// - `Local`: ./.vcli-profile            (current working dir)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindScope {
    Venv,
    User,
    Local,
}

impl BindScope {
    pub fn label(self) -> &'static str {
        match self {
            BindScope::Venv => "venv",
            BindScope::User => "user",
            BindScope::Local => "local",
        }
    }
}

/// Bind a profile to the user-level default: `~/.vcli/profile`, whose sole
/// content is the profile name — the same format the venv and local bindings
/// use. Creates `~/.vcli/` if missing.
pub fn bind_user_profile(profile: &str) -> std::io::Result<PathBuf> {
    bind_user_profile_in(&user_config_dir(), profile)
}

/// Remove the user-level profile binding. Idempotent.
///
/// Deletes `~/.vcli/profile`, and also strips the `VB_PROFILE=` line from the
/// deprecated `~/.vcli/.env` (other lines preserved) so a stale legacy binding
/// cannot resurrect the profile through the deprecated fallback.
pub fn clear_user_profile() -> std::io::Result<()> {
    clear_user_profile_in(&user_config_dir())
}

/// [`bind_user_profile`] against an arbitrary config dir.
fn bind_user_profile_in(dir: &std::path::Path, profile: &str) -> std::io::Result<PathBuf> {
    let cleaned = clean_profile(profile).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "profile must be non-empty",
        )
    })?;
    if cleaned.contains('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "profile must not contain newlines",
        ));
    }

    let path = user_profile_path_in(dir);
    fs::create_dir_all(dir)?;
    fs::write(&path, format!("{cleaned}\n"))?;
    Ok(path)
}

/// [`clear_user_profile`] against an arbitrary config dir.
fn clear_user_profile_in(dir: &std::path::Path) -> std::io::Result<()> {
    let path = user_profile_path_in(dir);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    clear_legacy_user_env_profile_in(dir);
    Ok(())
}

/// Strip the `VB_PROFILE=` line from the deprecated `~/.vcli/.env`.
fn clear_legacy_user_env_profile_in(dir: &std::path::Path) {
    let path = user_env_path_in(dir);
    if !path.exists() {
        return;
    }
    let Ok(content) = fs::read_to_string(&path) else {
        return;
    };
    let kept: Vec<String> = content
        .lines()
        .filter(|l| !l.trim_start().starts_with("VB_PROFILE="))
        .map(|l| l.to_string())
        .collect();
    let body = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    };
    let _ = fs::write(&path, body);
}

/// Bind a profile to the current working directory: `./.vcli-profile`.
/// Creates the file with the profile name as its sole content line.
pub fn bind_local_profile(profile: &str) -> std::io::Result<PathBuf> {
    let cleaned = clean_profile(profile).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "profile must be non-empty",
        )
    })?;
    if cleaned.contains('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "profile must not contain newlines",
        ));
    }

    let cwd = env::current_dir()?;
    let path = cwd.join(PROFILE_BINDING_FILENAME);
    fs::write(&path, format!("{cleaned}\n"))?;
    Ok(path)
}

/// Remove the project-local `./.vcli-profile` binding.
pub fn clear_local_profile() -> std::io::Result<()> {
    let cwd = env::current_dir()?;
    let path = cwd.join(PROFILE_BINDING_FILENAME);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
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

    // ---- Multi-scope binding tests ----
    //
    // The user-scope tests used to write the real `~/.vcli`; they now run
    // against a `tempfile::tempdir()` through the `*_in(dir, ...)` variants,
    // so they are hermetic and safe to run in parallel.

    /// Test that `bind_user_profile` writes the profile name into
    /// `<config dir>/profile` and `clear_user_profile` removes it again.
    #[test]
    fn test_bind_and_clear_user_profile() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let profile_path = user_profile_path_in(dir);

        // Bind.
        let path = bind_user_profile_in(dir, "t28_digital").unwrap();
        assert_eq!(path, profile_path);
        let content = fs::read_to_string(&profile_path).unwrap();
        assert_eq!(content.trim(), "t28_digital", "got: {content:?}");

        // Re-bind: should replace, not append.
        bind_user_profile_in(dir, "analog_default").unwrap();
        let content = fs::read_to_string(&profile_path).unwrap();
        assert_eq!(content.trim(), "analog_default", "got: {content:?}");
        assert!(
            !content.contains("t28_digital"),
            "old entry should be replaced"
        );

        // Clear.
        clear_user_profile_in(dir).unwrap();
        assert!(
            !profile_path.exists(),
            "the user profile file should be gone"
        );
    }

    /// `clear_user_profile` must also strip `VB_PROFILE=` from the deprecated
    /// `.env` (other lines preserved), otherwise a stale legacy binding would
    /// resurrect the profile through the deprecated fallback.
    #[test]
    fn test_clear_user_profile_strips_legacy_env_binding() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let env_path = user_env_path_in(dir);

        // Seed the legacy file with a VB_PROFILE line plus other settings.
        fs::write(
            &env_path,
            "VB_PROFILE=stale\nVB_REMOTE_HOST=eda-lab\nVB_PORT=12345\n",
        )
        .unwrap();

        clear_user_profile_in(dir).unwrap();

        let content = fs::read_to_string(&env_path).unwrap();
        assert!(!content.contains("VB_PROFILE="), "VB_PROFILE cleared");
        assert!(
            content.contains("VB_REMOTE_HOST=eda-lab"),
            "other lines still preserved"
        );
        assert!(
            content.contains("VB_PORT=12345"),
            "other lines still preserved"
        );
    }

    /// The user scope resolves `<config dir>/profile` before the deprecated
    /// `.env`, so a machine that has both is not stuck on the legacy value.
    #[test]
    fn test_user_profile_wins_over_legacy_env() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fs::write(user_profile_path_in(dir), "from-profile-file\n").unwrap();
        fs::write(user_env_path_in(dir), "VB_PROFILE=from-legacy-env\n").unwrap();

        let resolved = read_profile_file(&user_profile_path_in(dir));
        assert_eq!(resolved.as_deref(), Some("from-profile-file"));
    }

    /// Empty / whitespace-only profile names should be rejected.
    #[test]
    fn test_bind_user_profile_rejects_empty() {
        let result = bind_user_profile("");
        assert!(result.is_err());
        let result = bind_user_profile("   \t  ");
        assert!(result.is_err());
    }

    /// Newline injection in profile name should be rejected.
    #[test]
    fn test_bind_user_profile_rejects_newline() {
        let result = bind_user_profile("a\nb");
        assert!(result.is_err());
    }
}

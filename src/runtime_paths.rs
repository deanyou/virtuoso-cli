//! Centralized path resolution for the bridge's runtime artifacts.
//!
//! Goals:
//!
//! 1. **Backward-compatible default** — keep `~/.cache/virtuoso_bridge/...`
//!    (and `~/.cache/virtuoso_bridge/logs/...`) on Linux, so existing
//!    installations are not moved silently.
//! 2. **Env overrides** — `VB_CACHE_DIR`, `VB_LOG_DIR`, `VB_HOME` let
//!    tests / multi-tenant setups redirect to a scratch location without
//!    touching the user's home.
//! 3. **XDG discipline** — when `XDG_CACHE_HOME` / `XDG_STATE_HOME` are
//!    set, honour them; otherwise fall back to `~/.cache` (Linux) /
//!    `~/Library/Caches` (macOS) / `%LOCALAPPDATA%` (Windows).
//! 4. **Legacy fallback** — `legacy_cache_file(name, profile)` reads the
//!    pre-refactor `~/.cache/virtuoso_bridge/<name>` paths so older
//!    state files survive a refactor.
//!
//! **Adopted from** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite)
//! `src/virtuoso_bridge/runtime_paths.py` (MIT, commit `6b9309d` 2026-06-05).

use std::path::{Path, PathBuf};

pub(crate) const APP_DIR: &str = "virtuoso_bridge";

/// Read an env var as a path. Returns `None` if unset / blank / not a valid
/// unicode string.
fn env_path(var: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(var)?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// If `home_var` is set, return `$home_var/<sub>` (no extra `virtuoso_bridge`).
fn env_path_under_home(home_var: &str, sub: &str) -> Option<PathBuf> {
    let root = env_path(home_var)?;
    Some(root.join(sub))
}

/// Cache root, honouring `VB_CACHE_DIR` → `VB_HOME/cache` → `XDG_CACHE_HOME` →
/// `dirs::cache_dir()` (`~/.cache` on Linux).
pub fn cache_root() -> PathBuf {
    if let Some(p) = env_path("VB_CACHE_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "cache") {
        return p;
    }
    if let Some(p) = env_path("XDG_CACHE_HOME") {
        return p;
    }
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Log root, honouring `VB_LOG_DIR` → `VB_HOME/logs` → `XDG_STATE_HOME/logs` →
/// `cache_dir()/logs` (Linux) / `~/Library/Logs/...` (macOS) /
/// `%LOCALAPPDATA%\<APP>\logs` (Windows).
pub fn log_root() -> PathBuf {
    if let Some(p) = env_path("VB_LOG_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "logs") {
        return p;
    }
    if let Some(p) = env_path_under_home("XDG_STATE_HOME", "logs") {
        return p;
    }
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()
            .map(|h| h.join("Library/Logs"))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    } else if cfg!(target_os = "windows") {
        env_path("LOCALAPPDATA")
            .or_else(|| env_path("APPDATA"))
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        // Linux: share state/log under cache so legacy `~/.cache/virtuoso_bridge/logs` still works.
        cache_root()
    };
    base.join(APP_DIR).join("logs")
}

/// Artifact (user-visible output) root, honouring `VB_OUTPUT_DIR` → `VB_HOME/artifacts` →
/// `XDG_STATE_HOME/artifacts` → `cache_root()/artifacts` (so artefacts live
/// inside the cache by default — same place the legacy code wrote screenshots
/// to via `Path("output")` when the user passed a relative path).
#[allow(dead_code)] // exposed for future use (digital-import, sim-output, etc.)
pub fn artifact_root() -> PathBuf {
    if let Some(p) = env_path("VB_OUTPUT_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "artifacts") {
        return p;
    }
    if let Some(p) = env_path_under_home("XDG_STATE_HOME", "artifacts") {
        return p;
    }
    cache_root().join(APP_DIR).join("artifacts")
}

/// Tmp root, honouring `VB_TMP_DIR` → `VB_HOME/tmp` → `TMPDIR`/`/tmp`.
#[allow(dead_code)] // exposed for future use (process sweeps, sim temp, etc.)
pub fn tmp_root() -> PathBuf {
    if let Some(p) = env_path("VB_TMP_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "tmp") {
        return p;
    }
    env_path("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Config root, honouring `VB_CONFIG_DIR` → `VB_HOME/config` → `XDG_CONFIG_HOME` →
/// `dirs::config_dir()` (`~/.config` on Linux). Used for plugin discovery.
pub fn config_root() -> PathBuf {
    if let Some(p) = env_path("VB_CONFIG_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "config") {
        return p;
    }
    if let Some(p) = env_path("XDG_CONFIG_HOME") {
        return p;
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Config path under the per-app subdir: `config_root()/vcli/<parts...>`.
pub fn config_subdir<P: AsRef<Path>>(parts: &[P]) -> PathBuf {
    let mut p = config_root().join("vcli");
    for part in parts {
        p = p.join(part);
    }
    p
}

/// State root, honouring `VB_STATE_DIR` → `VB_HOME/state` → `XDG_STATE_HOME` →
/// `cache_root()` (so legacy `~/.cache/virtuoso_bridge/state_*.json` keeps
/// working).
pub fn state_root() -> PathBuf {
    if let Some(p) = env_path("VB_STATE_DIR") {
        return p;
    }
    if let Some(p) = env_path_under_home("VB_HOME", "state") {
        return p;
    }
    if let Some(p) = env_path("XDG_STATE_HOME") {
        return p;
    }
    cache_root()
}

/// Cache path under the per-app subdir: `cache_root()/virtuoso_bridge/<parts...>`.
///
/// Equivalent to the previous `dirs::cache_dir().join("virtuoso_bridge")` +
/// arbitrary subpath call, but env-overridable.
pub fn cache_subdir<P: AsRef<Path>>(parts: &[P]) -> PathBuf {
    let mut p = cache_root().join(APP_DIR);
    for part in parts {
        p = p.join(part);
    }
    p
}

/// Log path under the per-app subdir: `log_root()/commands.log`.
pub fn command_log_file() -> PathBuf {
    log_root().join("commands.log")
}

/// Legacy path (pre-runtime_paths) for state files. Returns
/// `~/.cache/virtuoso_bridge/<name>` (or `<name>_<profile>.json`) — kept so
/// the refactor doesn't strand files written by older versions.
#[allow(dead_code)] // exposed for migration / future use
pub fn legacy_cache_file(name: &str, profile: Option<&str>) -> PathBuf {
    let mut p = cache_root().join(APP_DIR);
    if let Some(prof) = profile {
        p = p.join(format!("{name}_{prof}"));
    } else {
        p = p.join(name);
    }
    p
}

/// Legacy state file path. Mirrors `legacy_cache_file("state.json", profile)`
/// but with a JSON extension always.
pub fn legacy_state_file(profile: Option<&str>) -> PathBuf {
    let name = if let Some(p) = profile {
        format!("state_{p}.json")
    } else {
        "state.json".to_string()
    };
    cache_root().join(APP_DIR).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply a test-scoped env override, run `f`, then restore.
    fn with_env<F: FnOnce()>(var: &str, value: Option<&str>, f: F) {
        let original = std::env::var_os(var);
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        f();
        match original {
            Some(o) => std::env::set_var(var, o),
            None => std::env::remove_var(var),
        }
    }

    #[test]
    fn cache_subdir_includes_app_dir() {
        with_env("VB_CACHE_DIR", None, || {
            let p = cache_subdir(&["sessions"]);
            assert!(p.to_string_lossy().contains("virtuoso_bridge"));
            assert!(p.to_string_lossy().ends_with("sessions"));
        });
    }

    #[test]
    fn cache_subdir_respects_vb_cache_dir() {
        with_env("VB_CACHE_DIR", Some("/tmp/test-vb-cache-1"), || {
            let p = cache_subdir(&["sessions", "abc.json"]);
            assert_eq!(
                p,
                PathBuf::from("/tmp/test-vb-cache-1/virtuoso_bridge/sessions/abc.json")
            );
        });
    }

    #[test]
    fn cache_subdir_respects_vb_home() {
        with_env("VB_HOME", Some("/tmp/test-vb-home"), || {
            let p = cache_subdir(&["x", "y"]);
            assert_eq!(
                p,
                PathBuf::from("/tmp/test-vb-home/cache/virtuoso_bridge/x/y")
            );
        });
    }

    #[test]
    fn cache_subdir_vb_cache_dir_takes_priority_over_vb_home() {
        with_env("VB_HOME", Some("/tmp/test-vb-home"), || {
            with_env("VB_CACHE_DIR", Some("/tmp/test-vb-cache-2"), || {
                let p = cache_subdir(&["x"]);
                assert_eq!(p, PathBuf::from("/tmp/test-vb-cache-2/virtuoso_bridge/x"));
            });
        });
    }

    #[test]
    fn log_root_respects_vb_log_dir() {
        with_env("VB_LOG_DIR", Some("/tmp/test-vb-logs"), || {
            assert_eq!(log_root(), PathBuf::from("/tmp/test-vb-logs"));
        });
    }

    #[test]
    fn command_log_file_under_log_root() {
        with_env("VB_LOG_DIR", Some("/tmp/test-vb-logs2"), || {
            assert_eq!(
                command_log_file(),
                PathBuf::from("/tmp/test-vb-logs2/commands.log")
            );
        });
    }

    #[test]
    fn legacy_state_file_with_profile() {
        with_env("VB_CACHE_DIR", Some("/tmp/test-vb-legacy"), || {
            assert_eq!(
                legacy_state_file(Some("eda-meow-1")),
                PathBuf::from("/tmp/test-vb-legacy/virtuoso_bridge/state_eda-meow-1.json")
            );
        });
    }

    #[test]
    fn legacy_state_file_without_profile() {
        with_env("VB_CACHE_DIR", Some("/tmp/test-vb-legacy2"), || {
            assert_eq!(
                legacy_state_file(None),
                PathBuf::from("/tmp/test-vb-legacy2/virtuoso_bridge/state.json")
            );
        });
    }

    #[test]
    fn artifact_root_respects_vb_output_dir() {
        with_env("VB_OUTPUT_DIR", Some("/tmp/test-vb-out"), || {
            assert_eq!(artifact_root(), PathBuf::from("/tmp/test-vb-out"));
        });
    }

    #[test]
    fn tmp_root_respects_vb_tmp_dir() {
        with_env("VB_TMP_DIR", Some("/tmp/test-vb-tmp"), || {
            assert_eq!(tmp_root(), PathBuf::from("/tmp/test-vb-tmp"));
        });
    }

    #[test]
    fn state_root_respects_vb_state_dir() {
        with_env("VB_STATE_DIR", Some("/tmp/test-vb-state"), || {
            assert_eq!(state_root(), PathBuf::from("/tmp/test-vb-state"));
        });
    }

    #[test]
    fn env_var_with_blank_value_is_ignored() {
        with_env("VB_CACHE_DIR", Some(""), || {
            // Blank env var should not produce an empty path
            let p = cache_subdir(&["x"]);
            assert!(!p.to_string_lossy().is_empty());
        });
    }

    #[test]
    fn cache_subdir_nested_components() {
        with_env("VB_CACHE_DIR", Some("/tmp/test-nested"), || {
            let p = cache_subdir(&["a", "b", "c.json"]);
            assert_eq!(
                p,
                PathBuf::from("/tmp/test-nested/virtuoso_bridge/a/b/c.json")
            );
        });
    }
}

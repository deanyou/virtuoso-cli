//! Target selection and resolution.
//!
//! Owns the precedence rules that pick the effective configuration for one
//! invocation:
//!
//! 1. explicit `--target` and `--profile` are mutually exclusive (error);
//! 2. explicit `--target` wins over everything;
//! 3. explicit `--profile` beats an ambient `VB_TARGET`;
//! 4. a non-empty ambient `VB_TARGET` is treated as a target selection;
//! 5. otherwise the `active_target` from `targets.yaml`;
//! 6. otherwise fall back to legacy profile/env defaults.
//!
//! The resolver never starts a daemon and never reads tokens: it only turns a
//! name into an immutable [`Config`]. Connection lifecycle and identity
//! validation (config digest, nonce) live in the daemon layer.

use crate::config::Config;
use crate::error::VirtuosoError;
use crate::target::TargetManager;

/// A concrete selection and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Explicit `--target NAME`.
    CliTarget(String),
    /// Explicit `--profile NAME`.
    CliProfile(String),
    /// Ambient `VB_TARGET` env var.
    EnvTarget(String),
    /// `active_target` from `targets.yaml`.
    ActiveTarget(String),
    /// No explicit selection: legacy profile/env defaults.
    LegacyEnv,
}

/// The resolved, immutable configuration for one invocation.
// TODO(P0-A CommandContext): remove `allow(dead_code)` once commands receive
// the resolved Config explicitly instead of re-reading env.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Target name for target-backed selections; `None` for legacy paths.
    pub name: Option<String>,
    pub config: Config,
}

/// Decide which selection applies from explicit CLI flags plus ambient
/// configuration. Pure selection: never starts a daemon, never reads tokens,
/// and never resolves the selected target itself.
pub fn resolve_selection(
    cli_target: Option<&str>,
    cli_profile: Option<&str>,
) -> Result<Selection, VirtuosoError> {
    match (cli_target, cli_profile) {
        (Some(_), Some(_)) => Err(VirtuosoError::Config(
            "--target and --profile are mutually exclusive; pass exactly one".into(),
        )),
        (Some(name), None) => Ok(Selection::CliTarget(name.to_string())),
        (None, Some(profile)) => Ok(Selection::CliProfile(profile.to_string())),
        (None, None) => {
            if let Ok(value) = std::env::var("VB_TARGET") {
                if !value.trim().is_empty() {
                    return Ok(Selection::EnvTarget(value));
                }
            }
            // With no explicit flags we implicitly rely on the active target, so
            // a missing/corrupt targets.yaml is a real error here (the report's
            // rule: only report corrupt config on paths that actually need it —
            // explicit --profile never reaches this branch).
            let manager = TargetManager::load()
                .map_err(|e| VirtuosoError::Config(format!("failed to load targets: {e}")))?;
            // Distinguish "active_target unset" (fall back to legacy) from
            // "active_target set but its definition is missing" (error). The
            // latter is an inconsistent config and must not silently degrade.
            if let Some(name) = manager.active_target_raw() {
                if manager.get(name).is_none() {
                    return Err(VirtuosoError::Config(format!(
                        "active_target '{name}' is set but not defined in targets"
                    )));
                }
                return Ok(Selection::ActiveTarget(name.to_string()));
            }
            Ok(Selection::LegacyEnv)
        }
    }
}

/// Resolve a target name into its immutable configuration. Errors if the
/// target does not exist or its configuration is invalid; never silently
/// falls back.
// TODO(P0-A CommandContext): remove `allow(dead_code)` once commands receive
// the resolved Config explicitly instead of re-reading env.
#[allow(dead_code)]
pub fn resolve_target(name: &str) -> Result<ResolvedTarget, VirtuosoError> {
    let manager = TargetManager::load()
        .map_err(|e| VirtuosoError::Config(format!("failed to load targets: {e}")))?;
    let target = manager
        .get(name)
        .ok_or_else(|| VirtuosoError::NotFound(format!("target '{name}' not found")))?;
    let config = Config::from_target(target, name)?;
    Ok(ResolvedTarget {
        name: Some(name.to_string()),
        config,
    })
}

/// Resolve a concrete [`Selection`] into the effective, immutable config for
/// one invocation. This is the single resolution point: callers (main) build a
/// [`crate::context::CommandContext`] from the result and pass it to migrated
/// commands, which must not re-read env.
// TODO(P0-A CommandContext): remove `allow(dead_code)` once commands receive
// the resolved Config explicitly instead of re-reading env.
#[allow(dead_code)]
pub fn resolve_from_selection(selection: Selection) -> Result<ResolvedTarget, VirtuosoError> {
    match selection {
        Selection::CliTarget(name) | Selection::EnvTarget(name) | Selection::ActiveTarget(name) => {
            resolve_target(&name)
        }
        Selection::CliProfile(profile) => Ok(ResolvedTarget {
            name: None,
            config: Config::from_env_with_profile_no_target(Some(&profile))?,
        }),
        Selection::LegacyEnv => Ok(ResolvedTarget {
            name: None,
            config: Config::from_env()?,
        }),
    }
}

/// Resolve the effective configuration for an invocation from CLI flags and
/// ambient configuration.
// TODO(P0-A CommandContext): remove `allow(dead_code)` once commands receive
// the resolved Config explicitly instead of re-reading env.
#[allow(dead_code)]
pub fn resolve(
    cli_target: Option<&str>,
    cli_profile: Option<&str>,
) -> Result<ResolvedTarget, VirtuosoError> {
    resolve_from_selection(resolve_selection(cli_target, cli_profile)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn conflict_errors() {
        let err = resolve_selection(Some("prod"), Some("analog")).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn cli_target_wins() {
        assert_eq!(
            resolve_selection(Some("prod"), None).unwrap(),
            Selection::CliTarget("prod".to_string())
        );
    }

    #[test]
    fn cli_profile_wins() {
        assert_eq!(
            resolve_selection(None, Some("analog")).unwrap(),
            Selection::CliProfile("analog".to_string())
        );
    }

    #[serial]
    #[test]
    fn env_target_used_when_no_cli_flags() {
        std::env::set_var("VB_TARGET", "prod");
        let sel = resolve_selection(None, None).unwrap();
        std::env::remove_var("VB_TARGET");
        assert_eq!(sel, Selection::EnvTarget("prod".to_string()));
    }

    #[serial]
    #[test]
    fn cli_profile_overrides_env_target() {
        std::env::set_var("VB_TARGET", "prod");
        let sel = resolve_selection(None, Some("analog")).unwrap();
        std::env::remove_var("VB_TARGET");
        assert_eq!(sel, Selection::CliProfile("analog".to_string()));
    }

    /// Write a targets.yaml under the given (temp) home directory.
    fn write_targets(home: &std::path::Path, yaml: &str) {
        let vcli = home.join(".vcli");
        std::fs::create_dir_all(&vcli).unwrap();
        std::fs::write(vcli.join("targets.yaml"), yaml).unwrap();
    }

    /// Run `body` with HOME pointed at a temp dir (used to isolate
    /// `TargetManager::load()`), then restore the original HOME.
    fn with_home<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
        let original_home = std::env::var_os("HOME");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        let result = body(temp.path());
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    #[serial]
    #[test]
    fn active_target_used_when_no_cli_or_env() {
        std::env::remove_var("VB_TARGET");
        let sel = with_home(|home| {
            write_targets(
                home,
                "active_target: prod\ntargets:\n  prod:\n    remote_host: compute-eda-42\n",
            );
            resolve_selection(None, None).unwrap()
        });
        assert_eq!(sel, Selection::ActiveTarget("prod".to_string()));
    }

    #[serial]
    #[test]
    fn legacy_env_when_nothing_selected() {
        std::env::remove_var("VB_TARGET");
        let sel = with_home(|_home| resolve_selection(None, None).unwrap());
        assert_eq!(sel, Selection::LegacyEnv);
    }

    #[serial]
    #[test]
    fn corrupt_targets_errors_when_needed() {
        std::env::remove_var("VB_TARGET");
        let err = with_home(|home| {
            write_targets(home, "active_target: prod\ntargets: [not valid");
            resolve_selection(None, None).unwrap_err()
        });
        assert!(err.to_string().contains("failed to load targets"));
    }

    #[serial]
    #[test]
    fn resolve_target_builds_config() {
        let resolved = with_home(|home| {
            write_targets(
                home,
                "targets:\n  prod:\n    remote_host: compute-eda-42\n    port: 30001\n",
            );
            resolve_target("prod").unwrap()
        });
        assert_eq!(resolved.name.as_deref(), Some("prod"));
        assert_eq!(
            resolved.config.remote_host.as_deref(),
            Some("compute-eda-42")
        );
        assert_eq!(resolved.config.port, 30001);
    }

    #[serial]
    #[test]
    fn resolve_target_not_found_errors() {
        let err = with_home(|_home| resolve_target("nope").unwrap_err());
        assert!(err.to_string().contains("not found"));
    }

    #[serial]
    #[test]
    fn invalid_active_target_errors() {
        // active_target is set to a name whose definition was deleted: this is
        // an inconsistent config and must error, not silently fall back.
        std::env::remove_var("VB_TARGET");
        let err = with_home(|home| {
            write_targets(
                home,
                "active_target: prod\ntargets:\n  test:\n    remote_host: compute-eda-99\n",
            );
            resolve_selection(None, None).unwrap_err()
        });
        assert!(err
            .to_string()
            .contains("active_target 'prod' is set but not defined"));
    }

    #[serial]
    #[test]
    fn resolve_invalid_active_target_errors() {
        std::env::remove_var("VB_TARGET");
        let err = with_home(|home| {
            write_targets(
                home,
                "active_target: prod\ntargets:\n  test:\n    remote_host: compute-eda-99\n",
            );
            resolve(None, None).unwrap_err()
        });
        assert!(err
            .to_string()
            .contains("active_target 'prod' is set but not defined"));
    }

    #[serial]
    #[test]
    fn unset_active_target_does_not_implicitly_select_default() {
        // An unset active_target must fall back to legacy env even when a
        // target literally named "default" exists — no implicit default pick.
        std::env::remove_var("VB_TARGET");
        let sel = with_home(|home| {
            write_targets(
                home,
                "targets:\n  default:\n    remote_host: compute-eda-default\n",
            );
            resolve_selection(None, None).unwrap()
        });
        assert_eq!(sel, Selection::LegacyEnv);
    }

    #[serial]
    #[test]
    fn resolve_target_flow_uses_target_config() {
        let resolved = with_home(|home| {
            write_targets(
                home,
                "targets:\n  prod:\n    remote_host: compute-eda-42\n    port: 30001\n",
            );
            resolve(Some("prod"), None).unwrap()
        });
        assert_eq!(resolved.name.as_deref(), Some("prod"));
        assert_eq!(
            resolved.config.remote_host.as_deref(),
            Some("compute-eda-42")
        );
        assert_eq!(resolved.config.port, 30001);
    }

    #[serial]
    #[test]
    fn resolve_env_target_flow_uses_target_config() {
        std::env::set_var("VB_TARGET", "test");
        let resolved = with_home(|home| {
            write_targets(
                home,
                "targets:\n  test:\n    remote_host: compute-eda-99\n    port: 30002\n",
            );
            resolve(None, None).unwrap()
        });
        std::env::remove_var("VB_TARGET");
        assert_eq!(resolved.name.as_deref(), Some("test"));
        assert_eq!(
            resolved.config.remote_host.as_deref(),
            Some("compute-eda-99")
        );
        assert_eq!(resolved.config.port, 30002);
    }

    #[serial]
    #[test]
    fn resolve_active_target_flow_uses_target_config() {
        std::env::remove_var("VB_TARGET");
        let resolved = with_home(|home| {
            write_targets(
                home,
                "active_target: prod\ntargets:\n  prod:\n    remote_host: compute-eda-42\n    port: 30001\n",
            );
            resolve(None, None).unwrap()
        });
        assert_eq!(resolved.name.as_deref(), Some("prod"));
        assert_eq!(
            resolved.config.remote_host.as_deref(),
            Some("compute-eda-42")
        );
        assert_eq!(resolved.config.port, 30001);
    }

    #[serial]
    #[test]
    fn resolve_profile_flow_ignores_vb_target() {
        // Regression: resolve(None, Some(profile)) must NOT be hijacked by a
        // leftover ambient VB_TARGET. The old from_env_with_profile read
        // VB_TARGET first and would have resolved to the "test" target here.
        // Assert the final config comes from the profile/legacy path.
        std::env::set_var("VB_TARGET", "test");
        std::env::set_var("VB_REMOTE_HOST", "sentinel-host");
        std::env::set_var("VB_PORT", "65432");
        let resolved = with_home(|home| {
            write_targets(
                home,
                "targets:\n  test:\n    remote_host: compute-eda-99\n    port: 30002\n",
            );
            resolve(None, Some("analog")).unwrap()
        });
        std::env::remove_var("VB_TARGET");
        std::env::remove_var("VB_REMOTE_HOST");
        std::env::remove_var("VB_PORT");
        assert_eq!(resolved.name, None);
        assert_eq!(
            resolved.config.remote_host.as_deref(),
            Some("sentinel-host")
        );
        assert_eq!(resolved.config.port, 65432);
        assert_eq!(resolved.config.profile.as_deref(), Some("analog"));
    }
}

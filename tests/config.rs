//! Integration tests for Config parsing.
//!
//! Isolation contract: `Config::from_env_with_profile()` reads the process
//! environment and honours the ambient `VB_TARGET` bridge (which can jump the
//! parse into a target file). There is no `.env` lookup any more (RFC #83), so
//! the only way a value reaches the parser is an exported variable — a test
//! that must see a specific value sets that var explicitly first, using
//! [`EnvGuard`] so the original value is restored on drop (RAII).
//!
//! Every env-reading/writing test is `#[serial]`: mutating the process
//! environment is shared state, so these tests must never run concurrently
//! with each other, and each must shield every variable its assertion depends
//! on — the mechanism is shared, not per-test.

/// RAII guard that sets an env var for the scope of the test and restores the
/// original value (or removes it if it was absent) on drop.
struct EnvGuard {
    restored: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    /// Set `key` to `val` for the rest of this scope, restoring the prior
    /// value on drop.
    fn set(key: &str, val: &str) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, val);
        Self {
            restored: vec![(key.to_string(), old)],
        }
    }

    /// Shield a variable from the ambient process env: `Config` treats an
    /// empty value as absent, so the parser falls back to its default. The
    /// original value is restored on drop.
    fn shield(key: &str) -> Self {
        Self::set(key, "")
    }

    fn restore(&mut self) {
        for (key, val) in self.restored.drain(..) {
            match val {
                Some(v) => std::env::set_var(&key, v),
                None => std::env::remove_var(&key),
            }
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Shield `VB_TARGET` so `from_env_with_profile` cannot jump the parse into a
/// target file. Used by every test below.
fn shield_target() -> EnvGuard {
    EnvGuard::shield("VB_TARGET")
}

/// Isolate the configuration file source: point `VB_CONFIG_DIR` at a fresh temp
/// dir that contains no `config.toml`, so `ConfigFile::load` returns `None` and
/// no user configuration (e.g. a real `~/.vcli/config.toml` carrying
/// `timeout = 60`) can leak into a default-value assertion. Returns the temp dir
/// (kept alive for the test's duration) and the env guard.
fn isolate_config_dir() -> (tempfile::TempDir, EnvGuard) {
    let dir = tempfile::tempdir().unwrap();
    let guard = EnvGuard::set("VB_CONFIG_DIR", dir.path().to_str().unwrap());
    (dir, guard)
}

/// Test that Config can be created without panicking.
#[serial_test::serial]
#[test]
fn test_config_from_env_works() {
    let _g = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    let result = virtuoso_cli::config::Config::from_env_with_profile(None);
    assert!(result.is_ok());
    // Should have a valid config with reasonable defaults
    let config = result.unwrap();
    assert!(config.port > 0);
    assert!(config.timeout > 0);
}

/// Test that spectre_max_workers has a reasonable default, verified in
/// isolation (no ambient `VB_SPECTRE_MAX_WORKERS`, no target file, no user
/// config file).
#[serial_test::serial]
#[test]
fn test_config_spectre_max_workers_default() {
    let _g = EnvGuard::shield("VB_SPECTRE_MAX_WORKERS");
    let _t = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    // Should be 8 by default
    assert_eq!(config.spectre_max_workers, 8);
}

/// The default timeout is 30, verified in isolation.
///
/// `VB_TIMEOUT` and `VB_TARGET` are shielded, and the config file source is
/// redirected to an empty temp dir, so neither an exported timeout nor a user's
/// real `~/.vcli/config.toml` can leak in. The prior values are restored on
/// drop.
#[serial_test::serial]
#[test]
fn test_config_timeout_default_isolated() {
    let _g = EnvGuard::shield("VB_TIMEOUT");
    let _t = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.timeout, 30);
}

/// An explicit ambient `VB_TIMEOUT` overrides the default. `45` differs from
/// the default (30) and from the value this machine used to keep in `~/.env`,
/// so a pass proves the process env var wins. `VB_TARGET` is shielded and the
/// config file source is isolated so the parse stays on the legacy path.
#[serial_test::serial]
#[test]
fn test_config_timeout_env_override_wins() {
    let _g = EnvGuard::set("VB_TIMEOUT", "45");
    let _t = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.timeout, 45);
}

/// P1 regression: a config file written with the friendly key that `vcli init`
/// and the TUI produce (`remote_host`) must be read by the resolution layer,
/// which passes the env-var name (`VB_REMOTE_HOST`).
#[serial_test::serial]
#[test]
fn friendly_file_key_resolves_into_config() {
    let _t = shield_target();
    let _rg = EnvGuard::shield("VB_REMOTE_HOST");
    let (_dir, _cg) = isolate_config_dir();
    let path = virtuoso_cli::config_file::path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "remote_host = \"eda-server\"\n").unwrap();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.remote_host.as_deref(), Some("eda-server"));
}

/// P1 regression: a value the TUI persists through `ConfigFile` (friendly key)
/// lands in `config.toml` and is picked up by the next resolution.
#[serial_test::serial]
#[test]
fn tui_save_round_trips_through_config() {
    let _t = shield_target();
    let _rg = EnvGuard::shield("VB_REMOTE_HOST");
    let (_dir, _cg) = isolate_config_dir();
    let mut f = virtuoso_cli::config_file::ConfigFile::default();
    f.set(None, "remote_host", "from-tui");
    f.save().unwrap();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert_eq!(config.remote_host.as_deref(), Some("from-tui"));
}

/// `port_explicit` is part of the connection identity: the same numeric port
/// with "allow auto-discovery" vs "force port matching" must produce
/// different digests, otherwise `tunnel status` drift detection / daemon Hello
/// cannot tell the two constraints apart. Pure struct literal — no env, no
/// filesystem — so it needs no serialisation or guards.
#[test]
fn test_config_digest_distinguishes_port_explicit() {
    let base = virtuoso_cli::config::Config {
        profile: None,
        remote_host: Some("compute-eda-42".into()),
        remote_user: None,
        port: 30001,
        port_explicit: false,
        jump_host: None,
        jump_user: None,
        ssh_port: None,
        ssh_key: None,
        ssh_config: None,
        ssh_backend: None,
        disable_control_master: false,
        timeout: 30,
        read_timeout: 120,
        keep_remote_files: false,
        spectre_cmd: "spectre".into(),
        spectre_args: vec![],
        spectre_max_workers: 8,
        ssh_max_sessions: 10,
        ssh_max_bulk_sessions: 2,
        ssh_reconnect_max_attempts: 8,
        ssh_reconnect_max_delay: 30,
        ssh_keepalive_interval: 30,
        ssh_keepalive_failures: 3,
        transport_shutdown_grace: 5,
        cadence_cshrc: None,
        spectre_bin: None,
        roles: Default::default(),
        transport_daemon_socket: None,
        transport_daemon_token: None,
        allow_cross_user_daemon: false,
    };
    let mut explicit = base.clone();
    explicit.port_explicit = true;
    assert_ne!(
        base.digest(),
        explicit.digest(),
        "port constraint mode must change the config digest"
    );
    assert_eq!(
        base.digest(),
        base.clone().digest(),
        "digest must be deterministic for identical configs"
    );
}

/// `VB_ALLOW_CROSS_USER_DAEMON` must resolve through the same layered lookup
/// as every other field, and must stay `false` when nothing sets it.
#[serial_test::serial]
#[test]
fn test_allow_cross_user_daemon_defaults_to_false() {
    let _g = EnvGuard::shield("VB_ALLOW_CROSS_USER_DAEMON");
    let _t = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
    assert!(
        !config.allow_cross_user_daemon,
        "the cross-user warning must stay enabled unless explicitly suppressed"
    );
}

/// The spelling set that shipped with this field (`1` / `true` / `yes` / `on`,
/// case-insensitive) must keep working, and every other value must fall back to
/// the conservative `false` rather than erroring.
#[serial_test::serial]
#[test]
fn test_allow_cross_user_daemon_env_spellings() {
    let _t = shield_target();
    let (_dir, _cg) = isolate_config_dir();
    for spelling in ["1", "true", "TRUE", "yes", "on", " On "] {
        let _g = EnvGuard::set("VB_ALLOW_CROSS_USER_DAEMON", spelling);
        let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
        assert!(
            config.allow_cross_user_daemon,
            "{spelling:?} must enable the suppression"
        );
    }
    for spelling in ["0", "false", "off", "no", "maybe", ""] {
        let _g = EnvGuard::set("VB_ALLOW_CROSS_USER_DAEMON", spelling);
        let config = virtuoso_cli::config::Config::from_env_with_profile(None).unwrap();
        assert!(
            !config.allow_cross_user_daemon,
            "{spelling:?} must leave the warning enabled"
        );
    }
}

/// A target's `allow_cross_user_daemon` must reach the resolved `Config`.
#[serial_test::serial]
#[test]
fn test_allow_cross_user_daemon_from_target() {
    use virtuoso_cli::target::TargetConfig;

    let on = TargetConfig {
        allow_cross_user_daemon: Some(true),
        ..Default::default()
    };
    let cfg = virtuoso_cli::config::Config::from_target(&on, "t-on").unwrap();
    assert!(cfg.allow_cross_user_daemon);

    let unset = TargetConfig::default();
    let cfg = virtuoso_cli::config::Config::from_target(&unset, "t-off").unwrap();
    assert!(!cfg.allow_cross_user_daemon);
}

/// `digest()` is the config identity used for tunnel drift detection and daemon
/// Hello validation. This field only silences an informational warning, so it is
/// deliberately NOT part of that identity: flipping it must not invalidate an
/// existing connection.
#[serial_test::serial]
#[test]
fn test_digest_ignores_allow_cross_user_daemon() {
    use virtuoso_cli::target::TargetConfig;

    let base = virtuoso_cli::config::Config::from_target(&TargetConfig::default(), "t").unwrap();
    let mut silenced = base.clone();
    silenced.allow_cross_user_daemon = true;
    assert_eq!(
        base.digest(),
        silenced.digest(),
        "warning suppression must not change the config identity"
    );
}

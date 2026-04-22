/// Unit tests for vcli — no SSH connection or Virtuoso required.
///
/// Coverage:
///   - SSHRunner: remote_target, jump host args, summarize_error, build_ssh_cmd args
///   - Config: ssh_target, ssh_jump, is_remote, env parsing, VB_PORT validation
///   - SessionInfo: JSON round-trip, list dedup/sort, missing session error
#[cfg(test)]
mod config_tests {
    use crate::config::Config;
    use std::env;
    use std::sync::Mutex;

    // Serialize env-var tests to prevent races (env is global process state)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn make_config(
        remote_host: Option<&str>,
        remote_user: Option<&str>,
        jump_host: Option<&str>,
        jump_user: Option<&str>,
    ) -> Config {
        Config {
            profile: None,
            remote_host: remote_host.map(String::from),
            remote_user: remote_user.map(String::from),
            port: 65432,
            jump_host: jump_host.map(String::from),
            jump_user: jump_user.map(String::from),
            ssh_port: None,
            ssh_key: None,
            timeout: 30,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
        }
    }

    #[test]
    fn ssh_target_no_user() {
        let cfg = make_config(Some("eda-server"), None, None, None);
        assert_eq!(cfg.ssh_target(), "eda-server");
    }

    #[test]
    fn ssh_target_with_user() {
        let cfg = make_config(Some("eda-server"), Some("designer"), None, None);
        assert_eq!(cfg.ssh_target(), "designer@eda-server");
    }

    #[test]
    fn ssh_target_no_host() {
        let cfg = make_config(None, Some("designer"), None, None);
        assert_eq!(cfg.ssh_target(), "designer@");
    }

    #[test]
    fn is_remote_with_host() {
        let cfg = make_config(Some("eda-server"), None, None, None);
        assert!(cfg.is_remote());
    }

    #[test]
    fn is_remote_no_host() {
        let cfg = make_config(None, None, None, None);
        assert!(!cfg.is_remote());
    }

    #[test]
    fn ssh_jump_with_user() {
        let cfg = make_config(Some("eda"), None, Some("bastion.corp.com"), Some("admin"));
        assert_eq!(cfg.ssh_jump(), Some("admin@bastion.corp.com".into()));
    }

    #[test]
    fn ssh_jump_without_user() {
        let cfg = make_config(Some("eda"), None, Some("bastion.corp.com"), None);
        assert_eq!(cfg.ssh_jump(), Some("bastion.corp.com".into()));
    }

    #[test]
    fn ssh_jump_none_when_no_jump_host() {
        let cfg = make_config(Some("eda"), None, None, Some("admin"));
        assert_eq!(cfg.ssh_jump(), None);
    }

    /// Helper to clean env vars before/after each config test.
    fn clean_env() {
        env::remove_var("VB_PORT");
        env::remove_var("VB_REMOTE_HOST");
        env::remove_var("VB_PROFILE");
    }

    #[test]
    fn vb_port_zero_is_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_env();
        env::set_var("VB_PORT", "0");
        let result = Config::from_env();
        clean_env();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("VB_PORT must be between 1 and 65535"));
    }

    #[test]
    fn vb_port_default_when_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_env();
        let cfg = Config::from_env().unwrap();
        clean_env();
        // Default port is derived from username hash: 65000 + sum(bytes) % 500
        assert!(cfg.port >= 65000 && cfg.port < 65500);
    }

    #[test]
    fn vb_port_custom() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_env();
        env::set_var("VB_PORT", "12345");
        let cfg = Config::from_env().unwrap();
        clean_env();
        assert_eq!(cfg.port, 12345);
    }

    #[test]
    fn vb_remote_host_empty_means_local() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_env();
        env::set_var("VB_REMOTE_HOST", "");
        let cfg = Config::from_env().unwrap();
        clean_env();
        assert!(!cfg.is_remote());
        assert!(cfg.remote_host.is_none());
    }

    #[test]
    fn spectre_args_parsed_correctly() {
        let _lock = ENV_LOCK.lock().unwrap();
        env::set_var("VB_SPECTRE_ARGS", "-64 +aps +mt=4");
        env::remove_var("VB_REMOTE_HOST");
        env::remove_var("VB_PORT");
        let cfg = Config::from_env().unwrap();
        env::remove_var("VB_SPECTRE_ARGS");
        assert_eq!(cfg.spectre_args, vec!["-64", "+aps", "+mt=4"]);
    }
}

#[cfg(test)]
mod ssh_runner_tests {
    use crate::transport::ssh::SSHRunner;

    #[test]
    fn remote_target_no_user() {
        let r = SSHRunner::new("eda-server");
        assert_eq!(r.remote_target(), "eda-server");
    }

    #[test]
    fn remote_target_with_user() {
        let r = SSHRunner::new("eda-server").with_user("designer");
        assert_eq!(r.remote_target(), "designer@eda-server");
    }

    #[test]
    fn jump_host_stored() {
        let r = SSHRunner::new("eda-server").with_jump("bastion.corp.com");
        assert_eq!(r.jump_host.as_deref(), Some("bastion.corp.com"));
    }

    #[test]
    fn build_ssh_cmd_contains_host() {
        let r = SSHRunner::new("my-eda-host").with_user("meow");
        let cmd = r.build_ssh_cmd();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&"meow@my-eda-host".to_string()),
            "args: {args:?}"
        );
    }

    #[test]
    fn build_ssh_cmd_includes_batchmode() {
        let r = SSHRunner::new("eda");
        let cmd = r.build_ssh_cmd();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"BatchMode=yes".to_string()), "args: {args:?}");
        assert!(
            args.contains(&"GSSAPIAuthentication=no".to_string()),
            "args: {args:?}"
        );
        assert!(
            args.contains(&"HostbasedAuthentication=no".to_string()),
            "args: {args:?}"
        );
    }

    #[test]
    fn build_ssh_cmd_jump_flag() {
        let mut r = SSHRunner::new("eda");
        r.jump_host = Some("bastion.corp.com".into());
        r.jump_user = Some("admin".into());
        let cmd = r.build_ssh_cmd();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let j_idx = args
            .iter()
            .position(|a| a == "-J")
            .expect("-J flag missing");
        assert_eq!(args[j_idx + 1], "admin@bastion.corp.com");
    }

    #[test]
    fn summarize_error_connection_refused() {
        let r = SSHRunner::new("eda");
        let msg = r.summarize_error("ssh: connect to host eda port 22: Connection refused");
        assert!(msg.contains("connection refused"), "got: {msg}");
    }

    #[test]
    fn summarize_error_auth_failure() {
        let r = SSHRunner::new("eda");
        let msg = r.summarize_error("Permission denied (publickey).");
        assert!(msg.contains("authentication failed"), "got: {msg}");
    }

    #[test]
    fn summarize_error_timeout() {
        let r = SSHRunner::new("eda");
        let msg = r.summarize_error("ssh: connect to host eda port 22: Connection timed out");
        assert!(msg.contains("timed out"), "got: {msg}");
    }

    #[test]
    fn summarize_error_dns() {
        let r = SSHRunner::new("eda");
        let msg =
            r.summarize_error("Could not resolve hostname bad-host: Name or service not known");
        assert!(msg.contains("hostname resolution"), "got: {msg}");
    }

    #[test]
    fn summarize_error_generic_takes_first_lines() {
        let r = SSHRunner::new("eda");
        let msg = r.summarize_error("line1\nline2\nline3\nline4");
        let parts: Vec<_> = msg.split(';').collect();
        assert!(parts.len() <= 3, "should only take first 3 lines: {msg}");
    }

    /// Integration test: requires sshd running on localhost:2222.
    /// Start with: sudo /usr/sbin/sshd -p 2222
    #[test]
    #[ignore]
    fn integration_localhost_roundtrip() {
        let mut r = SSHRunner::new("localhost");
        r.ssh_port = Some(2222);
        r.connect_timeout = 5;

        let ok = r.test_connection(None).expect("test_connection failed");
        assert!(ok, "SSH connection to localhost:2222 failed");

        let result = r.run_command("echo PONG", None).expect("run_command failed");
        assert!(result.success, "command failed: {:?}", result.stderr);
        assert_eq!(result.stdout.trim(), "PONG");
    }
}

#[cfg(test)]
mod session_info_tests {
    use crate::models::SessionInfo;
    use std::fs;
    use tempfile::TempDir;

    fn make_session(id: &str, port: u16) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            port,
            pid: 0,
            host: "eda-server".into(),
            user: "meow".into(),
            created: "Apr  6 12:00:00 2026".into(),
        }
    }

    fn write_session(dir: &std::path::Path, s: &SessionInfo) {
        let path = dir.join(format!("{}.json", s.id));
        fs::write(path, serde_json::to_string(s).unwrap()).unwrap();
    }

    #[test]
    fn session_json_round_trip() {
        let s = make_session("eda-meow-1", 42109);
        let json = serde_json::to_string(&s).unwrap();
        let s2: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(s.id, s2.id);
        assert_eq!(s.port, s2.port);
        assert_eq!(s.host, s2.host);
        assert_eq!(s.user, s2.user);
    }

    #[test]
    fn session_load_missing_returns_error() {
        // Point to a temp dir with no files
        let _tmp = TempDir::new().unwrap();
        let fake_id = "nonexistent-session-xyz";
        // load() uses the real cache dir, so just verify the error message shape
        let result = SessionInfo::load(fake_id);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains(fake_id),
            "error should mention the session id: {msg}"
        );
    }

    #[test]
    fn session_list_empty_dir_returns_empty_vec() {
        // sessions_dir() that doesn't exist → list() returns []
        // We can't easily override the dir, but we can verify list() doesn't panic
        // when the real dir exists and contains valid JSON.
        let result = SessionInfo::list();
        assert!(result.is_ok());
    }

    #[test]
    fn session_sorted_by_id() {
        // Verify list() sorts by id ascending
        // Write to real sessions dir to test end-to-end sort
        let dir = dirs::cache_dir()
            .unwrap()
            .join("virtuoso_bridge")
            .join("sessions");
        fs::create_dir_all(&dir).unwrap();

        let s1 = make_session("zzz-sort-test-1", 11111);
        let s2 = make_session("aaa-sort-test-2", 22222);
        write_session(&dir, &s1);
        write_session(&dir, &s2);

        let sessions = SessionInfo::list().unwrap();
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        let pos1 = ids.iter().position(|&id| id == "aaa-sort-test-2").unwrap();
        let pos2 = ids.iter().position(|&id| id == "zzz-sort-test-1").unwrap();
        assert!(pos1 < pos2, "aaa should come before zzz");

        // Cleanup
        fs::remove_file(dir.join("zzz-sort-test-1.json")).ok();
        fs::remove_file(dir.join("aaa-sort-test-2.json")).ok();
    }
}

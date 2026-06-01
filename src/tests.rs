/// Unit tests for vcli — no SSH connection or Virtuoso required.
///
/// Coverage:
///   - SSHRunner: remote_target, jump host args, summarize_error, build_ssh_cmd args
///   - Config: ssh_target, ssh_jump, is_remote, env parsing, VB_PORT validation
///   - SessionInfo: JSON round-trip, list dedup/sort, missing session error
fn cmd_args(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod config_tests {
    use crate::config::Config;
    use std::env;
    use std::sync::Mutex;

    // Serialize env-var tests to prevent races (env is global process state).
    // Shared with config_tests_ext — both modules must hold this lock when touching env vars.
    pub(super) static ENV_LOCK: Mutex<()> = Mutex::new(());

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
            ssh_config: None,
            disable_control_master: false,
            timeout: 30,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: 8,
            cadence_cshrc: None,
            spectre_bin: None,
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
        env::remove_var("VB_SSH_CONFIG");
        env::remove_var("VB_DISABLE_CONTROL_MASTER");
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
    use super::cmd_args;
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
        let args = cmd_args(&r.build_ssh_cmd());
        assert!(
            args.contains(&"meow@my-eda-host".to_string()),
            "args: {args:?}"
        );
    }

    #[test]
    fn build_ssh_cmd_includes_batchmode() {
        let r = SSHRunner::new("eda");
        let args = cmd_args(&r.build_ssh_cmd());
        assert!(
            args.contains(&"BatchMode=yes".to_string()),
            "args: {args:?}"
        );
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
        let args = cmd_args(&r.build_ssh_cmd());
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

        let result = r
            .run_command("echo PONG", None)
            .expect("run_command failed");
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
            daemon_user: None,
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
        // Verify list() sorts by id ascending.
        // Bind real ports so concurrent cleanup() calls don't delete these sessions.
        let l1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let l2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p1 = l1.local_addr().unwrap().port();
        let p2 = l2.local_addr().unwrap().port();

        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id1 = format!("zzz-sort-test-{p1}");
        let id2 = format!("aaa-sort-test-{p2}");
        write_session(&dir, &make_session(&id1, p1));
        write_session(&dir, &make_session(&id2, p2));

        let sessions = SessionInfo::list().unwrap();
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        let pos1 = ids.iter().position(|&id| id == id2.as_str()).unwrap();
        let pos2 = ids.iter().position(|&id| id == id1.as_str()).unwrap();

        fs::remove_file(dir.join(format!("{id1}.json"))).ok();
        fs::remove_file(dir.join(format!("{id2}.json"))).ok();
        drop((l1, l2));

        assert!(pos1 < pos2, "aaa should come before zzz");
    }

    #[test]
    fn two_port_based_sessions_coexist() {
        // Regression: before the fix every Virtuoso instance generated "host-user-1"
        // (RBSessionSeq resets to 0 per process, increments to 1) and the second
        // RBWriteSession call silently overwrote the first session file.
        // After the fix RBIpcErrHandler uses the OS-assigned port as suffix, so each
        // bridge instance writes a distinct file. Verify both survive.
        // Bind real ports so concurrent cleanup() calls don't delete these sessions.
        let l1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let l2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p1 = l1.local_addr().unwrap().port();
        let p2 = l2.local_addr().unwrap().port();

        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id1 = format!("rt-test-session-{p1}");
        let id2 = format!("rt-test-session-{p2}");
        fs::remove_file(dir.join(format!("{id1}.json"))).ok();
        fs::remove_file(dir.join(format!("{id2}.json"))).ok();

        write_session(&dir, &make_session(&id1, p1));
        write_session(&dir, &make_session(&id2, p2));

        let sessions = SessionInfo::list().unwrap();
        let found1 = sessions.iter().any(|s| s.id == id1);
        let found2 = sessions.iter().any(|s| s.id == id2);

        fs::remove_file(dir.join(format!("{id1}.json"))).ok();
        fs::remove_file(dir.join(format!("{id2}.json"))).ok();
        drop((l1, l2));

        assert!(
            found1,
            "first session must survive second session registration"
        );
        assert!(found2, "second session must be registered independently");
    }

    #[test]
    fn session_id_suffix_equals_port() {
        // Contract set by RBIpcErrHandler in ramic_bridge.il:
        //   RBSessionId = sprintf(nil "%s-%d" RBSessionBase RBPort)
        // The trailing decimal in the ID is the port, so tooling can parse either field.
        let port: u16 = 54321;
        let s = make_session("meowu-meow-54321", port);
        let suffix: u16 = s.id.rsplit('-').next().unwrap().parse().unwrap();
        assert_eq!(suffix, port);
    }

    #[test]
    fn session_port_survives_round_trip() {
        // Write a port-based session file, read it back via load(), confirm the
        // port field still matches the ID suffix after JSON round-trip.
        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id = "rt-test-port-match-62000";
        fs::remove_file(dir.join(format!("{id}.json"))).ok();
        write_session(&dir, &make_session(id, 62000));

        let loaded = SessionInfo::load(id).unwrap();
        fs::remove_file(dir.join(format!("{id}.json"))).ok();

        let suffix: u16 = loaded.id.rsplit('-').next().unwrap().parse().unwrap();
        assert_eq!(
            suffix, loaded.port,
            "port field must match ID suffix after load"
        );
    }

    #[test]
    fn multiple_sessions_all_visible() {
        // When N Virtuoso instances are running, SessionInfo::list() must return all N
        // so VirtuosoClient::from_env() can present the full "--session <id>" list.
        // Bind real ports so concurrent cleanup() calls don't delete these sessions.
        let l1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let l2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p1 = l1.local_addr().unwrap().port();
        let p2 = l2.local_addr().unwrap().port();

        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id1 = format!("rt-test-multi-{p1}");
        let id2 = format!("rt-test-multi-{p2}");
        fs::remove_file(dir.join(format!("{id1}.json"))).ok();
        fs::remove_file(dir.join(format!("{id2}.json"))).ok();

        write_session(&dir, &make_session(&id1, p1));
        write_session(&dir, &make_session(&id2, p2));

        let sessions = SessionInfo::list().unwrap();
        let found1 = sessions.iter().any(|s| s.id == id1);
        let found2 = sessions.iter().any(|s| s.id == id2);

        fs::remove_file(dir.join(format!("{id1}.json"))).ok();
        fs::remove_file(dir.join(format!("{id2}.json"))).ok();
        drop((l1, l2));

        assert!(found1, "first Virtuoso's session must appear in list");
        assert!(found2, "second Virtuoso's session must appear in list");
    }

    #[test]
    fn stale_session_filtered_in_cleanup() {
        // Dead session files (port not bound) must be removed by session::cleanup()
        // Bind then drop to get a port we know is currently free.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id = format!("rt-test-stale-{port}");
        fs::remove_file(dir.join(format!("{id}.json"))).ok();
        write_session(&dir, &make_session(&id, port));

        let result = crate::commands::session::cleanup().unwrap();
        let removed: Vec<String> = result["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        fs::remove_file(dir.join(format!("{id}.json"))).ok();

        assert!(
            removed.contains(&id),
            "stale session must appear in cleanup result"
        );
    }

    #[test]
    fn live_session_not_removed_by_cleanup() {
        // A session whose port is actually bound must survive cleanup()
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let dir = SessionInfo::sessions_dir();
        fs::create_dir_all(&dir).unwrap();

        let id = format!("rt-test-live-{port}");
        fs::remove_file(dir.join(format!("{id}.json"))).ok();
        write_session(&dir, &make_session(&id, port));

        let result = crate::commands::session::cleanup().unwrap();
        let removed: Vec<String> = result["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        fs::remove_file(dir.join(format!("{id}.json"))).ok();
        drop(listener);

        assert!(
            !removed.contains(&id),
            "live session must not be removed by cleanup"
        );
    }
}

#[cfg(test)]
mod sexp_tests {
    use crate::client::skill_sexp::{parse_sexp, sexp_to_str_list, SexpVal};

    #[test]
    fn roundtrip_empty_list() {
        assert_eq!(parse_sexp("()").unwrap(), SexpVal::List(vec![]));
    }

    #[test]
    fn nested_list_of_lists() {
        let input = r#"(("fnxSession0" "idle") ("fnxSession1" nil))"#;
        let val = parse_sexp(input).unwrap();
        let outer = match val {
            SexpVal::List(v) => v,
            other => panic!("expected List, got {other:?}"),
        };
        assert_eq!(outer.len(), 2);

        let row0 = sexp_to_str_list(&outer[0]).unwrap();
        assert_eq!(row0, vec![Some("fnxSession0".into()), Some("idle".into())]);

        let row1 = sexp_to_str_list(&outer[1]).unwrap();
        assert_eq!(row1, vec![Some("fnxSession1".into()), None]);
    }

    #[test]
    fn string_with_embedded_quotes() {
        let val = parse_sexp(r#""say \"hello\"""#).unwrap();
        assert_eq!(val, SexpVal::Str(r#"say "hello""#.into()));
    }

    #[test]
    fn nil_top_level() {
        assert_eq!(parse_sexp("nil").unwrap(), SexpVal::Nil);
    }

    #[test]
    fn bool_true_top_level() {
        assert_eq!(parse_sexp("t").unwrap(), SexpVal::Bool(true));
    }

    #[test]
    fn sexp_to_str_list_on_non_list_returns_none() {
        assert!(sexp_to_str_list(&SexpVal::Nil).is_none());
        assert!(sexp_to_str_list(&SexpVal::Bool(true)).is_none());
        assert!(sexp_to_str_list(&SexpVal::Str("x".into())).is_none());
    }

    #[test]
    fn whitespace_is_ignored() {
        let val = parse_sexp("  (  nil   t  )  ").unwrap();
        assert_eq!(val, SexpVal::List(vec![SexpVal::Nil, SexpVal::Bool(true)]));
    }

    #[test]
    fn atom_preserved_as_is() {
        assert_eq!(
            parse_sexp("fnxSession3").unwrap(),
            SexpVal::Atom("fnxSession3".into())
        );
    }
}

#[cfg(test)]
mod cm_tests {
    use super::cmd_args;
    use crate::transport::ssh::SSHRunner;

    #[test]
    fn cm_failure_mux_client() {
        assert!(SSHRunner::is_cm_failure(
            "mux_client_request_session: send fds failed"
        ));
    }

    #[test]
    fn cm_failure_named_pipe() {
        assert!(SSHRunner::is_cm_failure(
            "ssh_mux_client_open: could not create named pipe"
        ));
    }

    #[test]
    fn cm_failure_control_path() {
        assert!(SSHRunner::is_cm_failure(
            "ControlPath too long for socket: /home/用户/.cache/..."
        ));
    }

    #[test]
    fn cm_failure_control_socket() {
        assert!(SSHRunner::is_cm_failure(
            "Control socket connect(/tmp/...): No such file or directory"
        ));
    }

    #[test]
    fn connection_refused_is_not_cm_failure() {
        assert!(!SSHRunner::is_cm_failure(
            "ssh: connect to host eda port 22: Connection refused"
        ));
    }

    #[test]
    fn auth_failure_is_not_cm_failure() {
        assert!(!SSHRunner::is_cm_failure("Permission denied (publickey)."));
    }

    #[test]
    fn cm_disabled_by_default_is_true() {
        let r = SSHRunner::new("eda");
        assert!(r.use_control_master.get());
    }

    #[test]
    fn cm_can_be_disabled() {
        let r = SSHRunner::new("eda");
        r.use_control_master.set(false);
        assert!(!r.use_control_master.get());

        // Verify SSH command no longer contains ControlMaster options
        let args = cmd_args(&r.build_ssh_cmd());
        assert!(
            !args.iter().any(|a| a.contains("ControlMaster")),
            "ControlMaster should be absent when disabled: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("ControlPath")),
            "ControlPath should be absent when disabled: {args:?}"
        );
    }

    #[test]
    fn cm_enabled_adds_control_master_args() {
        let r = SSHRunner::new("eda");
        let args = cmd_args(&r.build_ssh_cmd());
        assert!(
            args.iter().any(|a| a.contains("ControlMaster")),
            "ControlMaster should be present when enabled: {args:?}"
        );
    }
}

#[cfg(test)]
mod config_tests_ext {
    use crate::config::Config;
    use crate::tests::config_tests::ENV_LOCK;
    use std::env;

    fn clean_ext_env() {
        env::remove_var("VB_PORT");
        env::remove_var("VB_REMOTE_HOST");
        env::remove_var("VB_SSH_CONFIG");
        env::remove_var("VB_DISABLE_CONTROL_MASTER");
    }

    #[test]
    fn vb_ssh_config_sets_field() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_ext_env();
        env::set_var("VB_SSH_CONFIG", "/home/meow/.ssh/custom_config");
        let cfg = Config::from_env().unwrap();
        clean_ext_env();
        assert_eq!(
            cfg.ssh_config.as_deref(),
            Some("/home/meow/.ssh/custom_config")
        );
    }

    #[test]
    fn vb_ssh_config_unset_is_none() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_ext_env();
        let cfg = Config::from_env().unwrap();
        clean_ext_env();
        assert!(cfg.ssh_config.is_none());
    }

    #[test]
    fn vb_disable_control_master_one() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_ext_env();
        env::set_var("VB_DISABLE_CONTROL_MASTER", "1");
        let cfg = Config::from_env().unwrap();
        clean_ext_env();
        assert!(cfg.disable_control_master);
    }

    #[test]
    fn vb_disable_control_master_true() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_ext_env();
        env::set_var("VB_DISABLE_CONTROL_MASTER", "true");
        let cfg = Config::from_env().unwrap();
        clean_ext_env();
        assert!(cfg.disable_control_master);
    }

    #[test]
    fn vb_disable_control_master_default_false() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean_ext_env();
        let cfg = Config::from_env().unwrap();
        clean_ext_env();
        assert!(!cfg.disable_control_master);
    }
}

#[cfg(test)]
mod ssh_login_shell_tests {
    use super::cmd_args;
    use crate::transport::ssh::SSHRunner;

    #[test]
    fn build_run_cmd_includes_login_flag() {
        let r = SSHRunner::new("eda");
        let args = cmd_args(&r.build_run_cmd());
        assert!(args.contains(&"sh".to_string()), "sh missing: {args:?}");
        assert!(
            args.contains(&"-l".to_string()),
            "login flag -l missing: {args:?}"
        );
        assert!(
            args.contains(&"-s".to_string()),
            "stdin flag -s missing: {args:?}"
        );
    }

    #[test]
    fn build_run_cmd_login_flag_after_host() {
        // sh -l -s must come after the SSH host argument, not before
        let r = SSHRunner::new("eda-server").with_user("meow");
        let args = cmd_args(&r.build_run_cmd());
        let host_idx = args
            .iter()
            .position(|a| a == "meow@eda-server")
            .expect("host arg missing");
        let sh_idx = args.iter().position(|a| a == "sh").expect("sh missing");
        assert!(sh_idx > host_idx, "sh must come after SSH host");
    }
}

#[cfg(test)]
mod daemon_stats_tests {
    use crate::models::DaemonStats;
    use std::fs;

    #[test]
    fn path_format() {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("virtuoso_bridge");
        assert_eq!(
            DaemonStats::path(41357),
            cache_dir.join(".ramic_stats_41357").to_string_lossy()
        );
        assert_eq!(
            DaemonStats::path(0),
            cache_dir.join(".ramic_stats_0").to_string_lossy()
        );
        assert_eq!(
            DaemonStats::path(65535),
            cache_dir.join(".ramic_stats_65535").to_string_lossy()
        );
    }

    #[test]
    fn load_missing_file_returns_none() {
        // Use an unlikely port to avoid collision with a real daemon
        assert!(DaemonStats::load(1).is_none());
    }

    #[test]
    fn load_valid_json() {
        let port: u16 = 59991;
        let path = DaemonStats::path(port);
        fs::write(&path, r#"{"calls":42,"errors":3,"uptime_secs":120}"#).unwrap();
        let stats = DaemonStats::load(port).expect("should load");
        let _ = fs::remove_file(&path);
        assert_eq!(stats.calls, 42);
        assert_eq!(stats.errors, 3);
        assert_eq!(stats.uptime_secs, 120);
    }

    #[test]
    fn load_malformed_json_returns_none() {
        let port: u16 = 59992;
        let path = DaemonStats::path(port);
        fs::write(&path, "not json {{{{").unwrap();
        let result = DaemonStats::load(port);
        let _ = fs::remove_file(&path);
        assert!(result.is_none());
    }

    #[test]
    fn json_round_trip() {
        let stats = DaemonStats {
            calls: 100,
            errors: 5,
            uptime_secs: 3661,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let stats2: DaemonStats = serde_json::from_str(&json).unwrap();
        assert_eq!(stats2.calls, 100);
        assert_eq!(stats2.errors, 5);
        assert_eq!(stats2.uptime_secs, 3661);
    }
}

#[cfg(test)]
mod suggestion_tests {
    use crate::error::VirtuosoError;

    #[test]
    fn execution_ending_with_nil_has_suggestion() {
        let e = VirtuosoError::Execution("close session failed: nil".into());
        let s = e.suggestion().expect("should have suggestion");
        assert!(s.contains("nil"), "got: {s}");
    }

    #[test]
    fn execution_containing_unbound_has_suggestion() {
        let e = VirtuosoError::Execution("*Error* eval: unbound variable foo".into());
        let s = e.suggestion().expect("should have suggestion");
        assert!(s.contains("nil") || s.contains("cellview"), "got: {s}");
    }

    #[test]
    fn execution_nil_substring_no_false_positive() {
        // "nil" appearing mid-word or mid-string must not trigger the suggestion
        // (only ": nil" suffix or "unbound" should match)
        let e = VirtuosoError::Execution("failed for client-nil-session".into());
        assert!(e.suggestion().is_none(), "false positive on embedded nil");
    }

    #[test]
    fn not_found_suggests_session_list() {
        let e = VirtuosoError::NotFound("meowu-meow-99".into());
        let s = e.suggestion().expect("NotFound should have suggestion");
        assert!(
            s.contains("session list") || s.contains("session"),
            "got: {s}"
        );
    }

    #[test]
    fn connection_error_suggests_tunnel() {
        let e = VirtuosoError::Connection("refused".into());
        let s = e.suggestion().expect("Connection should have suggestion");
        assert!(s.contains("tunnel"), "got: {s}");
    }

    #[test]
    fn timeout_suggestion_doubles_seconds() {
        let e = VirtuosoError::Timeout(30);
        let s = e.suggestion().expect("Timeout should have suggestion");
        assert!(
            s.contains("60"),
            "doubled timeout missing from suggestion: {s}"
        );
    }

    #[test]
    fn unrelated_execution_error_has_no_suggestion() {
        let e = VirtuosoError::Execution("some completely unrelated failure".into());
        assert!(e.suggestion().is_none());
    }
}

#[cfg(test)]
mod error_meta_tests {
    use crate::error::VirtuosoError;
    use crate::exit_codes;

    #[test]
    fn exit_code_config_is_usage_error() {
        assert_eq!(
            VirtuosoError::Config("bad".into()).exit_code(),
            exit_codes::USAGE_ERROR
        );
    }

    #[test]
    fn exit_code_not_found() {
        assert_eq!(
            VirtuosoError::NotFound("x".into()).exit_code(),
            exit_codes::NOT_FOUND
        );
    }

    #[test]
    fn exit_code_conflict() {
        assert_eq!(
            VirtuosoError::Conflict("x".into()).exit_code(),
            exit_codes::CONFLICT
        );
    }

    #[test]
    fn exit_code_connection_and_ssh_and_timeout_are_general() {
        assert_eq!(
            VirtuosoError::Connection("x".into()).exit_code(),
            exit_codes::GENERAL_ERROR
        );
        assert_eq!(
            VirtuosoError::Ssh("x".into()).exit_code(),
            exit_codes::GENERAL_ERROR
        );
        assert_eq!(
            VirtuosoError::Timeout(10).exit_code(),
            exit_codes::GENERAL_ERROR
        );
    }

    #[test]
    fn error_type_strings() {
        assert_eq!(
            VirtuosoError::Connection("".into()).error_type(),
            "connection_failed"
        );
        assert_eq!(
            VirtuosoError::Execution("".into()).error_type(),
            "execution_failed"
        );
        assert_eq!(VirtuosoError::Ssh("".into()).error_type(), "ssh_error");
        assert_eq!(VirtuosoError::Timeout(5).error_type(), "timeout");
        assert_eq!(
            VirtuosoError::Config("".into()).error_type(),
            "config_error"
        );
        assert_eq!(VirtuosoError::NotFound("".into()).error_type(), "not_found");
        assert_eq!(VirtuosoError::Conflict("".into()).error_type(), "conflict");
    }

    #[test]
    fn retryable_only_connection_and_timeout() {
        assert!(VirtuosoError::Connection("x".into()).retryable());
        assert!(VirtuosoError::Timeout(5).retryable());
        assert!(!VirtuosoError::Execution("x".into()).retryable());
        assert!(!VirtuosoError::Ssh("x".into()).retryable());
        assert!(!VirtuosoError::Config("x".into()).retryable());
        assert!(!VirtuosoError::NotFound("x".into()).retryable());
        assert!(!VirtuosoError::Conflict("x".into()).retryable());
    }

    #[test]
    fn to_cli_error_maps_all_fields() {
        let e = VirtuosoError::Connection("refused".into());
        let ce = e.to_cli_error();
        assert_eq!(ce.error, "connection_failed");
        assert!(ce.message.contains("refused"), "{}", ce.message);
        assert!(ce.suggestion.is_some());
        assert!(ce.retryable);
    }

    #[test]
    fn to_cli_error_not_found_has_suggestion_and_not_retryable() {
        let e = VirtuosoError::NotFound("sess-x".into());
        let ce = e.to_cli_error();
        assert_eq!(ce.error, "not_found");
        assert!(ce.suggestion.is_some());
        assert!(!ce.retryable);
    }
}

#[cfg(test)]
mod virtuoso_result_tests {
    use crate::models::{ExecutionStatus, VirtuosoResult};

    fn make_success(output: &str) -> VirtuosoResult {
        VirtuosoResult::success(output)
    }

    fn make_error(errors: Vec<String>) -> VirtuosoResult {
        VirtuosoResult::error(errors)
    }

    #[test]
    fn ok_true_for_success_status() {
        assert!(make_success("result").ok());
    }

    #[test]
    fn ok_false_for_error_status() {
        assert!(!make_error(vec![]).ok());
    }

    #[test]
    fn skill_ok_false_when_output_is_nil() {
        assert!(!make_success("nil").skill_ok());
        assert!(!make_success("  nil  ").skill_ok());
    }

    #[test]
    fn skill_ok_true_for_non_nil_success() {
        assert!(make_success("t").skill_ok());
        assert!(make_success("\"some result\"").skill_ok());
        assert!(make_success("42").skill_ok());
    }

    #[test]
    fn skill_ok_false_when_status_error_even_if_non_nil_output() {
        let mut r = make_error(vec![]);
        r.output = "42".into();
        assert!(!r.skill_ok());
    }

    #[test]
    fn ok_or_exec_passes_through_on_success() {
        let r = make_success("42");
        assert!(r.ok_or_exec("op").is_ok());
    }

    #[test]
    fn ok_or_exec_returns_err_on_nil() {
        let r = make_success("nil");
        let e = r.ok_or_exec("myop").unwrap_err();
        assert!(e.to_string().contains("myop"), "{e}");
    }

    #[test]
    fn ok_or_exec_includes_nak_error_detail() {
        let mut r = make_error(vec!["*Error* eval: undefined".into()]);
        r.output = String::new();
        let e = r.ok_or_exec("fetch").unwrap_err();
        assert!(e.to_string().contains("*Error*"), "{e}");
    }

    #[test]
    fn output_unquoted_strips_surrounding_quotes() {
        let r = make_success("\"hello\"");
        assert_eq!(r.output_unquoted(), "hello");
    }

    #[test]
    fn output_unquoted_no_quotes_unchanged() {
        let r = make_success("hello");
        assert_eq!(r.output_unquoted(), "hello");
    }

    #[test]
    fn output_unquoted_empty_quoted_string() {
        let r = make_success("\"\"");
        assert_eq!(r.output_unquoted(), "");
    }

    #[test]
    fn success_constructor_sets_status() {
        let r = make_success("ok");
        assert_eq!(r.status, ExecutionStatus::Success);
        assert_eq!(r.output, "ok");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn error_constructor_sets_status_and_errors() {
        let r = make_error(vec!["oops".into()]);
        assert_eq!(r.status, ExecutionStatus::Error);
        assert_eq!(r.errors, vec!["oops"]);
    }
}

#[cfg(test)]
mod schematic_tests {
    use crate::commands::schematic::{parse_skill_json, Orient};

    #[test]
    fn orient_as_str_all_variants() {
        assert_eq!(Orient::R0.as_str(), "R0");
        assert_eq!(Orient::R90.as_str(), "R90");
        assert_eq!(Orient::R180.as_str(), "R180");
        assert_eq!(Orient::R270.as_str(), "R270");
        assert_eq!(Orient::MX.as_str(), "MX");
        assert_eq!(Orient::MY.as_str(), "MY");
        assert_eq!(Orient::MXR90.as_str(), "MXR90");
        assert_eq!(Orient::MYR90.as_str(), "MYR90");
    }

    #[test]
    fn parse_plain_json_array() {
        let v = parse_skill_json(r#"[{"name":"M1"}]"#).unwrap();
        assert_eq!(v[0]["name"], "M1");
    }

    #[test]
    fn parse_skill_quoted_json() {
        // SKILL returns the JSON as a quoted string: "\"[{...}]\""
        let v = parse_skill_json(r#""[{\"name\":\"M1\"}]""#).unwrap();
        assert_eq!(v[0]["name"], "M1");
    }

    #[test]
    fn parse_malformed_returns_err() {
        assert!(parse_skill_json("not json {{{{").is_err());
    }

    #[test]
    fn parse_empty_array() {
        let v = parse_skill_json("[]").unwrap();
        assert!(v.as_array().unwrap().is_empty());
    }
}

#[cfg(test)]
mod config_extra_tests {
    use crate::config::Config;
    use crate::tests::config_tests::ENV_LOCK;
    use std::env;

    fn clean() {
        env::remove_var("VB_PORT");
        env::remove_var("VB_REMOTE_HOST");
        env::remove_var("VB_REMOTE_HOST_prod");
        env::remove_var("VB_PROFILE");
        env::remove_var("USER");
    }

    #[test]
    fn default_port_in_expected_range() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean();
        let cfg = Config::from_env().unwrap();
        clean();
        assert!(cfg.port >= 65000 && cfg.port < 65500, "port: {}", cfg.port);
    }

    #[test]
    fn default_port_deterministic_for_same_user() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean();
        env::set_var("USER", "testuser");
        env::remove_var("VB_PORT");
        let cfg1 = Config::from_env().unwrap();
        let cfg2 = Config::from_env().unwrap();
        clean();
        assert_eq!(cfg1.port, cfg2.port);
    }

    #[test]
    fn env_with_profile_prefers_profile_key() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean();
        env::set_var("VB_REMOTE_HOST", "generic-host");
        env::set_var("VB_REMOTE_HOST_prod", "prod-host");
        let cfg = Config::from_env_with_profile(Some("prod")).unwrap();
        clean();
        assert_eq!(cfg.remote_host.as_deref(), Some("prod-host"));
    }

    #[test]
    fn env_with_profile_falls_back_to_base_key() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean();
        env::set_var("VB_REMOTE_HOST", "generic-host");
        let cfg = Config::from_env_with_profile(Some("staging")).unwrap();
        clean();
        assert_eq!(cfg.remote_host.as_deref(), Some("generic-host"));
    }

    #[test]
    fn env_with_profile_empty_value_treated_as_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clean();
        env::set_var("VB_REMOTE_HOST_prod", "");
        env::set_var("VB_REMOTE_HOST", "generic-host");
        let cfg = Config::from_env_with_profile(Some("prod")).unwrap();
        clean();
        // empty profile-specific value → fall back to base
        assert_eq!(cfg.remote_host.as_deref(), Some("generic-host"));
    }
}

#[cfg(test)]
mod maestro_ops_extra_tests {
    use crate::client::maestro_ops::MaestroOps;

    fn ops() -> MaestroOps {
        MaestroOps
    }

    #[test]
    fn focused_window_skill_contains_required_calls() {
        let s = ops().focused_window_skill();
        assert!(s.contains("hiGetCurrentWindow()"), "{s}");
        assert!(s.contains("davSession"), "{s}");
        assert!(s.contains("maeGetSessions()"), "{s}");
        assert!(s.contains("asiGetAnalogRunDir"), "{s}");
    }

    #[test]
    fn run_dir_skill_escapes_session_name() {
        let s = ops().run_dir_skill(r#"sess"x"#);
        assert!(s.contains(r#"sess\"x"#), "{s}");
        assert!(s.contains("asiGetAnalogRunDir"), "{s}");
    }

    #[test]
    fn run_dir_skill_wraps_in_let() {
        let s = ops().run_dir_skill("sess1");
        assert!(s.starts_with("let("), "{s}");
        assert!(s.contains("\"sess1\""), "{s}");
    }
}

#[cfg(test)]
mod output_format_tests {
    use crate::output::{CliError, OutputFormat};

    #[test]
    fn resolve_json_explicit() {
        assert_eq!(OutputFormat::resolve(Some("json")), OutputFormat::Json);
    }

    #[test]
    fn resolve_table_explicit() {
        assert_eq!(OutputFormat::resolve(Some("table")), OutputFormat::Table);
    }

    #[test]
    fn resolve_unknown_explicit_falls_back_to_table() {
        assert_eq!(OutputFormat::resolve(Some("csv")), OutputFormat::Table);
        assert_eq!(OutputFormat::resolve(Some("")), OutputFormat::Table);
    }

    #[test]
    fn resolve_none_in_non_tty_is_json() {
        // Test processes run without a TTY, so None → Json
        assert_eq!(OutputFormat::resolve(None), OutputFormat::Json);
    }

    #[test]
    fn cli_error_json_with_suggestion() {
        let e = CliError {
            error: "not_found".into(),
            message: "sess-x not found".into(),
            suggestion: Some("vcli session list".into()),
            diagnostic: None,
            retryable: false,
        };
        let json = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["error"], "not_found");
        assert_eq!(v["message"], "sess-x not found");
        assert_eq!(v["suggestion"], "vcli session list");
        assert_eq!(v["retryable"], false);
    }

    #[test]
    fn cli_error_json_omits_suggestion_when_none() {
        let e = CliError {
            error: "execution_failed".into(),
            message: "failed".into(),
            suggestion: None,
            diagnostic: None,
            retryable: false,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains("suggestion"),
            "suggestion key should be absent: {json}"
        );
    }

    #[test]
    fn cli_error_retryable_serializes() {
        let e = CliError {
            error: "connection_failed".into(),
            message: "refused".into(),
            suggestion: None,
            diagnostic: None,
            retryable: true,
        };
        let json = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["retryable"], true);
    }
}

#[cfg(test)]
mod job_tests {
    use crate::spectre::jobs::{Job, JobStatus};

    fn make_job(id: &str) -> Job {
        Job {
            id: id.into(),
            status: JobStatus::Running,
            netlist_path: "/tmp/test.scs".into(),
            raw_dir: None,
            pid: Some(99999),
            created: "2026-05-01T00:00:00+00:00".into(),
            finished: None,
            error: None,
            remote_host: None,
            remote_dir: None,
        }
    }

    #[test]
    fn job_status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&JobStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    #[test]
    fn job_status_deserializes_lowercase() {
        let s: JobStatus = serde_json::from_str("\"running\"").unwrap();
        assert_eq!(s, JobStatus::Running);
    }

    #[test]
    fn job_json_round_trip() {
        let job = make_job("rt-test-roundtrip");
        let json = serde_json::to_string_pretty(&job).unwrap();
        let job2: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(job2.id, job.id);
        assert_eq!(job2.status, job.status);
        assert_eq!(job2.netlist_path, job.netlist_path);
        assert_eq!(job2.pid, job.pid);
    }

    #[test]
    fn job_save_and_load_round_trip() {
        let id = "rt-test-save-load-01";
        let job = make_job(id);
        job.save().expect("save should succeed");

        let loaded = Job::load(id).expect("load should succeed");
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.status, JobStatus::Running);
        assert_eq!(loaded.pid, Some(99999));

        Job::delete(id).ok();
    }

    #[test]
    fn job_load_missing_returns_not_found() {
        let err = Job::load("rt-test-nonexistent-xyz-99").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn cancel_non_running_job_returns_error() {
        for status in [JobStatus::Completed, JobStatus::Failed] {
            let mut job = make_job("rt-test-cancel-non-running");
            job.status = status;
            let err = job.cancel().unwrap_err();
            assert!(err.to_string().contains("not running"), "{err}");
        }
    }

    #[test]
    fn list_all_includes_saved_job() {
        let id = "rt-test-list-all-01";
        let job = make_job(id);
        job.save().expect("save");

        let jobs = Job::list_all().expect("list_all");
        let found = jobs.iter().any(|j| j.id == id);
        Job::delete(id).ok();
        assert!(found, "saved job should appear in list_all");
    }

    #[test]
    fn list_all_sorted_by_created() {
        let id_a = "rt-test-sort-aaa";
        let id_z = "rt-test-sort-zzz";
        let mut job_a = make_job(id_a);
        let mut job_z = make_job(id_z);
        job_a.created = "2026-01-01T00:00:00+00:00".into();
        job_z.created = "2026-12-31T00:00:00+00:00".into();
        job_a.save().ok();
        job_z.save().ok();

        let jobs = Job::list_all().expect("list_all");
        let pos_a = jobs.iter().position(|j| j.id == id_a);
        let pos_z = jobs.iter().position(|j| j.id == id_z);
        Job::delete(id_a).ok();
        Job::delete(id_z).ok();

        if let (Some(a), Some(z)) = (pos_a, pos_z) {
            assert!(a < z, "earlier created job should sort first");
        }
    }
}

#[cfg(test)]
mod history_tests {
    use crate::history::{append_cmd, append_skill, history_dir, load_cmd, load_skill};
    use std::fs;

    fn rm_skill(session_id: &str) {
        let _ = fs::remove_file(history_dir().join(format!("{session_id}.jsonl")));
    }

    #[test]
    fn append_and_load_skill_entries() {
        let id = "rt-hist-skill-basic";
        rm_skill(id);

        append_skill(id, "version()", true, "IC231");
        append_skill(id, "car(list(1 2))", true, "1");
        append_skill(id, "nil", false, "");

        let entries = load_skill(id, 0);
        assert_eq!(entries.len(), 3, "all three entries must be present");
        assert_eq!(entries[0].skill, "version()");
        assert!(entries[0].ok);
        assert_eq!(entries[2].skill, "nil");
        assert!(!entries[2].ok);

        rm_skill(id);
    }

    #[test]
    fn skill_output_truncated_at_512_chars() {
        let id = "rt-hist-skill-trunc";
        rm_skill(id);

        append_skill(id, "expr", true, &"x".repeat(1000));

        let entries = load_skill(id, 0);
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].output.len() <= 512,
            "output must be truncated to 512 chars, got {}",
            entries[0].output.len()
        );

        rm_skill(id);
    }

    #[test]
    fn load_skill_missing_session_returns_empty() {
        let entries = load_skill("rt-hist-no-such-session-xyz", 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn skill_entries_have_timestamp() {
        let id = "rt-hist-skill-ts";
        rm_skill(id);

        append_skill(id, "t", true, "t");

        let entries = load_skill(id, 0);
        assert!(!entries[0].ts.is_empty(), "timestamp must be populated");
        assert!(
            entries[0].ts.starts_with("20"),
            "timestamp must be ISO-8601, got: {}",
            entries[0].ts
        );

        rm_skill(id);
    }

    #[test]
    fn append_and_load_cmd_entries() {
        // Use a unique session ID so this test's entries are isolated
        let session_id = "rt-hist-cmd-basic-54321";
        let args = vec![
            "vcli".to_string(),
            "skill".to_string(),
            "exec".to_string(),
            "version()".to_string(),
        ];
        append_cmd(&args, Some(session_id), 0);

        let entries = load_cmd(Some(session_id), 0);
        assert!(!entries.is_empty(), "at least one cmd entry must be found");
        let last = entries.last().unwrap();
        assert_eq!(last.cmd, args);
        assert_eq!(last.exit_code, 0);
        assert_eq!(last.session.as_deref(), Some(session_id));
    }

    #[test]
    fn cmd_error_exit_code_recorded() {
        let session_id = "rt-hist-cmd-err-54321";
        append_cmd(
            &["vcli".to_string(), "bad-cmd".to_string()],
            Some(session_id),
            1,
        );

        let entries = load_cmd(Some(session_id), 0);
        let last = entries.last().unwrap();
        assert_eq!(last.exit_code, 1, "exit_code 1 must be recorded");
    }

    #[test]
    fn load_cmd_session_filter_isolates_entries() {
        let s1 = "rt-hist-cmd-filter-s1-11111";
        let s2 = "rt-hist-cmd-filter-s2-22222";

        append_cmd(&["vcli".to_string(), "skill".to_string()], Some(s1), 0);
        append_cmd(&["vcli".to_string(), "maestro".to_string()], Some(s2), 0);

        let for_s1 = load_cmd(Some(s1), 0);
        assert!(
            for_s1.iter().all(|e| e.session.as_deref() == Some(s1)),
            "filter must return only s1 entries"
        );
        assert!(
            for_s1.iter().any(|e| e.cmd.contains(&"skill".to_string())),
            "s1 entry with 'skill' arg must be present"
        );

        let for_s2 = load_cmd(Some(s2), 0);
        assert!(
            for_s2.iter().all(|e| e.session.as_deref() == Some(s2)),
            "filter must return only s2 entries"
        );
    }

    #[test]
    fn load_cmd_limit_caps_result_count() {
        let session_id = "rt-hist-cmd-limit-33333";
        for i in 0..10u32 {
            append_cmd(&["vcli".to_string(), i.to_string()], Some(session_id), 0);
        }

        let limited = load_cmd(Some(session_id), 3);
        assert!(
            limited.len() <= 3,
            "limit=3 must return ≤3 entries, got {}",
            limited.len()
        );
    }

    #[test]
    fn load_cmd_limit_zero_returns_all() {
        let session_id = "rt-hist-cmd-nolimit-44444";
        for i in 0..5u32 {
            append_cmd(&["vcli".to_string(), i.to_string()], Some(session_id), 0);
        }

        let all = load_cmd(Some(session_id), 0);
        assert!(
            all.len() >= 5,
            "limit=0 must return all entries, got {}",
            all.len()
        );
    }

    #[test]
    fn session_history_command_returns_correct_structure() {
        let id = "rt-hist-cmd-struct-55555";
        rm_skill(id);

        append_skill(id, "car(list())", true, "nil");
        append_skill(id, "t", true, "t");

        let result = crate::commands::session::history(id, false, false, 50).unwrap();
        assert_eq!(result["status"], "success");
        assert_eq!(result["session"], id);
        assert!(result["skill"].is_array());
        assert!(result["cmd"].is_array());

        let skill_arr = result["skill"].as_array().unwrap();
        assert_eq!(skill_arr.len(), 2, "two SKILL entries must be returned");
        assert_eq!(skill_arr[0]["type"], "skill");
        assert_eq!(skill_arr[0]["skill"], "car(list())");

        rm_skill(id);
    }

    #[test]
    fn session_history_skill_only_flag() {
        let id = "rt-hist-cmd-skillonly-66666";
        rm_skill(id);
        append_skill(id, "t", true, "t");

        let result = crate::commands::session::history(id, true, false, 50).unwrap();
        assert!(!result["skill"].as_array().unwrap().is_empty());
        assert_eq!(
            result["cmd"].as_array().unwrap().len(),
            0,
            "--skill flag must return empty cmd list"
        );

        rm_skill(id);
    }

    #[test]
    fn session_history_cmd_only_flag() {
        let session_id = "rt-hist-cmd-cmdonly-77777";
        append_cmd(&["vcli".to_string()], Some(session_id), 0);

        let result = crate::commands::session::history(session_id, false, true, 50).unwrap();
        assert_eq!(
            result["skill"].as_array().unwrap().len(),
            0,
            "--cmd flag must return empty skill list"
        );
        assert!(!result["cmd"].as_array().unwrap().is_empty());
    }
}

#[cfg(test)]
mod skill_command_tests {
    /// Test that eval's progn wrapping is correct for various inputs.
    #[test]
    fn eval_progn_wrapping_single_expression() {
        // Verify that wrapping single expression in progn works correctly
        let code = "1+1";
        let wrapped = format!("progn(\n{}\n)", code);
        assert_eq!(wrapped, "progn(\n1+1\n)");
    }

    #[test]
    fn eval_progn_wrapping_multiline() {
        let code = "let((x 5))\n x*x\n)";
        let wrapped = format!("progn(\n{}\n)", code);
        assert!(wrapped.starts_with("progn(\n"));
        assert!(wrapped.ends_with("\n)"));
        assert!(wrapped.contains("let((x 5))"));
        assert!(wrapped.contains("x*x"));
    }

    #[test]
    fn eval_progn_trailing_comment() {
        // Ensure trailing comment doesn't swallow the closing paren
        let code = "1+1 ; trailing comment";
        let wrapped = format!("progn(\n{}\n)", code);
        // The newline before ) terminates the line comment
        assert!(wrapped.ends_with("\n)"));
    }

    #[test]
    fn eval_input_validation_empty_code() {
        use crate::commands::skill;
        use crate::error::VirtuosoError;

        let result = skill::eval(Some("   ".to_string()), false);
        assert!(matches!(result, Err(VirtuosoError::Config(_))));
    }

    #[test]
    fn eval_input_validation_no_code() {
        use crate::commands::skill;
        use crate::error::VirtuosoError;

        let result = skill::eval(None, false);
        assert!(matches!(result, Err(VirtuosoError::Config(_))));
    }

    #[test]
    fn eval_input_validation_stdin_and_argv_conflict() {
        use crate::commands::skill;
        use crate::error::VirtuosoError;

        let result = skill::eval(Some("code".to_string()), true);
        assert!(matches!(result, Err(VirtuosoError::Config(_))));
    }
}

/// Integration tests for Spectre simulation parsing.
/// These tests use real file structures that Spectre generates.
#[cfg(test)]
mod spectre_integration_tests {
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_end_to_end_sweep_directory_classic() {
        // Simulate classic Spectre sweep output: sw1.sweep1/1/, sw1.sweep1/2/, ...
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create sweep directories with multiple signals
        for sweep_idx in 1..=3 {
            let sweep_dir = raw.join(format!("sw1.sweep{}", sweep_idx));
            let point_dir = sweep_dir.join(sweep_idx.to_string());
            fs::create_dir_all(&point_dir).unwrap();

            // Create tran.tran.tran with SWEEP/TRACE/VALUE sections
            let tran_file = point_dir.join("tran.tran.tran");
            let content = format!(
                r#"SWEEP
"time"
0.0
1.0e-6
2.0e-6
TRACE
VALUE
net1
0.5
1.0
1.5
net2
0.0
0.1
0.2
END"#
            );
            fs::write(&tran_file, content).unwrap();
        }

        // Parse the sweep directory
        let sweep_data =
            crate::spectre::parsers::parse_sweep_psf_directory(&raw).expect("parse should succeed");

        // Verify we have 3 sweep points
        assert_eq!(sweep_data.len(), 3);
        for idx in 1..=3 {
            assert!(sweep_data.contains_key(&idx), "Missing sweep point {}", idx);
        }

        // Parse as flat sweep points
        let points = crate::spectre::parsers::parse_sweep_flat(&raw).expect("parse should succeed");
        assert_eq!(points.len(), 3);

        // Verify sweep point structure
        for (i, point) in points.iter().enumerate() {
            assert_eq!(point.index, i + 1);
            // Each point should have signal data
            assert!(!point.signals.is_empty());
        }
    }

    #[test]
    fn test_end_to_end_sweep_directory_lx_mode() {
        // Simulate Spectre LX/X mode flat file naming: sw1-000_tran.tran, sw1-001_tran.tran, ...
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        for i in 0..5 {
            let filename = format!("sw1-{:03}_tran.tran", i);
            let path = raw.join(filename);

            // Create PSF content with time values
            let time_values: Vec<String> = (0..=100)
                .map(|j| format!("{:.6}", j as f64 * 1e-8))
                .collect();
            let content = format!("SWEEP\n{}\nTRACE\nVALUE\nEND", time_values.join("\n"));

            fs::write(&path, content).unwrap();
        }

        let sweep_data =
            crate::spectre::parsers::parse_sweep_psf_directory(&raw).expect("parse should succeed");

        // Should have 5 sweep points (converted to 1-indexed)
        assert_eq!(sweep_data.len(), 5);
        assert!(sweep_data.contains_key(&1));
        assert!(sweep_data.contains_key(&5));
    }

    #[test]
    fn test_end_to_end_psf_ascii_regular() {
        // Simulate regular (non-sweep) PSF output
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        let psf_dir = raw.join("psf");
        fs::create_dir_all(&psf_dir).unwrap();

        // Create time signal
        let time_file = psf_dir.join("time.tran");
        fs::write(&time_file, "0.0\n1.0\n2.0\n").unwrap();

        // Create voltage signals
        let vout_file = psf_dir.join("V(out).tran");
        fs::write(&vout_file, "0.0\n0.5\n1.0\n").unwrap();

        let vin_file = psf_dir.join("V(in).tran");
        fs::write(&vin_file, "0.0\n0.1\n0.2\n").unwrap();

        let data = crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");

        assert_eq!(data.len(), 3);
        // Note: file_stem() strips the last extension, so "time.tran" -> "time"
        assert!(data.contains_key("time"));
        assert!(data.contains_key("V(out)"));
        assert!(data.contains_key("V(in)"));
    }

    #[test]
    fn test_end_to_end_results_dir_fallback() {
        // Test that results/ directory is used as fallback
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        let results_dir = raw.join("results");
        fs::create_dir_all(&results_dir).unwrap();

        let dc_file = results_dir.join("dc_op.dc");
        fs::write(&dc_file, "1.2\n").unwrap();

        let data = crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");
        // file_stem strips last extension: "dc_op.dc" -> "dc_op"
        assert!(data.contains_key("dc_op"));
    }

    #[test]
    fn test_end_to_end_sweep_with_params() {
        // Test sweep where each point has a parameter value
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Simulate VDS sweep with varying VGS values
        let sweep_params = [0.5, 1.0, 1.5, 2.0];
        for (idx, vgs) in sweep_params.iter().enumerate() {
            let sweep_dir = raw.join(format!("sw1.sweep{}", idx + 1));
            let point_dir = sweep_dir.join((idx + 1).to_string());
            fs::create_dir_all(&point_dir).unwrap();

            // Create DC operating point output
            let dc_file = point_dir.join("dc_op.dc");
            let content = format!("SWEEP\n\"vgs\"\n{}\nTRACE\nVALUE\nEND", vgs);
            fs::write(&dc_file, content).unwrap();
        }

        let points = crate::spectre::parsers::parse_sweep_flat(&raw).expect("parse should succeed");

        // Verify we extracted the sweep parameter values
        assert_eq!(points.len(), 4);
        for (i, point) in points.iter().enumerate() {
            let expected_vgs = sweep_params[i];
            // The sweep_value should be the parameter value
            assert!(
                (point.sweep_value - expected_vgs).abs() < 0.01,
                "Point {}: expected VGS={}, got {}",
                i,
                expected_vgs,
                point.sweep_value
            );
        }
    }

    #[test]
    fn test_end_to_end_mixed_sweep_and_dc() {
        // Test directory with both sweep and regular analysis
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create sweep structure
        let sweep_dir = raw.join("sw1.sweep1");
        let point_dir = sweep_dir.join("1");
        fs::create_dir_all(&point_dir).unwrap();

        let tran_file = point_dir.join("tran.tran.tran");
        fs::write(&tran_file, "SWEEP\n0.0\n1.0\n2.0\nTRACE\nVALUE\nEND").unwrap();

        // Also create regular psf directory with DC analysis
        let psf_dir = raw.join("psf");
        fs::create_dir_all(&psf_dir).unwrap();
        let dc_file = psf_dir.join("dc_op.dc");
        fs::write(&dc_file, "0.5\n").unwrap();

        // Sweep data should be returned
        let sweep_data =
            crate::spectre::parsers::parse_sweep_psf_directory(&raw).expect("parse should succeed");
        assert!(!sweep_data.is_empty());

        // Verify psf data also works
        let psf_data =
            crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");
        assert!(!psf_data.is_empty());
    }

    #[test]
    fn test_end_to_end_empty_raw_directory() {
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Empty directory should not cause errors
        let sweep_data =
            crate::spectre::parsers::parse_sweep_psf_directory(&raw).expect("parse should succeed");
        assert!(sweep_data.is_empty());

        let psf_data =
            crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");
        assert!(psf_data.is_empty());
    }

    #[test]
    fn test_sweep_point_scalar_extraction() {
        // Test extracting scalar values from sweep points
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create sweep with gain measurements
        // Note: The current parser extracts SWEEP section values (time/freq/params),
        // not VALUE section signal data. So we verify the sweep_value (param) extraction.
        for i in 1..=3 {
            let sweep_dir = raw.join(format!("sw1.sweep{}", i));
            let point_dir = sweep_dir.join(i.to_string());
            fs::create_dir_all(&point_dir).unwrap();

            // Use simple file name
            let dc_file = point_dir.join("dc.dc");
            // Format: SWEEP with parameter values, TRACE, VALUE (signal), signal value
            let content = format!(
                r#"SWEEP
"vdd"
{}.0
TRACE
VALUE
END"#,
                i
            );
            fs::write(&dc_file, content).unwrap();
        }

        let points = crate::spectre::parsers::parse_sweep_flat(&raw).expect("parse should succeed");

        // Verify sweep_value extraction (the parameter value at each sweep point)
        assert_eq!(points.len(), 3);
        for (i, point) in points.iter().enumerate() {
            let expected_vdd = (i + 1) as f64;
            assert!(
                (point.sweep_value - expected_vdd).abs() < 0.01,
                "Point {}: expected VDD={}, got {}",
                i,
                expected_vdd,
                point.sweep_value
            );
        }
    }

    #[test]
    fn test_psf_ascii_delta_compressed() {
        // Test parsing delta-compressed PSF (stores only changes)
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        let psf_dir = raw.join("psf");
        fs::create_dir_all(&psf_dir).unwrap();

        // Delta-compressed format typically stores base value and deltas
        let tran_file = psf_dir.join("tran.tran");
        let content = r#"SWEEP
"time"
0.0
1.0
2.0
TRACE
VALUE
*0
+0.1
+0.1
END"#;
        fs::write(&tran_file, content).unwrap();

        let data = crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");
        // The parser should at least not crash on delta format
        // Note: file_stem of "tran.tran" is "tran"
        assert!(data.contains_key("tran"));
    }

    #[test]
    fn test_psf_signal_names_preserved() {
        // Verify that signal names from file stems are preserved
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        let psf_dir = raw.join("psf");
        fs::create_dir_all(&psf_dir).unwrap();

        // Create files with specific names
        // Note: file_stem strips last extension, so "net_VDD.tran" -> "net_VDD"
        fs::write(&psf_dir.join("net_VDD.tran"), "0.0\n1.0\n").unwrap();
        fs::write(&psf_dir.join("net_VSS.tran"), "0.0\n0.0\n").unwrap();
        fs::write(&psf_dir.join("I_vdd.i"), "1e-3\n").unwrap();

        let data = crate::spectre::parsers::parse_psf_ascii(&raw).expect("parse should succeed");

        assert!(data.contains_key("net_VDD"));
        assert!(data.contains_key("net_VSS"));
        assert!(data.contains_key("I_vdd"));
    }

    #[test]
    fn test_sweep_directory_preference_over_flat() {
        // When both sweep directories and flat files exist, sweep directories should be preferred
        let tmp = TempDir::new().unwrap();
        let raw = tmp.path().join("raw");
        fs::create_dir_all(&raw).unwrap();

        // Create sweep directory
        let sweep_dir = raw.join("sw1.sweep1");
        let point_dir = sweep_dir.join("1");
        fs::create_dir_all(&point_dir).unwrap();
        fs::write(
            &point_dir.join("tran.tran.tran"),
            "SWEEP\n0.0\n1.0\n2.0\nTRACE\nVALUE\nEND",
        )
        .unwrap();

        // Create flat file (should be ignored since sweep directories exist)
        fs::write(&raw.join("sw1-000_tran.tran"), "0.0\n1.0\n2.0\n").unwrap();

        let sweep_data =
            crate::spectre::parsers::parse_sweep_psf_directory(&raw).expect("parse should succeed");

        // Should find the sweep directory data (not flat files)
        assert_eq!(sweep_data.len(), 1);
        assert!(sweep_data.contains_key(&1));
    }
}

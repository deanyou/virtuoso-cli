//! Exercise the actual CLI/RPC boundary without a live Virtuoso process.
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

struct Bridge {
    port: u16,
    requests: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    dir: tempfile::TempDir,
}

impl Bridge {
    fn new(response: &'static [u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let (tx, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // BSD sockets may inherit the listener's nonblocking flag.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut data = String::new();
                        stream.read_to_string(&mut data).unwrap();
                        if data.is_empty() {
                            continue;
                        }
                        let request: Value = serde_json::from_str(&data).unwrap();
                        tx.send(request["skill"].as_str().unwrap().to_owned())
                            .unwrap();
                        stream.write_all(response).unwrap();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            }
        });
        Self {
            port,
            requests,
            stop,
            worker: Some(worker),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, capability: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vcli"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("VB_HOME", self.dir.path())
            .env("VB_REMOTE_HOST", "localhost")
            .env("VB_PORT", self.port.to_string())
            .env("VCLI_CAPABILITY", capability)
            .current_dir(self.dir.path())
            .args(["--format", "json"])
            .args(args)
            .output()
            .unwrap()
    }

    fn file(&self) -> std::path::PathBuf {
        let path = self.dir.path().join("probe with spaces.ils");
        std::fs::write(&path, "procedure(vcliTestProbe() 42)\nt\n").unwrap();
        path.canonicalize().unwrap()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.expect("mock bridge thread failed");
        }
    }
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn admin_exec_and_eval_send_load_to_bridge() {
    for op in ["exec", "eval"] {
        let bridge = Bridge::new(b"\x02t");
        let code = "load(\"/remote/probe.il\")";
        assert_success(&bridge.run("admin", &["skill", op, code]));
        let sent = bridge
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(sent.contains(code), "{sent}");
    }
}

#[test]
fn admin_readonly_still_rejects_load_before_sending() {
    let bridge = Bridge::new(b"\x02t");
    let out = bridge.run(
        "admin",
        &["skill", "exec", "load(\"/remote/probe.il\")", "--readonly"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("blocked"));
    assert!(bridge.requests.try_recv().is_err());
}

#[test]
fn load_checks_admin_before_accessing_the_file() {
    let bridge = Bridge::new(b"\x02t");
    let out = bridge.run("cell", &["skill", "load", "missing.il"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not permitted"),
        "{:?}",
        out
    );
    assert!(bridge.requests.try_recv().is_err());
}

#[test]
fn local_load_uses_original_absolute_path_and_preserves_ils() {
    let bridge = Bridge::new(b"\x02t");
    let file = bridge.file();
    assert_success(&bridge.run("admin", &["skill", "load", "probe with spaces.ils"]));
    let sent = bridge
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let escaped = virtuoso_cli::client::bridge::escape_skill_string(file.to_str().unwrap());
    assert!(sent.contains(&format!("\"{escaped}\"")), "{sent}");
}

#[cfg(unix)]
#[test]
fn local_load_preserves_a_symlinks_language_suffix_and_escapes_path() {
    let bridge = Bridge::new(b"\x02t");
    let target = bridge.dir.path().join("target.il");
    std::fs::write(&target, "t\n").unwrap();
    let link = bridge.dir.path().join("probe \"quoted\".ils");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = bridge.run("admin", &["skill", "load", link.to_str().unwrap()]);
    assert_success(&out);
    let sent = bridge
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let escaped = virtuoso_cli::client::bridge::escape_skill_string(link.to_str().unwrap());
    assert_eq!(sent, format!("load(\"{escaped}\")"));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["loaded_path"], link.to_str().unwrap());
}

#[test]
fn rpc_load_success_reports_the_execution_host_path() {
    let bridge = Bridge::new(b"\x02t");
    let file = bridge.file();
    let params = serde_json::json!({"path": file}).to_string();
    let out = bridge.run(
        "admin",
        &["rpc", "call", "--method", "skill.load", "--params", &params],
    );
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["loaded_path"], file.to_str().unwrap());
}

#[test]
fn cli_and_rpc_load_propagate_nil_and_skill_errors() {
    for response in [b"\x02nil".as_slice(), b"\x15syntax error".as_slice()] {
        for rpc in [false, true] {
            let bridge = Bridge::new(response);
            let file = bridge.file();
            let params = serde_json::json!({"path": file}).to_string();
            let args = if rpc {
                vec!["rpc", "call", "--method", "skill.load", "--params", &params]
            } else {
                vec!["skill", "load", file.to_str().unwrap()]
            };
            let out = bridge.run("admin", &args);
            assert!(!out.status.success(), "load must fail: {:?}", out);
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("execution_failed"),
                "{:?}",
                out
            );
            assert!(
                bridge.requests.try_recv().is_ok(),
                "load never reached bridge: {:?}",
                out
            );
        }
    }
}

#[test]
fn raw_nil_remains_a_valid_expression_result() {
    let bridge = Bridge::new(b"\x02nil");
    let out = bridge.run("admin", &["skill", "exec", "nil"]);
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["output"], "nil");
}

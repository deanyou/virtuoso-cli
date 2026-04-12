use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::TunnelState;
use crate::transport::ssh::SSHRunner;
use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

static RESOURCES: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources");

/// Verify that a PID belongs to an SSH process by checking /proc/<pid>/cmdline.
/// Returns false if the process doesn't exist or isn't SSH (PID reuse protection).
fn verify_ssh_pid(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let cmdline_path = format!("/proc/{pid}/cmdline");
        if let Ok(cmdline) = std::fs::read_to_string(&cmdline_path) {
            cmdline.contains("ssh")
        } else {
            false
        }
    }
    #[cfg(not(unix))]
    {
        true // no /proc on non-unix, fall back to trusting PID
    }
}

pub struct SSHClient {
    pub runner: SSHRunner,
    pub port: u16,
    pub keep_remote_files: bool,
    tunnel_pid: Option<u32>,
}

impl SSHClient {
    pub fn from_env(keep_remote_files: bool) -> Result<Self> {
        let cfg = Config::from_env()?;
        let mut runner = SSHRunner::new(cfg.remote_host.as_deref().unwrap_or(""));
        if let Some(ref user) = cfg.remote_user {
            runner = runner.with_user(user);
        }
        if let Some(ref jump) = cfg.jump_host {
            let mut r = runner.with_jump(jump);
            if let Some(ref user) = cfg.jump_user {
                r.jump_user = Some(user.clone());
            }
            runner = r;
        }
        runner.ssh_port = cfg.ssh_port;
        runner.ssh_key_path = cfg.ssh_key.clone();

        Ok(Self {
            runner,
            port: cfg.port,
            keep_remote_files,
            tunnel_pid: None,
        })
    }

    pub fn warm(&mut self, _timeout: Option<u64>) -> Result<()> {
        self.ensure_remote_setup()?;
        self.ensure_tunnel()?;
        self.save_state()?;
        tracing::info!("tunnel established on port {}", self.port);
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        if let Some(pid) = self.tunnel_pid {
            #[cfg(unix)]
            {
                if verify_ssh_pid(pid) {
                    let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                } else {
                    tracing::warn!("PID {pid} is not an SSH process, skipping kill");
                }
            }
            #[cfg(not(unix))]
            {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
            tracing::info!("killed tunnel process {}", pid);
        }

        if !self.keep_remote_files {
            self.cleanup_remote()?;
        }

        TunnelState::clear().ok();
        Ok(())
    }

    pub fn saved_port(&self) -> Option<u16> {
        TunnelState::load().ok().flatten().map(|s| s.port)
    }

    pub fn is_tunnel_alive(&self) -> bool {
        if let Some(pid) = self.tunnel_pid {
            #[cfg(unix)]
            {
                verify_ssh_pid(pid) && unsafe { libc::kill(pid as i32, 0) == 0 }
            }
            #[cfg(not(unix))]
            {
                Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output()
                    .is_err()
            }
        } else {
            false
        }
    }

    pub fn upload_file(&self, local: &str, remote: &str) -> Result<()> {
        self.runner.upload(local, remote)
    }

    pub fn download_file(&self, remote: &str, local: &str) -> Result<()> {
        self.runner.download(remote, local)
    }

    pub fn upload_text(&self, text: &str, remote: &str) -> Result<()> {
        self.runner.upload_text(text, remote)
    }

    pub fn run_command(&self, cmd: &str) -> Result<crate::models::RemoteTaskResult> {
        self.runner.run_command(cmd, None)
    }

    fn ensure_remote_setup(&self) -> Result<String> {
        let python = self.runner.detect_python()?;

        let setup_dir = "/tmp/virtuoso_bridge";
        self.runner
            .run_command(&format!("mkdir -p {setup_dir}"), None)?;

        let daemon_path = if let Some(ref py) = python {
            if py.contains("2.7") {
                self.deploy_daemon_27(setup_dir)?
            } else {
                self.deploy_daemon_3(setup_dir)?
            }
        } else {
            self.deploy_rust_daemon(setup_dir)?
        };

        let il_path = self.deploy_il_script(setup_dir, &daemon_path, python.as_deref())?;

        tracing::info!(
            "remote setup complete: daemon={}, il={}",
            daemon_path,
            il_path
        );
        Ok(il_path)
    }

    fn ensure_tunnel(&mut self) -> Result<()> {
        for port in self.port..(self.port + 10) {
            if self.try_ssh_tunnel(port).is_ok() {
                self.port = port;
                return Ok(());
            }
        }
        Err(VirtuosoError::Ssh(
            "failed to establish tunnel on any port".into(),
        ))
    }

    fn try_ssh_tunnel(&mut self, port: u16) -> Result<()> {
        let target = self.runner.remote_target();
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-f",
            "-N",
            "-L",
            &format!("127.0.0.1:{port}:127.0.0.1:{port}"),
            &target,
        ]);

        // ControlMaster: share tunnel SSH connection with command sessions
        let control_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("virtuoso_bridge")
            .join("ssh");
        let _ = std::fs::create_dir_all(&control_dir);
        let control_path = control_dir.join("%h-%p-%r");
        cmd.args([
            "-o",
            "ControlMaster=auto",
            "-o",
            &format!("ControlPath={}", control_path.display()),
            "-o",
            "ControlPersist=600",
        ]);

        if let Some(p) = self.runner.ssh_port {
            cmd.arg("-p").arg(p.to_string());
        }
        if let Some(ref key) = self.runner.ssh_key_path {
            cmd.arg("-i").arg(key);
        }
        if let Some(ref jump) = self.runner.jump_host {
            cmd.arg("-J").arg(jump);
        }

        let output = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VirtuosoError::Ssh(format!("failed to start tunnel: {e}")))?;

        let pid = output.id();
        self.tunnel_pid = Some(pid);

        use std::net::TcpStream;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                return Ok(());
            }
        }
        Err(VirtuosoError::Ssh("tunnel port not reachable".into()))
    }

    fn save_state(&self) -> Result<()> {
        let state = TunnelState {
            version: 1,
            port: self.port,
            pid: self.tunnel_pid.unwrap_or(0),
            remote_host: self.runner.host.clone(),
            setup_path: Some("/tmp/virtuoso_bridge".into()),
        };
        state.save().map_err(|e| VirtuosoError::Ssh(e.to_string()))
    }

    fn deploy_daemon_3(&self, setup_dir: &str) -> Result<String> {
        let path = format!("{setup_dir}/ramic_bridge_daemon_3.py");
        let content = RESOURCES
            .get_file("daemons/ramic_bridge_daemon_3.py")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| {
                VirtuosoError::Ssh("ramic_bridge_daemon_3.py not found in resources".into())
            })?;

        self.runner.upload_text(content, &path)?;
        Ok(path)
    }

    fn deploy_daemon_27(&self, setup_dir: &str) -> Result<String> {
        let path = format!("{setup_dir}/ramic_bridge_daemon_27.py");
        let content = RESOURCES
            .get_file("daemons/ramic_bridge_daemon_27.py")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| {
                VirtuosoError::Ssh("ramic_bridge_daemon_27.py not found in resources".into())
            })?;

        self.runner.upload_text(content, &path)?;
        Ok(path)
    }

    fn deploy_rust_daemon(&self, setup_dir: &str) -> Result<String> {
        let arch = self.runner.detect_arch()?;
        let binary_name = match arch.as_str() {
            "x86_64" => "virtuoso-daemon-x86_64",
            "aarch64" => "virtuoso-daemon-aarch64",
            _ => {
                return Err(VirtuosoError::Ssh(format!(
                    "unsupported architecture: {arch}"
                )))
            }
        };

        let path = format!("{setup_dir}/{binary_name}");

        let embedded = RESOURCES
            .get_file(format!("daemons/{binary_name}"))
            .ok_or_else(|| {
                VirtuosoError::Ssh(format!("{binary_name} not found in resources, build with: cargo build --features daemon --release && cp target/release/virtuoso-daemon resources/daemons/{binary_name}"))
            })?;

        let content = embedded.contents();
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| VirtuosoError::Ssh(format!("temp file failed: {e}")))?;
        tmp.as_file()
            .write_all(content)
            .map_err(|e| VirtuosoError::Ssh(format!("write temp failed: {e}")))?;

        self.runner.upload(tmp.path().to_str().unwrap(), &path)?;
        self.runner.run_command(&format!("chmod +x {path}"), None)?;

        Ok(path)
    }

    fn deploy_il_script(
        &self,
        setup_dir: &str,
        daemon_path: &str,
        python: Option<&str>,
    ) -> Result<String> {
        let il_content = RESOURCES
            .get_file("ramic_bridge.il")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| VirtuosoError::Ssh("ramic_bridge.il not found in resources".into()))?;

        let il_content = il_content
            .replace("__DAEMON_PATH__", daemon_path)
            .replace("__PYTHON_CMD__", python.unwrap_or(""));

        let path = format!("{setup_dir}/ramic_bridge.il");
        self.runner.upload_text(&il_content, &path)?;
        Ok(path)
    }

    fn cleanup_remote(&self) -> Result<()> {
        self.runner
            .run_command("rm -rf /tmp/virtuoso_bridge", None)?;
        Ok(())
    }
}

pub fn file_md5(path: &str) -> Result<String> {
    let content =
        fs::read(path).map_err(|e| VirtuosoError::Config(format!("failed to read file: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

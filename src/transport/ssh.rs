#![allow(dead_code)]

use crate::error::{Result, VirtuosoError};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::models::RemoteTaskResult;

fn shell_quote(s: &str) -> String {
    shlex::try_quote(s)
        .unwrap_or(std::borrow::Cow::Borrowed(s))
        .into_owned()
}

pub struct SSHRunner {
    pub host: String,
    pub user: Option<String>,
    pub jump_host: Option<String>,
    pub jump_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_key_path: Option<String>,
    pub ssh_config_path: Option<String>,
    pub timeout: u64,
    pub connect_timeout: u64,
    pub verbose: bool,
    /// Dynamically disabled when a ControlMaster failure is detected at runtime.
    pub use_control_master: Cell<bool>,
}

impl SSHRunner {
    pub fn new(host: &str) -> Self {
        Self {
            host: host.into(),
            user: None,
            jump_host: None,
            jump_user: None,
            ssh_port: None,
            ssh_key_path: None,
            ssh_config_path: None,
            timeout: 30,
            connect_timeout: 10,
            verbose: false,
            use_control_master: Cell::new(true),
        }
    }

    /// Detect ControlMaster failure patterns in SSH stderr output.
    /// These typically appear on Windows/WSL2 when the CM socket path contains
    /// non-ASCII characters or the named pipe cannot be created.
    pub fn is_cm_failure(stderr: &str) -> bool {
        stderr.contains("mux_client_request_session")
            || stderr.contains("could not create named pipe")
            || stderr.contains("ControlPath")
            || stderr.contains("Control socket connect")
            || stderr.contains("multiplexing not supported")
            || stderr.contains("mux_client_hello_exchange")
    }

    pub fn with_jump(mut self, jump: &str) -> Self {
        self.jump_host = Some(jump.into());
        self
    }

    pub fn with_user(mut self, user: &str) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn test_connection(&self, timeout: Option<u64>) -> Result<bool> {
        let effective_timeout = timeout.unwrap_or(self.connect_timeout);
        let output = self.run_test_connection(effective_timeout)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if Self::is_cm_failure(&stderr) && self.use_control_master.get() {
                tracing::warn!(
                    "ControlMaster failure in test_connection, retrying without CM: {}",
                    stderr.lines().next().unwrap_or("")
                );
                self.use_control_master.set(false);
                let output2 = self.run_test_connection(effective_timeout)?;
                return Ok(output2.status.success());
            }
        }
        Ok(output.status.success())
    }

    fn run_test_connection(&self, connect_timeout: u64) -> Result<std::process::Output> {
        let mut cmd = self.build_ssh_cmd_with_timeout(connect_timeout);
        cmd.arg("exit").arg("0");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| VirtuosoError::Ssh(format!("failed to run ssh: {e}")))
    }

    pub fn run_command(&self, command: &str, timeout: Option<u64>) -> Result<RemoteTaskResult> {
        let result = self.run_command_inner(command, timeout)?;
        if !result.success && Self::is_cm_failure(&result.stderr) && self.use_control_master.get() {
            tracing::warn!(
                "ControlMaster failure detected, retrying without CM: {}",
                result.stderr.lines().next().unwrap_or("")
            );
            self.use_control_master.set(false);
            return self.run_command_inner(command, timeout);
        }
        Ok(result)
    }

    fn run_command_inner(&self, command: &str, timeout: Option<u64>) -> Result<RemoteTaskResult> {
        let _timeout = timeout.unwrap_or(self.timeout);
        let start = Instant::now();

        let mut cmd = self.build_ssh_cmd();
        // -l: login shell — sources /etc/profile and ~/.profile so PATH is
        // populated correctly on EDA hosts where the login shell is csh/tcsh.
        cmd.arg("sh").arg("-l").arg("-s");

        let output = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VirtuosoError::Ssh(format!("failed to spawn ssh: {e}")))?;

        if let Some(mut stdin) = output.stdin.as_ref() {
            stdin
                .write_all(command.as_bytes())
                .map_err(|e| VirtuosoError::Ssh(format!("failed to write command: {e}")))?;
        }

        let output = output
            .wait_with_output()
            .map_err(|e| VirtuosoError::Ssh(format!("ssh failed: {e}")))?;

        let elapsed = start.elapsed().as_secs_f64();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let error = if output.status.success() {
            None
        } else {
            Some(self.summarize_error(&stderr))
        };

        let mut timings = HashMap::new();
        timings.insert("total".into(), elapsed);

        Ok(RemoteTaskResult {
            success: output.status.success(),
            returncode: output.status.code().unwrap_or(-1),
            stdout,
            stderr,
            remote_dir: None,
            error,
            timings,
        })
    }

    pub fn upload(&self, local: &str, remote: &str) -> Result<()> {
        let _target = self.remote_target();

        let status = Command::new("tar")
            .arg("cf")
            .arg("-")
            .arg("-C")
            .arg(
                std::path::Path::new(local)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            )
            .arg(std::path::Path::new(local).file_name().unwrap_or_default())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| VirtuosoError::Ssh(format!("tar failed: {e}")))?;

        let tar_stdout = status.stdout.unwrap();

        let remote_dir = std::path::Path::new(remote)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_string_lossy();

        let mut ssh = self.build_ssh_cmd();
        let quoted_dir = shell_quote(&remote_dir);
        // Must pass "sh -c 'command'" as a single argument to SSH,
        // otherwise "sh", "-c", "command" are concatenated without quotes,
        // breaking commands with &&.
        let inner_cmd = format!("mkdir -p {quoted_dir} && cd {quoted_dir} && tar xf -");
        ssh.arg(format!("sh -c {}", shell_quote(&inner_cmd)));
        ssh.stdin(tar_stdout);

        let output = ssh
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| VirtuosoError::Ssh(format!("ssh upload failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VirtuosoError::Ssh(format!("upload failed: {stderr}")));
        }

        Ok(())
    }

    pub fn upload_text(&self, text: &str, remote: &str) -> Result<()> {
        let remote_dir = std::path::Path::new(remote)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_string_lossy();

        let quoted_dir = shell_quote(&remote_dir);
        let mkdir_cmd = format!("mkdir -p {quoted_dir}");
        let mkdir = self.run_command(&mkdir_cmd, None)?;
        if !mkdir.success {
            return Err(VirtuosoError::Ssh(format!(
                "failed to create remote dir: {}",
                mkdir.stderr
            )));
        }

        let mut cmd = self.build_ssh_cmd();
        let quoted_remote = shell_quote(remote);
        // Must pass "sh -c 'command'" as a single argument to SSH,
        // otherwise "sh", "-c", "command" are concatenated without quotes,
        // breaking commands with &&.
        cmd.arg(format!(
            "sh -c {}",
            shell_quote(&format!("cat > {quoted_remote}"))
        ));

        let output = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VirtuosoError::Ssh(format!("ssh failed: {e}")))?;

        if let Some(mut stdin) = output.stdin.as_ref() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| VirtuosoError::Ssh(format!("write failed: {e}")))?;
        }

        let output = output
            .wait_with_output()
            .map_err(|e| VirtuosoError::Ssh(format!("upload failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VirtuosoError::Ssh(format!("upload failed: {stderr}")));
        }

        Ok(())
    }

    pub fn download(&self, remote: &str, local: &str) -> Result<()> {
        let _target = self.remote_target();

        let local_path = std::path::Path::new(local);
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VirtuosoError::Ssh(format!("failed to create local dir: {e}")))?;
        }

        let mut cmd = self.build_ssh_cmd();
        cmd.arg("cat").arg(remote);

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| VirtuosoError::Ssh(format!("ssh download failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VirtuosoError::Ssh(format!("download failed: {stderr}")));
        }

        std::fs::write(local, output.stdout)
            .map_err(|e| VirtuosoError::Ssh(format!("failed to write local file: {e}")))?;

        Ok(())
    }

    pub fn detect_python(&self) -> Result<Option<String>> {
        for py in &["python3", "python", "python2.7"] {
            let result = self.run_command(&format!("which {py} 2>/dev/null"), None)?;
            if result.success && !result.stdout.trim().is_empty() {
                return Ok(Some(py.to_string()));
            }
        }
        Ok(None)
    }

    pub fn detect_arch(&self) -> Result<String> {
        let result = self.run_command("uname -m", None)?;
        if result.success {
            Ok(result.stdout.trim().to_string())
        } else {
            Err(VirtuosoError::Ssh(format!(
                "failed to detect arch: {}",
                result.stderr
            )))
        }
    }

    pub(crate) fn build_ssh_cmd(&self) -> Command {
        self.build_ssh_cmd_with_timeout(self.connect_timeout)
    }

    fn build_ssh_cmd_with_timeout(&self, connect_timeout: u64) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("ConnectTimeout={connect_timeout}"),
            // EDA lab KDC stalls masquerade as banner-exchange timeouts;
            // disable both auth methods we never use.
            "-o",
            "GSSAPIAuthentication=no",
            "-o",
            "HostbasedAuthentication=no",
        ]);

        if self.use_control_master.get() {
            // ControlMaster: reuse SSH connections to avoid repeated handshakes.
            // Disabled at runtime if a CM failure is detected (WSL2/Windows paths
            // with non-ASCII characters, named pipe creation failures, etc.).
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
        }

        if let Some(port) = self.ssh_port {
            cmd.arg("-p").arg(port.to_string());
        }
        if let Some(ref key) = self.ssh_key_path {
            cmd.arg("-i").arg(key);
        }
        if let Some(ref config) = self.ssh_config_path {
            cmd.arg("-F").arg(config);
        }
        if let Some(ref jump) = self.jump_host {
            let jump_target = match &self.jump_user {
                Some(u) => format!("{u}@{jump}"),
                None => jump.clone(),
            };
            cmd.arg("-J").arg(jump_target);
        }

        cmd.arg(self.remote_target());
        cmd
    }

    pub fn remote_target(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// Build a minimal SSH command string for manual connectivity verification.
    /// Useful for error messages when the tunnel cannot be established.
    pub fn verify_cmd_hint(&self) -> String {
        let mut parts = vec!["ssh".to_string()];
        if let Some(ref jump) = self.jump_host {
            let jump_target = match &self.jump_user {
                Some(u) => format!("{u}@{jump}"),
                None => jump.clone(),
            };
            parts.push(format!("-J {jump_target}"));
        }
        if let Some(port) = self.ssh_port {
            parts.push(format!("-p {port}"));
        }
        if let Some(ref key) = self.ssh_key_path {
            parts.push(format!("-i {key}"));
        }
        parts.push(self.remote_target());
        parts.join(" ")
    }

    pub(crate) fn summarize_error(&self, stderr: &str) -> String {
        let lower = stderr.to_lowercase();
        if lower.contains("connection refused") {
            "connection refused - check if SSH is running".into()
        } else if lower.contains("authentication") || lower.contains("permission denied") {
            "authentication failed - check SSH keys".into()
        } else if lower.contains("timeout") || lower.contains("timed out") {
            "connection timed out - check network".into()
        } else if lower.contains("could not resolve") {
            "hostname resolution failed - check DNS".into()
        } else {
            stderr.lines().take(3).collect::<Vec<_>>().join("; ")
        }
    }
}

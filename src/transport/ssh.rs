use crate::error::{Result, VirtuosoError};
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
    pub ssh_key_path: Option<String>,
    pub ssh_config_path: Option<String>,
    pub timeout: u64,
    pub connect_timeout: u64,
    pub verbose: bool,
}

impl SSHRunner {
    pub fn new(host: &str) -> Self {
        Self {
            host: host.into(),
            user: None,
            jump_host: None,
            jump_user: None,
            ssh_key_path: None,
            ssh_config_path: None,
            timeout: 30,
            connect_timeout: 10,
            verbose: false,
        }
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
        let _timeout = timeout.unwrap_or(self.connect_timeout);
        let mut cmd = self.build_ssh_cmd();
        cmd.arg("exit").arg("0");

        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| VirtuosoError::Ssh(format!("failed to run ssh: {e}")))?;

        Ok(output.status.success())
    }

    pub fn run_command(&self, command: &str, timeout: Option<u64>) -> Result<RemoteTaskResult> {
        let _timeout = timeout.unwrap_or(self.timeout);
        let start = Instant::now();

        let mut cmd = self.build_ssh_cmd();
        cmd.arg("sh").arg("-s");

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
        let _stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();

        let mut timings = HashMap::new();
        timings.insert("total".into(), elapsed);

        Ok(RemoteTaskResult {
            success: output.status.success(),
            returncode: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: stderr_str.clone(),
            remote_dir: None,
            error: if output.status.success() {
                None
            } else {
                Some(self.summarize_error(&stderr_str))
            },
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
        ssh.arg("sh").arg("-c").arg(format!(
            "mkdir -p {quoted_dir} && cd {quoted_dir} && tar xf -"
        ));
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
        let mkdir = self.run_command(&format!("mkdir -p {quoted_dir}"), None)?;
        if !mkdir.success {
            return Err(VirtuosoError::Ssh(format!(
                "failed to create remote dir: {}",
                mkdir.stderr
            )));
        }

        let mut cmd = self.build_ssh_cmd();
        let quoted_remote = shell_quote(remote);
        cmd.arg("sh")
            .arg("-c")
            .arg(format!("cat > {quoted_remote}"));

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
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("ConnectTimeout={}", self.connect_timeout),
        ]);

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

#![allow(dead_code)]

use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, SimulationResult};
use crate::spectre::jobs::{Job, JobStatus};
use crate::transport::ssh::SSHRunner;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use uuid::Uuid;

pub struct SpectreSimulator {
    pub spectre_cmd: String,
    pub spectre_args: Vec<String>,
    pub timeout: u64,
    pub work_dir: PathBuf,
    pub output_format: String,
    pub remote: bool,
    pub ssh_runner: Option<SSHRunner>,
    pub remote_work_dir: Option<String>,
    pub keep_remote_files: bool,
}

impl SpectreSimulator {
    pub fn from_env() -> Result<Self> {
        let cfg = crate::config::Config::from_env()?;
        let remote = cfg.is_remote();

        let ssh_runner = if remote {
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
            Some(runner)
        } else {
            None
        };

        Ok(Self {
            spectre_cmd: cfg.spectre_cmd,
            spectre_args: cfg.spectre_args,
            timeout: cfg.timeout,
            work_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            output_format: "psfascii".into(),
            remote,
            ssh_runner,
            remote_work_dir: None,
            keep_remote_files: cfg.keep_remote_files,
        })
    }

    pub fn run_simulation(
        &self,
        netlist: &str,
        params: Option<&HashMap<String, String>>,
    ) -> Result<SimulationResult> {
        if self.remote {
            self.run_remote(netlist, params)
        } else {
            self.run_local(netlist, params)
        }
    }

    pub fn check_license(&self) -> Result<String> {
        if let Some(ref runner) = self.ssh_runner {
            let cmds = vec![
                "which spectre 2>/dev/null || echo 'not found'",
                "spectre -W 2>/dev/null | head -1 || echo 'unknown'",
                "lmstat -a 2>/dev/null | grep -i spectre | head -5 || echo 'lmstat not available'",
            ];

            let mut results = Vec::new();
            for cmd in cmds {
                let result = runner.run_command(cmd, None)?;
                results.push(result.stdout.trim().to_string());
            }
            Ok(results.join("\n"))
        } else {
            let output = Command::new("sh")
                .arg("-c")
                .arg("which spectre 2>/dev/null && spectre -W 2>/dev/null | head -1")
                .output()
                .map_err(|e| VirtuosoError::Execution(e.to_string()))?;
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }

    /// Launch simulation in background, return job ID immediately.
    /// Works for both local and remote (via SSH nohup).
    pub fn run_async(&self, netlist: &str) -> Result<Job> {
        if self.remote {
            return self.run_async_remote(netlist);
        }

        let run_id = Uuid::new_v4().to_string()[..8].to_string();
        let run_dir = self.work_dir.join(&run_id);
        fs::create_dir_all(&run_dir).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let netlist_path = run_dir.join("input.scs");
        fs::write(&netlist_path, netlist).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let raw_dir = run_dir.join("raw");
        fs::create_dir_all(&raw_dir).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let log_path = run_dir.join("spectre.out");

        let mut cmd = Command::new(&self.spectre_cmd);
        cmd.arg("-64")
            .arg(&netlist_path)
            .arg("+escchars")
            .arg("+log")
            .arg(&log_path)
            .arg("-format")
            .arg(&self.output_format)
            .arg("-raw")
            .arg(&raw_dir)
            .arg("+lqtimeout")
            .arg("900")
            .arg("-maxw")
            .arg("5")
            .arg("-maxn")
            .arg("5")
            .arg("+logstatus");

        for arg in &self.spectre_args {
            cmd.arg(arg);
        }

        let child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| VirtuosoError::Execution(format!("spectre failed to start: {e}")))?;

        let job = Job {
            id: run_id,
            status: JobStatus::Running,
            netlist_path: netlist_path.to_string_lossy().into(),
            raw_dir: Some(raw_dir.to_string_lossy().into()),
            pid: Some(child.id()),
            created: chrono::Local::now().to_rfc3339(),
            finished: None,
            error: None,
            remote_host: None,
            remote_dir: None,
        };
        job.save()?;
        // Process runs detached — status checked lazily via Job::refresh()
        Ok(job)
    }

    fn run_async_remote(&self, netlist: &str) -> Result<Job> {
        let runner = self.ssh_runner.as_ref().ok_or_else(|| {
            VirtuosoError::Execution("no SSH runner for remote async simulation".into())
        })?;

        let run_id = Uuid::new_v4().to_string()[..8].to_string();
        let remote_dir = format!("/tmp/virtuoso_bridge/spectre/{run_id}");

        // Create dir + upload netlist
        runner.run_command(&format!("mkdir -p {remote_dir}"), None)?;
        runner.upload_text(netlist, &format!("{remote_dir}/input.scs"))?;

        // Build spectre command
        let extra = if self.spectre_args.is_empty() {
            String::new()
        } else {
            format!(" {}", self.spectre_args.join(" "))
        };
        // Source login profile for PATH + license env (non-interactive SSH lacks them).
        // Covers bash (.bash_profile/.bashrc) and sh (.profile).
        // Use VB_SPECTRE_CMD=<absolute path> if spectre is still not found.
        let spectre_cmd = format!(
            ". /etc/profile 2>/dev/null; . ~/.bash_profile 2>/dev/null; . ~/.bashrc 2>/dev/null; \
             cd {remote_dir} && nohup {cmd} -64 input.scs +escchars +log spectre.out \
             -format {fmt} -raw raw +lqtimeout 900 -maxw 5 -maxn 5 +logstatus{extra} \
             > /dev/null 2>&1 & echo $!",
            cmd = self.spectre_cmd,
            fmt = self.output_format,
        );

        // Launch and capture PID
        let result = runner.run_command(&spectre_cmd, Some(10))?;
        let pid: u32 = result
            .stdout
            .trim()
            .parse()
            .map_err(|_| VirtuosoError::Execution(format!("bad PID: {}", result.stdout)))?;

        let job = Job {
            id: run_id,
            status: JobStatus::Running,
            netlist_path: format!("{remote_dir}/input.scs"),
            raw_dir: Some(format!("{remote_dir}/raw")),
            pid: Some(pid),
            created: chrono::Local::now().to_rfc3339(),
            finished: None,
            error: None,
            remote_host: Some(runner.remote_target()),
            remote_dir: Some(remote_dir),
        };
        job.save()?;
        Ok(job)
    }

    fn run_local(
        &self,
        netlist: &str,
        _params: Option<&HashMap<String, String>>,
    ) -> Result<SimulationResult> {
        let run_id = Uuid::new_v4().to_string();
        let run_dir = self.work_dir.join(&run_id);
        fs::create_dir_all(&run_dir).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let netlist_path = run_dir.join("input.scs");
        fs::write(&netlist_path, netlist).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let raw_dir = run_dir.join("raw");
        fs::create_dir_all(&raw_dir).map_err(|e| VirtuosoError::Execution(e.to_string()))?;

        let log_path = run_dir.join("spectre.out");

        let mut cmd = Command::new(&self.spectre_cmd);
        cmd.arg("-64")
            .arg(&netlist_path)
            .arg("+escchars")
            .arg("+log")
            .arg(&log_path)
            .arg("-format")
            .arg(&self.output_format)
            .arg("-raw")
            .arg(&raw_dir)
            .arg("+lqtimeout")
            .arg("900")
            .arg("-maxw")
            .arg("5")
            .arg("-maxn")
            .arg("5")
            .arg("+logstatus");

        for arg in &self.spectre_args {
            cmd.arg(arg);
        }

        let mut child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| VirtuosoError::Execution(format!("spectre failed to start: {e}")))?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(self.timeout);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(VirtuosoError::Timeout(self.timeout));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(e) => {
                    return Err(VirtuosoError::Execution(format!(
                        "failed to wait on spectre: {e}"
                    )))
                }
            }
        };

        let log_content = fs::read_to_string(&log_path).unwrap_or_default();

        if !status.success() {
            return Ok(SimulationResult {
                status: ExecutionStatus::Error,
                tool_version: None,
                data: HashMap::new(),
                errors: vec![format!("spectre exited with code {:?}", status.code())],
                warnings: Vec::new(),
                metadata: [("log".into(), log_content)].into_iter().collect(),
            });
        }

        let data = if raw_dir.join("psf").exists() || raw_dir.join("results").exists() {
            crate::spectre::parsers::parse_psf_ascii(&raw_dir)?
        } else {
            HashMap::new()
        };

        Ok(SimulationResult {
            status: ExecutionStatus::Success,
            tool_version: None,
            data,
            errors: Vec::new(),
            warnings: Vec::new(),
            metadata: [("log".into(), log_content), ("run_id".into(), run_id)]
                .into_iter()
                .collect(),
        })
    }

    fn run_remote(
        &self,
        netlist: &str,
        _params: Option<&HashMap<String, String>>,
    ) -> Result<SimulationResult> {
        let runner = self.ssh_runner.as_ref().ok_or_else(|| {
            VirtuosoError::Execution("no SSH runner available for remote simulation".into())
        })?;

        let run_id = Uuid::new_v4().to_string();
        let remote_dir = format!("/tmp/virtuoso_bridge/spectre/{run_id}");

        runner.run_command(&format!("mkdir -p {remote_dir}"), None)?;

        let netlist_content = netlist.to_string();
        runner.upload_text(&netlist_content, &format!("{remote_dir}/input.scs"))?;

        let spectre_cmd = if self.spectre_args.is_empty() {
            format!(
                "{cmd} -64 input.scs +escchars +log spectre.out -format {fmt} -raw raw +lqtimeout 900 -maxw 5 -maxn 5 +logstatus",
                cmd = self.spectre_cmd,
                fmt = self.output_format
            )
        } else {
            format!(
                "{cmd} -64 input.scs +escchars +log spectre.out -format {fmt} -raw raw +lqtimeout 900 -maxw 5 -maxn 5 +logstatus {}",
                self.spectre_args.join(" "),
                cmd = self.spectre_cmd,
                fmt = self.output_format
            )
        };

        let sim_cmd = format!("cd {remote_dir} && {spectre_cmd}");
        let result = runner.run_command(&sim_cmd, Some(self.timeout * 2))?;

        let mut sim_result = SimulationResult {
            status: if result.success {
                ExecutionStatus::Success
            } else {
                ExecutionStatus::Error
            },
            tool_version: None,
            data: HashMap::new(),
            errors: if result.success {
                Vec::new()
            } else {
                vec![result.stderr.clone()]
            },
            warnings: Vec::new(),
            metadata: [
                ("run_id".into(), run_id.clone()),
                ("remote_dir".into(), remote_dir.clone()),
            ]
            .into_iter()
            .collect(),
        };

        if result.success {
            let local_raw = self.work_dir.join(&run_id).join("raw");
            runner.download(&format!("{remote_dir}/raw"), local_raw.to_str().unwrap())?;

            if let Ok(data) = crate::spectre::parsers::parse_psf_ascii(&local_raw) {
                sim_result.data = data;
            }
        }

        if !self.keep_remote_files {
            runner.run_command(&format!("rm -rf {remote_dir}"), None)?;
        }

        Ok(sim_result)
    }
}

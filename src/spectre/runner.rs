#![allow(dead_code)]

use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, SimulationResult};
use crate::spectre::jobs::{Job, JobStatus};
use crate::streaming::{JobEvent, JobEventSink};
use crate::transport::ssh::SSHRunner;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
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
    sink: Arc<dyn JobEventSink>,
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
            sink: Arc::new(crate::streaming::NullSink),
        })
    }

    pub fn with_sink(mut self, sink: Arc<dyn JobEventSink>) -> Self {
        self.sink = sink;
        self
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
            // Combine into one SSH round-trip to avoid repeated handshakes.
            let cmd = "which spectre 2>/dev/null || echo 'not found'; \
                       spectre -W 2>/dev/null | head -1 || echo 'unknown'; \
                       lmstat -a 2>/dev/null | grep -i spectre | head -5 || echo 'lmstat not available'";
            let result = runner.run_command(cmd, None)?;
            Ok(result.stdout.trim().to_string())
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

        // Emit started event
        self.sink.emit(JobEvent::Started {
            job_id: run_id.clone(),
            created: chrono::Local::now().to_rfc3339(),
        });

        let mut child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| VirtuosoError::Execution(format!("spectre failed to start: {e}")))?;

        let sink = Arc::clone(&self.sink);
        let log_path_for_thread = log_path.clone();
        let job_id_for_thread = run_id.clone();

        // Spawn progress tracking thread using tokio runtime
        let progress_thread = thread::spawn(move || {
            let rt = crate::async_runtime::runtime();
            rt.block_on(async move {
                let poll_interval = Duration::from_millis(500);
                let mut interval = tokio::time::interval(poll_interval);
                loop {
                    interval.tick().await;

                    // Parse log for progress
                    if let Ok(content) = fs::read_to_string(&log_path_for_thread) {
                        // Check if simulation is done
                        if content.contains("Simulation completed")
                            || content.contains("成就")
                            || content.contains("Error:")
                            || content.contains("Failed")
                        {
                            break;
                        }

                        if let Some((percent, message)) = parse_spectre_progress(&content) {
                            sink.emit(JobEvent::Progress {
                                job_id: job_id_for_thread.clone(),
                                percent,
                                message,
                                iteration: None,
                                time_point: None,
                            });
                        }
                    }
                }
            });
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(self.timeout);
        let start_time = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = progress_thread.join();
                        return Err(VirtuosoError::Timeout(self.timeout));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(e) => {
                    let _ = progress_thread.join();
                    return Err(VirtuosoError::Execution(format!(
                        "failed to wait on spectre: {e}"
                    )));
                }
            }
        };

        // Wait for progress thread to finish
        let _ = progress_thread.join();

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let log_content = fs::read_to_string(&log_path).unwrap_or_default();

        if !status.success() {
            self.sink.emit(JobEvent::Failed {
                job_id: run_id.clone(),
                error: format!("spectre exited with code {:?}", status.code()),
            });
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

        self.sink.emit(JobEvent::Completed {
            job_id: run_id.clone(),
            duration_ms,
            errors: Vec::new(),
        });

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

/// Parse spectre log output for progress information.
/// Returns (percent, message) if progress was found.
fn parse_spectre_progress(log_content: &str) -> Option<(f32, String)> {
    // Spectre outputs progress via +logstatus in formats like:
    // "Time: 1.23e-6 s" or "Time: 123.456u" or "Iteration: 1234"
    // We look for the last occurrence of these patterns.

    let lines: Vec<&str> = log_content.lines().collect();

    // Look for time progress patterns
    // Example: "Time: 1.23e-6 s" or "Time: 123.456u"
    let mut last_time: Option<String> = None;
    let mut last_iteration: Option<u64> = None;

    for line in lines.iter().rev() {
        let line = line.trim();

        // Match time patterns like "Time: 1.23e-6 s" or "Time: 123.456u"
        if let Some(time_val) = line.strip_prefix("Time:") {
            let time_val = time_val.trim().trim_end_matches('s').trim();
            last_time = Some(time_val.to_string());
        }

        // Match iteration patterns like "Iteration: 1234"
        if let Some(iter_str) = line.strip_prefix("Iteration:") {
            if let Ok(iter) = iter_str.trim().parse::<u64>() {
                last_iteration = Some(iter);
            }
        }

        // Once we have both time and iteration, we can stop
        if last_time.is_some() && last_iteration.is_some() {
            break;
        }
    }

    if let (Some(time), Some(iter)) = (last_time, last_iteration) {
        // Estimate progress based on typical simulation time
        // Fortran format output style: "t=1200u/3n" or similar
        let message = format!("t={}, iter={}", time, iter);

        // Try to extract a rough percentage from the message
        // Common patterns: "t=X/Y" where Y is the end time
        let percent = if let Some((current, total)) = parse_time_ratio(&message) {
            (current / total * 100.0).min(100.0) as f32
        } else {
            // Fallback: just use iteration count as a rough indicator
            // without knowing the total, we can only indicate activity
            50.0
        };

        return Some((percent, message));
    }

    // If no time/iteration found, check for generic progress indicators
    for line in lines.iter().rev() {
        let line = line.trim();
        if line.contains("Progress:") || line.contains("progress:") {
            if let Some(pct) = extract_percent(line) {
                return Some((pct, line.to_string()));
            }
        }
    }

    // Check for completion or error keywords
    if log_content.contains("Simulation completed") || log_content.contains("成就") {
        return Some((100.0, "completed".to_string()));
    }

    None
}

/// Parse time ratio from messages like "t=1200u/3n".
fn parse_time_ratio(msg: &str) -> Option<(f64, f64)> {
    // Look for pattern like "t=1200u/3n" or "t=1.2e-6/3e-6"
    if let Some(start) = msg.find("t=") {
        let after_t = &msg[start + 2..];
        if let Some(slash) = after_t.find('/') {
            let current = &after_t[..slash];
            let total = &after_t[slash + 1..];
            if let (Some(c), Some(t)) = (parse_time_value(current), parse_time_value(total)) {
                if t > 0.0 {
                    return Some((c, t));
                }
            }
        }
    }
    None
}

/// Parse a time value like "1200u", "3n", "1.2e-6" into f64 (in seconds).
fn parse_time_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0.0);
    }

    // Check for unit suffix
    let (num_str, unit) = if let Some(idx) =
        s.find(|c: char| !c.is_numeric() && c != '.' && c != '-' && c != 'e' && c != 'E')
    {
        (&s[..idx], &s[idx..])
    } else {
        (s, "")
    };

    let value: f64 = num_str.parse().ok()?;

    Some(match unit {
        "f" => value * 1e-15,
        "p" => value * 1e-12,
        "n" => value * 1e-9,
        "u" => value * 1e-6,
        "m" => value * 1e-3,
        "" => value,
        _ => value, // Assume base unit if unknown
    })
}

/// Extract percentage from a string like "Progress: 45%"
fn extract_percent(s: &str) -> Option<f32> {
    // Look for number followed by %
    if let Some(idx) = s.find(|c: char| c.is_numeric()) {
        let rest = &s[idx..];
        if let Some(end) = rest.find('%') {
            if let Ok(pct) = rest[..end].trim().parse::<f32>() {
                return Some(pct.min(100.0).max(0.0));
            }
        }
    }
    None
}

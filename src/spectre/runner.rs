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
    pub max_workers: u32,
    /// Path to Cadence environment setup file (VB_CADENCE_CSHRC).
    /// Source this before running spectre on remote SSH.
    pub cadence_cshrc: Option<String>,
    sink: Arc<dyn JobEventSink>,
}

/// Check if simulation output indicates a netlist read-in error.
/// "Circuit read-in complete" is NORMAL Spectre output.
/// Only flag actual errors: "error reading" or "read-in failed".
fn has_readin_error(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("error reading") || lower.contains("read-in failed")
}

/// Extract specific error messages from simulation log for better diagnostics.
fn extract_error_messages(content: &str) -> Vec<String> {
    let mut errors = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        // Match common Spectre error patterns
        if line.contains("Error") || line.contains("ERROR") || line.contains("error:") {
            // Skip benign messages
            if line.contains("No licences")
                || line.contains("Warning")
                || line.contains("info:")
                || line.contains("Reading")
            {
                continue;
            }
            errors.push(line.to_string());
        }
    }

    errors
}

/// Check if simulation output indicates a license error.
fn has_license_error(content: &str) -> bool {
    let lower = content.to_lowercase();
    (lower.contains("license") || lower.contains("licence"))
        && (lower.contains("error") || lower.contains("denied") || lower.contains("unavailable"))
}

/// Completion detection patterns (configurable for i18n).
/// Default: English "Simulation completed" and Chinese "成就".
fn is_simulation_complete(content: &str) -> bool {
    content.contains("Simulation completed")
        || content.contains("成就")
        || std::env::var("VB_SPECTRE_COMPLETION_PATTERN")
            .map(|p| content.contains(&p))
            .unwrap_or(false)
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
            max_workers: cfg.spectre_max_workers,
            cadence_cshrc: cfg.cadence_cshrc,
            sink: Arc::new(crate::streaming::NullSink),
        })
    }

    pub fn with_sink(mut self, sink: Arc<dyn JobEventSink>) -> Self {
        self.sink = sink;
        self
    }

    /// Build a command prefix that sources the Cadence environment.
    /// Returns "" if no cshrc is configured, otherwise returns
    /// "source /path/to/cshrc && " for csh or ". /path/to/cshrc && " for bash.
    fn env_prefix(&self) -> String {
        if let Some(ref cshrc) = self.cadence_cshrc {
            // Try to detect shell type - default to csh for Cadence tools
            if cshrc.ends_with(".csh") || cshrc.ends_with(".cshrc") {
                format!("source {cshrc} && ")
            } else {
                format!(". {cshrc} && ")
            }
        } else {
            String::new()
        }
    }

    /// Run a command with Cadence environment sourced (for remote SSH).
    fn run_with_env(
        &self,
        runner: &SSHRunner,
        cmd: &str,
        timeout: Option<u64>,
    ) -> crate::error::Result<crate::models::RemoteTaskResult> {
        let prefix = self.env_prefix();
        let full_cmd = format!("{prefix}{cmd}");
        runner.run_command(&full_cmd, timeout)
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
            // Build command with environment sourced
            let check_cmd = "which spectre 2>/dev/null || echo 'not found'; \
                       spectre -W 2>/dev/null | head -1 || echo 'unknown'; \
                       lmstat -a 2>/dev/null | grep -i spectre | head -5 || echo 'lmstat not available'";
            // Use env_prefix for SSH commands to load Cadence environment
            let prefix = self.env_prefix();
            let full_cmd = format!("{prefix}{check_cmd}");
            let result = runner.run_command(&full_cmd, None)?;
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
            .arg(self.max_workers.to_string())
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

        // Build spectre command with Cadence environment sourced
        let extra = if self.spectre_args.is_empty() {
            String::new()
        } else {
            format!(" {}", self.spectre_args.join(" "))
        };
        // Source login profile for PATH + license env (non-interactive SSH lacks them).
        // Use VB_CADENCE_CSHRC to source Cadence environment if configured.
        let env_prefix = self.env_prefix();
        let spectre_cmd = format!(
            ". /etc/profile 2>/dev/null; . ~/.bash_profile 2>/dev/null; . ~/.bashrc 2>/dev/null; \
             cd {remote_dir} && {env_prefix}nohup {cmd} -64 input.scs +escchars +log spectre.out \
             -format {fmt} -raw raw +lqtimeout 900 -maxw 5 -maxn {maxn} +logstatus{extra} \
             > /dev/null 2>&1 & echo $!",
            cmd = self.spectre_cmd,
            fmt = self.output_format,
            maxn = self.max_workers,
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
            .arg(self.max_workers.to_string())
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
                        if is_simulation_complete(&content)
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

        // Check for read-in errors (but not "Circuit read-in complete" which is normal)
        let readin_errors = extract_error_messages(&log_content)
            .into_iter()
            .filter(|e| !e.contains("Circuit read-in complete"))
            .collect::<Vec<_>>();

        if !status.success() || has_readin_error(&log_content) {
            self.sink.emit(JobEvent::Failed {
                job_id: run_id.clone(),
                error: format!(
                    "spectre exited with code {:?} or netlist read error",
                    status.code()
                ),
            });

            let errors = if readin_errors.is_empty() {
                vec![format!("spectre exited with code {:?}", status.code())]
            } else {
                readin_errors
            };

            return Ok(SimulationResult {
                status: ExecutionStatus::Error,
                tool_version: None,
                data: HashMap::new(),
                errors,
                warnings: Vec::new(),
                metadata: [("log".into(), log_content)].into_iter().collect(),
            });
        }

        // Parse results - try sweep output first, then regular PSF
        let data = if let Ok(sweep) = crate::spectre::parsers::parse_sweep_psf_directory(&raw_dir) {
            // Convert sweep data to regular HashMap for compatibility
            // Each sweep point's data is stored as "point_<N>_<signal>" keys
            let mut flat: HashMap<String, Vec<f64>> = HashMap::new();
            let mut indices: Vec<_> = sweep.keys().collect();
            indices.sort();
            for &idx in indices {
                for (signal, values) in &sweep[&idx] {
                    let key = format!("point_{}_{}", idx, signal);
                    flat.insert(key, values.clone());
                }
            }
            flat
        } else if raw_dir.join("psf").exists() || raw_dir.join("results").exists() {
            crate::spectre::parsers::parse_psf_ascii(&raw_dir)?
        } else {
            HashMap::new()
        };

        // Extract warnings (but not the benign "Circuit read-in complete")
        let warnings: Vec<String> = extract_error_messages(&log_content)
            .into_iter()
            .filter(|e| e.contains("Warning") || e.contains("warning") || e.contains("WARNING"))
            .collect();

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
            warnings,
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

        let extra = if self.spectre_args.is_empty() {
            String::new()
        } else {
            format!(" {}", self.spectre_args.join(" "))
        };
        let spectre_cmd = format!(
            "{cmd} -64 input.scs +escchars +log spectre.out -format {fmt} -raw raw +lqtimeout 900 -maxw 5 -maxn {maxn} +logstatus{extra}",
            cmd = self.spectre_cmd,
            fmt = self.output_format,
            maxn = self.max_workers,
        );

        // Build command with Cadence environment sourced for remote execution
        let env_prefix = self.env_prefix();
        let sim_cmd = format!("cd {remote_dir} && {env_prefix}{spectre_cmd}");
        let result = runner.run_command(&sim_cmd, Some(self.timeout * 2))?;

        // Check for read-in errors in remote output
        let combined_output = format!("{}\n{}", result.stdout, result.stderr);
        let readin_errors: Vec<String> = extract_error_messages(&combined_output)
            .into_iter()
            .filter(|e| !e.contains("Circuit read-in complete"))
            .collect();

        let has_error = !result.success || has_readin_error(&combined_output);
        let error_detail = if readin_errors.is_empty() {
            if !result.success {
                result.stderr.clone()
            } else {
                String::new()
            }
        } else {
            readin_errors.join("; ")
        };

        let mut sim_result = SimulationResult {
            status: if has_error {
                ExecutionStatus::Error
            } else {
                ExecutionStatus::Success
            },
            tool_version: None,
            data: HashMap::new(),
            errors: if error_detail.is_empty() {
                Vec::new()
            } else {
                vec![error_detail]
            },
            warnings: Vec::new(),
            metadata: [
                ("run_id".into(), run_id.clone()),
                ("remote_dir".into(), remote_dir.clone()),
            ]
            .into_iter()
            .collect(),
        };

        if !has_error {
            let local_raw = self.work_dir.join(&run_id).join("raw");
            runner.download(&format!("{remote_dir}/raw"), local_raw.to_str().unwrap())?;

            // Try sweep output first, then regular PSF
            if let Ok(sweep) = crate::spectre::parsers::parse_sweep_psf_directory(&local_raw) {
                let mut flat: HashMap<String, Vec<f64>> = HashMap::new();
                let mut indices: Vec<_> = sweep.keys().collect();
                indices.sort();
                for &idx in indices {
                    for (signal, values) in &sweep[&idx] {
                        let key = format!("point_{}_{}", idx, signal);
                        flat.insert(key, values.clone());
                    }
                }
                sim_result.data = flat;
            } else if let Ok(data) = crate::spectre::parsers::parse_psf_ascii(&local_raw) {
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
    if is_simulation_complete(log_content) {
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
                return Some(pct.clamp(0.0, 100.0));
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // === Error detection tests ===

    #[test]
    fn test_has_readin_error_actual_error() {
        let content = "Error reading netlist: undefined module 'resistor'\nCircuit read-in failed";
        assert!(has_readin_error(content));
    }

    #[test]
    fn test_has_readin_error_case_insensitive() {
        let content = "ERROR READING netlist\nread-in failed";
        assert!(has_readin_error(content));
    }

    #[test]
    fn test_has_readin_error_benign_complete() {
        // "Circuit read-in complete" is NORMAL output, not an error
        let content = "Circuit read-in complete\nSimulation running...";
        assert!(!has_readin_error(content));
    }

    #[test]
    fn test_has_readin_error_empty() {
        let content = "";
        assert!(!has_readin_error(content));
    }

    #[test]
    fn test_has_license_error() {
        let content = "License error: spectre is not available\nERROR: No licences";
        assert!(has_license_error(content));
    }

    #[test]
    fn test_has_license_error_british_spelling() {
        let content = "Licence error: spectre denied";
        assert!(has_license_error(content));
    }

    #[test]
    fn test_has_license_error_only_word() {
        // "license" alone without error/denied/unavailable is not an error
        let content = "Using license file: /path/to/license.dat";
        assert!(!has_license_error(content));
    }

    #[test]
    fn test_extract_error_messages() {
        let content = "Warning: something unusual\nError: netlist parse failed\ninfo: reading file\nERROR: undefined module";
        let errors = extract_error_messages(content);
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("Error: netlist parse failed"));
        assert!(errors[1].contains("ERROR: undefined module"));
    }

    #[test]
    fn test_extract_error_messages_skips_benign() {
        let content = "Info: Reading design\nNo licences found for abc\nWarning: deprecated syntax";
        let errors = extract_error_messages(content);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_extract_error_messages_empty() {
        let content = "";
        let errors = extract_error_messages(content);
        assert!(errors.is_empty());
    }

    // === Completion detection tests ===

    #[test]
    fn test_is_simulation_complete_english() {
        let content = "Simulation completed successfully";
        assert!(is_simulation_complete(content));
    }

    #[test]
    fn test_is_simulation_complete_chinese() {
        let content = "仿真成就完成";
        assert!(is_simulation_complete(content));
    }

    #[test]
    fn test_is_simulation_complete_not_complete() {
        let content = "Simulation running...\nTime: 1e-6 s";
        assert!(!is_simulation_complete(content));
    }

    // === Progress parsing tests ===

    #[test]
    fn test_parse_time_value_simple() {
        assert_eq!(parse_time_value("1.5"), Some(1.5));
        assert_eq!(parse_time_value("0"), Some(0.0));
    }

    #[test]
    fn test_parse_time_value_with_units() {
        // Use approximate comparison for floating point
        fn approx_eq(a: Option<f64>, b: f64) -> bool {
            a.map(|v| (v - b).abs() < 1e-12).unwrap_or(false)
        }
        assert!(approx_eq(parse_time_value("1.5n"), 1.5e-9));
        assert!(approx_eq(parse_time_value("100u"), 100e-6));
        assert!(approx_eq(parse_time_value("2m"), 2e-3));
        assert!(approx_eq(parse_time_value("3p"), 3e-12));
        assert!(approx_eq(parse_time_value("1.5f"), 1.5e-15));
    }

    #[test]
    fn test_parse_time_value_scientific() {
        assert_eq!(parse_time_value("1e-9"), Some(1e-9));
        assert_eq!(parse_time_value("1.5e-6"), Some(1.5e-6));
    }

    #[test]
    fn test_parse_time_value_empty() {
        assert_eq!(parse_time_value(""), Some(0.0));
        assert_eq!(parse_time_value("   "), Some(0.0));
    }

    #[test]
    fn test_parse_time_ratio() {
        let msg = "t=1200u/3n";
        let (current, total) = parse_time_ratio(msg).unwrap();
        assert!((current - 1200e-6).abs() < 1e-12);
        assert!((total - 3e-9).abs() < 1e-12);
    }

    #[test]
    fn test_parse_time_ratio_scientific() {
        let msg = "t=1.2e-6/3e-6";
        let (current, total) = parse_time_ratio(msg).unwrap();
        assert!((current - 1.2e-6).abs() < 1e-12);
        assert!((total - 3e-6).abs() < 1e-12);
    }

    #[test]
    fn test_parse_time_ratio_no_slash() {
        let msg = "t=1e-6";
        assert!(parse_time_ratio(msg).is_none());
    }

    #[test]
    fn test_parse_time_ratio_zero_total() {
        let msg = "t=1u/0";
        assert!(parse_time_ratio(msg).is_none());
    }

    #[test]
    fn test_extract_percent() {
        assert_eq!(extract_percent("Progress: 45%"), Some(45.0));
        assert_eq!(extract_percent("Load: 100% complete"), Some(100.0));
    }

    #[test]
    fn test_extract_percent_clamped() {
        assert_eq!(extract_percent("Over: 150% done"), Some(100.0));
        // Note: negative percentages don't have practical meaning in simulation logs
        // The function only looks for numeric chars, so "-10%" returns 10 (not -10)
        assert_eq!(extract_percent("Under: -10% done"), Some(10.0));
    }

    #[test]
    fn test_extract_percent_no_percent() {
        assert!(extract_percent("no percentage here").is_none());
    }

    #[test]
    fn test_parse_spectre_progress_time_and_iteration() {
        let log = r#"Simulation running...
Time: 1.2e-6 s
Iteration: 1234
Loading..."#;
        let result = parse_spectre_progress(log);
        assert!(result.is_some());
        let (percent, msg) = result.unwrap();
        assert!(msg.contains("1.2e-6"));
        assert!(msg.contains("1234"));
        // With t=X/Y parsing, should estimate percentage
        assert!(percent >= 0.0 && percent <= 100.0);
    }

    #[test]
    fn test_parse_spectre_progress_with_ratio() {
        let log = r#"Time: t=500u/1m
Iteration: 500"#;
        let result = parse_spectre_progress(log);
        assert!(result.is_some());
        let (percent, _) = result.unwrap();
        // 500u / 1m = 0.5 = 50%
        assert!((percent - 50.0).abs() < 1.0);
    }

    #[test]
    fn test_parse_spectre_progress_completed() {
        let log = "Simulation completed successfully";
        let result = parse_spectre_progress(log);
        assert!(result.is_some());
        let (percent, msg) = result.unwrap();
        assert_eq!(percent, 100.0);
        assert_eq!(msg, "completed");
    }

    #[test]
    fn test_parse_spectre_progress_progress_keyword() {
        let log = "Progress: 75%";
        let result = parse_spectre_progress(log);
        assert!(result.is_some());
        let (percent, _) = result.unwrap();
        assert_eq!(percent, 75.0);
    }

    #[test]
    fn test_parse_spectre_progress_no_progress() {
        let log = "Starting simulation...";
        let result = parse_spectre_progress(log);
        assert!(result.is_none());
    }
}

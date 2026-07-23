#![allow(dead_code)]

use crate::error::{Result, VirtuosoError};
use crate::models::{ExecutionStatus, SimulationResult};
use crate::spectre::jobs::{Job, JobStatus};
use crate::streaming::{JobEvent, JobEventSink};
use crate::transport::ssh::{shell_quote, SSHRunner};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// Result of a parallel Spectre run — label plus outcome.
#[derive(Debug)]
pub struct ParallelSimResult {
    pub label: String,
    pub result: Result<SimulationResult>,
}

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
    /// Absolute path to Spectre binary (VB_SPECTRE_BIN).
    /// When set, this path is used directly instead of relying on PATH.
    pub spectre_bin: Option<String>,
    sink: Arc<dyn JobEventSink>,
}

impl Clone for SpectreSimulator {
    fn clone(&self) -> Self {
        Self {
            spectre_cmd: self.spectre_cmd.clone(),
            spectre_args: self.spectre_args.clone(),
            timeout: self.timeout,
            work_dir: self.work_dir.clone(),
            output_format: self.output_format.clone(),
            remote: self.remote,
            ssh_runner: self.ssh_runner.clone(),
            remote_work_dir: self.remote_work_dir.clone(),
            keep_remote_files: self.keep_remote_files,
            max_workers: self.max_workers,
            cadence_cshrc: self.cadence_cshrc.clone(),
            spectre_bin: self.spectre_bin.clone(),
            sink: Arc::clone(&self.sink),
        }
    }
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

fn build_local_license_command(spectre: &str) -> Command {
    let mut command = Command::new(spectre);
    command.arg("-W");
    command
}

fn build_remote_license_command(cadence_cshrc: Option<&str>, spectre: &str) -> String {
    let spectre = shell_quote(spectre);
    let probe = format!(
        "command -v {spectre} 2>/dev/null || echo 'not found'; \
         {spectre} -W 2>/dev/null | head -1 || echo 'unknown'; \
         lmstat -a 2>/dev/null | grep -i spectre | head -5 || echo 'lmstat not available'"
    );

    match cadence_cshrc {
        Some(cshrc) if cshrc.ends_with(".csh") || cshrc.ends_with(".cshrc") => {
            // SSHRunner feeds commands to `sh -l -s`; source the csh file only
            // inside an explicitly invoked csh, then execute the POSIX probe.
            let csh_script = format!(
                "source {} && exec sh -c {}",
                shell_quote(cshrc),
                shell_quote(&probe)
            );
            format!("csh -c {}", shell_quote(&csh_script))
        }
        Some(cshrc) => format!(". {} && ({probe})", shell_quote(cshrc)),
        None => probe,
    }
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

// ─────────────────────────────────────────────────────────────────────────────
// SpectreOutcomeClassifier — unified failure classification
// ─────────────────────────────────────────────────────────────────────────────

/// Classification outcome for a Spectre run.
#[derive(Debug, Clone, PartialEq)]
pub enum SpectreOutcome {
    /// Spectre succeeded cleanly.
    Success,
    /// Spectre produced usable data but logged warnings or license deprecation.
    PartialSuccess { warnings: Vec<String> },
    /// Spectre failed — no usable data, explicit failure in log.
    Failure { reason: String },
    /// Spectre ran but left incomplete/partial data (convergence failure, etc.).
    PartialFailure { reason: String },
}

impl SpectreOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(
            self,
            SpectreOutcome::Success | SpectreOutcome::PartialSuccess { .. }
        )
    }

    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            SpectreOutcome::Failure { .. } | SpectreOutcome::PartialFailure { .. }
        )
    }

    fn is_partial(&self) -> bool {
        matches!(
            self,
            SpectreOutcome::PartialSuccess { .. } | SpectreOutcome::PartialFailure { .. }
        )
    }

    fn execution_status(&self) -> ExecutionStatus {
        match self {
            SpectreOutcome::Success => ExecutionStatus::Success,
            SpectreOutcome::PartialSuccess { .. } => ExecutionStatus::Partial,
            SpectreOutcome::Failure { .. } => ExecutionStatus::Error,
            SpectreOutcome::PartialFailure { .. } => ExecutionStatus::Partial,
        }
    }

    fn short_reason(&self) -> String {
        match self {
            SpectreOutcome::Success => String::new(),
            SpectreOutcome::PartialSuccess { warnings } => {
                if warnings.is_empty() {
                    String::new()
                } else {
                    format!("{} warning(s)", warnings.len())
                }
            }
            SpectreOutcome::Failure { reason } | SpectreOutcome::PartialFailure { reason } => {
                reason.clone()
            }
        }
    }
}

/// Unified Spectre log classifier.
/// Replaces the ad-hoc `!status.success() || has_readin_error()` checks
/// in run_local, run_remote, and run_parallel.
#[derive(Debug, Clone, Default)]
pub struct SpectreOutcomeClassifier {
    pub exit_ok: bool,
    pub log_content: String,
    pub has_raw_data: bool,
}

impl SpectreOutcomeClassifier {
    pub fn new(exit_ok: bool, log_content: &str, has_raw_data: bool) -> Self {
        Self {
            exit_ok,
            log_content: log_content.to_string(),
            has_raw_data,
        }
    }

    /// Classify the Spectre run outcome from the given log.
    ///
    /// Detection rules (checked in order):
    /// 1. Explicit fatal / panic / SEV → Failure
    /// 2. Missing include / netlist error → Failure
    /// 3. Convergence failure (SPCRTRF-*, "failed to converge") → PartialFailure
    ///    (even if exit code is 0)
    /// 4. Read-in error → Failure
    /// 5. License error → PartialFailure (data may still be usable)
    /// 6. Exit code != 0 → Failure
    /// 7. "No convergence difficulties" alone is NOT a failure signal
    /// 8. If we have raw data and none of the above → Success
    /// 9. If we have raw data but there are warnings → PartialSuccess
    /// 10. If no raw data and no explicit errors → Failure (silent exit)
    pub fn classify(&self) -> SpectreOutcome {
        let lc = &self.log_content;

        // 1. Fatal / panic / SEV
        if self.has_fatal_error(lc) {
            return SpectreOutcome::Failure {
                reason: self.fatal_reason(lc),
            };
        }

        // 2. Missing include / netlist errors
        if self.has_missing_include(lc) {
            return SpectreOutcome::Failure {
                reason: "missing netlist include or library".to_string(),
            };
        }

        // 3. Convergence failure (SPCRTRF-*, explicit "failed to converge")
        //    These can occur even with exit code 0 — must be checked regardless.
        if self.has_convergence_failure(lc) {
            if self.has_raw_data {
                return SpectreOutcome::PartialFailure {
                    reason: self.convergence_reason(lc),
                };
            } else {
                return SpectreOutcome::Failure {
                    reason: self.convergence_reason(lc),
                };
            }
        }

        // 4. Read-in error
        if has_readin_error(lc) {
            return SpectreOutcome::Failure {
                reason: "netlist read-in failed".to_string(),
            };
        }

        // 5. License error — not fatal, data may still be usable
        if has_license_error(lc) {
            if self.has_raw_data {
                return SpectreOutcome::PartialSuccess {
                    warnings: vec!["license error — simulation may be incomplete".to_string()],
                };
            } else {
                return SpectreOutcome::Failure {
                    reason: "license error — simulation did not run".to_string(),
                };
            }
        }

        // 6. Non-zero exit
        if !self.exit_ok {
            if self.has_raw_data {
                return SpectreOutcome::PartialFailure {
                    reason: format!("non-zero exit code{}", self.short_exit_reason()),
                };
            } else {
                return SpectreOutcome::Failure {
                    reason: format!("spectre exited with error{}", self.short_exit_reason()),
                };
            }
        }

        // 7. "No convergence difficulties" is BENIGN — not a failure
        //    (already handled by has_convergence_failure not matching it)

        // 8+9. Exit ok + no critical errors — classify based on data and warnings
        let warnings = self.warnings();
        if self.has_raw_data {
            if warnings.is_empty() {
                SpectreOutcome::Success
            } else {
                SpectreOutcome::PartialSuccess { warnings }
            }
        } else {
            // Silent exit — no data and no explicit error detected
            SpectreOutcome::Failure {
                reason: "spectre exited cleanly but produced no output data".to_string(),
            }
        }
    }

    /// "failed to converge" or SPCRTRF-* / SPCRTRF:*
    fn has_convergence_failure(&self, content: &str) -> bool {
        let lower = content.to_lowercase();
        // Explicit convergence failure messages
        lower.contains("failed to converge")
            || lower.contains("convergence failure")
            || lower.contains("failed during convergence")
            || lower.contains("do not converge")
            || lower.contains("could not converge")
            // SPCRTRF error codes: e.g., SPCRTRF-15044
            || Regex::new(r"SPCRTRF[:-]\d+").map(|re| re.is_match(content)).unwrap_or(false)
            // Spectre hierarchical error format: SPCRTRF_<number>
            || Regex::new(r"SPCRTRF_\d+")
                .map(|re| re.is_match(content))
                .unwrap_or(false)
    }

    fn convergence_reason(&self, content: &str) -> String {
        let lower = content.to_lowercase();
        if lower.contains("failed to converge") || lower.contains("convergence failure") {
            "convergence failure".to_string()
        } else if let Some(caps) = Regex::new(r"(SPCRTRF[:-]?\d+)")
            .ok()
            .and_then(|re| re.captures(content))
        {
            format!(
                "Spectre error {}: convergence failed",
                caps.get(0).unwrap().as_str()
            )
        } else {
            "convergence failure".to_string()
        }
    }

    /// Fatal: "fatal" near an error, SEV, panic, assertion failure.
    fn has_fatal_error(&self, content: &str) -> bool {
        let lower = content.to_lowercase();
        // "fatal" as a standalone word near error context
        if lower.contains("fatal") {
            return true;
        }
        // SEV (Spectre Enhanced Verifier) fatal errors
        if lower.contains("sev") && lower.contains("error") {
            return true;
        }
        // Panic strings
        if lower.contains("panic") || lower.contains("assertion") {
            return true;
        }
        // Coredump
        if lower.contains("coredump") || lower.contains("core dumped") {
            return true;
        }
        false
    }

    fn fatal_reason(&self, content: &str) -> String {
        let lower = content.to_lowercase();
        if lower.contains("panic") {
            return "spectre panic".to_string();
        }
        if lower.contains("assertion") {
            return "spectre assertion failure".to_string();
        }
        if lower.contains("coredump") || lower.contains("core dumped") {
            return "spectre coredump".to_string();
        }
        if lower.contains("sev") {
            return "SEV fatal error".to_string();
        }
        "fatal error".to_string()
    }

    /// Missing include / missing model file.
    fn has_missing_include(&self, content: &str) -> bool {
        let lower = content.to_lowercase();
        (lower.contains("missing include") || lower.contains("cannot open"))
            && (lower.contains(".scs")
                || lower.contains(".include")
                || lower.contains("file not found"))
    }

    fn short_exit_reason(&self) -> String {
        let re = Regex::new(r"exit code.*?(\d+)").ok();
        re.and_then(|re| re.captures(&self.log_content))
            .map(|c| format!(" (exit {})", c.get(1).unwrap().as_str()))
            .unwrap_or_default()
    }

    fn warnings(&self) -> Vec<String> {
        extract_error_messages(&self.log_content)
            .into_iter()
            .filter(|e| {
                let l = e.to_lowercase();
                l.contains("warning") || l.contains("warning:")
            })
            .collect()
    }
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
            spectre_bin: cfg.spectre_bin,
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

    /// Get the effective Spectre command to execute.
    /// Prefers spectre_bin (absolute path) over spectre_cmd (command name).
    fn spectre_command(&self) -> &str {
        self.spectre_bin.as_deref().unwrap_or(&self.spectre_cmd)
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

    /// Run multiple netlist simulations in parallel, returning results in input order.
    ///
    /// Each entry is `(label, netlist)` — results are returned with the same label.
    /// Worker count is capped at `self.max_workers` and `num_cpus::get()`.
    ///
    /// For remote mode, each worker gets its own `SSHRunner` clone so that
    /// `use_control_master` state is isolated per thread (avoids CM fallback races).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let inputs = vec![
    ///     ("tt".to_string(), netlist_tt),
    ///     ("ss".to_string(), netlist_ss),
    ///     ("ff".to_string(), netlist_ff),
    /// ];
    /// let results = sim.run_parallel(&inputs);
    /// for ParallelSimResult { label, result } in results {
    ///     match result {
    ///         Ok(sim_res) => println!("{}: OK", label),
    ///         Err(e) => println!("{}: ERROR - {}", label, e),
    ///     }
    /// }
    /// ```
    pub fn run_parallel(&self, inputs: &[(String, String)]) -> Vec<ParallelSimResult> {
        if inputs.is_empty() {
            return Vec::new();
        }

        let n_workers = self.max_workers.min(num_cpus::get() as u32).max(1) as usize;

        // Clone immutable config for each worker thread.
        let sim = self.clone();
        let inputs_arc: Vec<_> = inputs.iter().map(|(l, n)| (l.clone(), n.clone())).collect();

        std::thread::scope(|s| {
            let chunks: Vec<_> = inputs_arc
                .chunks(inputs_arc.len().div_ceil(n_workers))
                .collect();

            let handles: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    s.spawn(|| {
                        chunk
                            .iter()
                            .map(|(label, netlist)| {
                                let result = sim.run_simulation(netlist, None);
                                ParallelSimResult {
                                    label: label.clone(),
                                    result,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();

            let mut all_results = Vec::with_capacity(inputs_arc.len());
            for handle in handles {
                all_results.extend(handle.join().unwrap_or_default());
            }
            all_results
        })
    }

    /// Check if simulation output indicates a netlist read-in error.
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
            && (lower.contains("error")
                || lower.contains("denied")
                || lower.contains("unavailable"))
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

    pub fn check_license(&self) -> Result<String> {
        let spectre = self.spectre_command();
        if let Some(ref runner) = self.ssh_runner {
            let command = build_remote_license_command(self.cadence_cshrc.as_deref(), spectre);
            let result = runner.run_command(&command, None)?;
            Ok(result.stdout.trim().to_string())
        } else {
            let output = build_local_license_command(spectre)
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

        let mut cmd = Command::new(self.spectre_command());
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
            cmd = self.spectre_command(),
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

        let mut cmd = Command::new(self.spectre_command());
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

        // ── Classify outcome BEFORE data parsing ─────────────────────────────────
        // This replaces the old `!status.success() || has_readin_error()` check
        // and correctly detects convergence failures even when exit code is 0.
        let has_raw = raw_dir.join("psf").exists()
            || raw_dir.join("results").exists()
            || crate::spectre::parsers::parse_sweep_psf_directory(&raw_dir).is_ok();
        let classifier = SpectreOutcomeClassifier::new(status.success(), &log_content, has_raw);
        let outcome = classifier.classify();

        // ── Emit event based on classification ─────────────────────────────────────
        match &outcome {
            SpectreOutcome::Failure { reason } => {
                self.sink.emit(JobEvent::Failed {
                    job_id: run_id.clone(),
                    error: reason.clone(),
                });
            }
            SpectreOutcome::PartialFailure { reason } => {
                self.sink.emit(JobEvent::Failed {
                    job_id: run_id.clone(),
                    error: reason.clone(),
                });
            }
            SpectreOutcome::PartialSuccess { warnings } => {
                self.sink.emit(JobEvent::Completed {
                    job_id: run_id.clone(),
                    duration_ms,
                    errors: warnings.clone(),
                });
            }
            SpectreOutcome::Success => {
                self.sink.emit(JobEvent::Completed {
                    job_id: run_id.clone(),
                    duration_ms,
                    errors: Vec::new(),
                });
            }
        }

        // ── Parse results — always attempt even on failure (data may be partial) ──
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

        // Parse scalar operating-point / STRUCT blocks
        let operating_points = crate::spectre::parsers::parse_structured_op_blocks(&raw_dir);

        // ── Build result — status from classification, not raw exit code ─────────
        let (errors, warnings) = match &outcome {
            SpectreOutcome::Success => (Vec::new(), Vec::new()),
            SpectreOutcome::PartialSuccess { warnings } => (Vec::new(), warnings.clone()),
            SpectreOutcome::Failure { reason } => (vec![reason.clone()], Vec::new()),
            SpectreOutcome::PartialFailure { reason } => (Vec::new(), vec![reason.clone()]),
        };

        Ok(SimulationResult {
            status: outcome.execution_status(),
            tool_version: None,
            data,
            operating_points,
            errors,
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
            cmd = self.spectre_command(),
            fmt = self.output_format,
            maxn = self.max_workers,
        );

        // Build command with Cadence environment sourced for remote execution
        let env_prefix = self.env_prefix();
        let sim_cmd = format!("cd {remote_dir} && {env_prefix}{spectre_cmd}");
        let result = runner.run_command(&sim_cmd, Some(self.timeout * 2))?;

        // Classify outcome — replaces `!result.success || has_readin_error(...)`.
        // Detects convergence failures (SPCRTRF-*, "failed to converge") even with exit 0.
        let combined_output = format!("{}\n{}", result.stdout, result.stderr);

        // ── Attempt data download even on failure (convergence failure may leave partial data) ──
        let mut data = HashMap::new();
        let mut operating_points = HashMap::new();

        let local_raw = self.work_dir.join(&run_id).join("raw");
        if runner
            .download(&format!("{remote_dir}/raw"), local_raw.to_str().unwrap())
            .is_ok()
        {
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
                data = flat;
            } else if let Ok(parsed) = crate::spectre::parsers::parse_psf_ascii(&local_raw) {
                data = parsed;
            }
            operating_points = crate::spectre::parsers::parse_structured_op_blocks(&local_raw);
        }

        // ── Build result from classification ───────────────────────────────────────
        // Re-classify now that we know if raw data exists
        let outcome =
            SpectreOutcomeClassifier::new(result.success, &combined_output, !data.is_empty())
                .classify();

        let (errors, warnings, status) = match &outcome {
            SpectreOutcome::Success => (Vec::new(), Vec::new(), ExecutionStatus::Success),
            SpectreOutcome::PartialSuccess { warnings } => {
                (Vec::new(), warnings.clone(), ExecutionStatus::Partial)
            }
            SpectreOutcome::Failure { reason } => {
                (vec![reason.clone()], Vec::new(), ExecutionStatus::Error)
            }
            SpectreOutcome::PartialFailure { reason } => {
                (Vec::new(), vec![reason.clone()], ExecutionStatus::Partial)
            }
        };

        if !self.keep_remote_files {
            runner.run_command(&format!("rm -rf {remote_dir}"), None)?;
        }

        Ok(SimulationResult {
            status,
            tool_version: None,
            data,
            operating_points,
            errors,
            warnings,
            metadata: [
                ("run_id".into(), run_id.clone()),
                ("remote_dir".into(), remote_dir.clone()),
            ]
            .into_iter()
            .collect(),
        })
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
    fn local_license_check_uses_direct_spectre_argv() {
        let command = build_local_license_command("spectre; touch /tmp/pwned");
        assert_eq!(command.get_program(), "spectre; touch /tmp/pwned");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new("-W")]
        );
    }

    #[test]
    fn remote_license_check_quotes_environment_and_spectre_command() {
        let command = build_remote_license_command(
            Some("/cadence/env setup.csh"),
            "spectre; touch /tmp/pwned",
        );
        assert!(command.contains("source '/cadence/env setup.csh'"));
        assert!(command.contains("'spectre; touch /tmp/pwned' -W"));
        assert!(!command.contains("spectre; touch /tmp/pwned -W"));
    }

    #[test]
    fn remote_license_check_sources_sh_environment_in_login_shell() {
        let command = build_remote_license_command(Some("/cadence/env setup.sh"), "spectre");
        assert!(command.starts_with(". '/cadence/env setup.sh' &&"));
        assert!(command.contains("spectre -W"));
    }

    #[test]
    fn remote_license_check_runs_csh_environment_under_csh_with_quoted_values() {
        let command = build_remote_license_command(
            Some("/cadence/env; setup.csh"),
            "spectre; touch /tmp/pwned",
        );
        assert!(command.starts_with("csh -c "));
        assert!(command.contains("/cadence/env; setup.csh"));
        assert!(command.contains("spectre; touch /tmp/pwned"));
        assert!(!command.contains("source /cadence/env; setup.csh"));
        assert!(!command.contains("spectre; touch /tmp/pwned -W"));
        assert!(!command.starts_with("source "));
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
        assert!((0.0..=100.0).contains(&percent));
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

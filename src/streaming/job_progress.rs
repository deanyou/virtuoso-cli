//! Job progress tracker — parses spectre.out log files for progress updates.
//!
//! Used by the streaming system to emit progress events as simulations run.

use crate::streaming::{JobEvent, JobEventSink};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tracks progress of a running Spectre job by polling its log file.
pub struct JobProgressTracker {
    job_id: String,
    log_path: PathBuf,
    last_position: u64,
    last_size_check: Instant,
    total_iterations: Option<u64>,
}

impl JobProgressTracker {
    pub fn new(job_id: String, log_path: PathBuf) -> Self {
        Self {
            job_id,
            log_path,
            last_position: 0,
            last_size_check: Instant::now(),
            total_iterations: None,
        }
    }

    /// Poll the log file and emit progress events.
    /// Returns the latest progress event, or None if no new output.
    pub fn poll(&mut self) -> std::io::Result<Option<JobEvent>> {
        let metadata = fs::metadata(&self.log_path)?;
        let current_size = metadata.len();

        if current_size == self.last_position {
            return Ok(None);
        }

        let mut file = fs::File::open(&self.log_path)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(self.last_position))?;

        let mut new_content = String::new();
        use std::io::Read;
        file.read_to_string(&mut new_content)?;
        self.last_position = current_size;

        let event = Self::parse_log_content(&self.job_id, &new_content, current_size);
        Ok(Some(event))
    }

    fn parse_log_content(job_id: &str, content: &str, _size: u64) -> JobEvent {
        // Spectre progress patterns:
        // - "Simulation running" (initial)
        // - "Time: 1.234e-9 s" (time point progress)
        // - "Iteration 1234" (iteration count)
        // - "completes with 0 errors" (completion)
        // - "Error:" (failure)

        let lines: Vec<&str> = content.lines().collect();

        // Check for completion or errors first
        for line in lines.iter().rev() {
            let trimmed = line.trim();
            if trimmed.contains("completes with 0 errors") {
                return JobEvent::Completed {
                    job_id: job_id.to_string(),
                    duration_ms: 0, // Duration tracked separately
                    errors: Vec::new(),
                };
            }
            if trimmed.starts_with("Error") || trimmed.contains("error") {
                return JobEvent::Failed {
                    job_id: job_id.to_string(),
                    error: trimmed.to_string(),
                };
            }
        }

        // Extract progress from simulation output
        let mut percent = None;
        let mut message = String::new();
        let mut iteration = None;
        let mut time_point = None;

        for line in lines.iter().rev().take(20) {
            let trimmed = line.trim();
            // Look for iteration info: "Iteration 1234"
            if let Some(idx) = trimmed.find("Iteration ") {
                let rest = &trimmed[idx + 10..];
                if let Some(end) = rest.find(char::is_whitespace) {
                    if let Ok(n) = rest[..end].parse::<u64>() {
                        iteration = Some(n);
                        // Rough percent estimate based on iteration
                        if let Some(total) = self.total_iterations {
                            percent = Some((n as f32 / total as f32) * 100.0);
                        }
                    }
                }
            }
            // Look for time: "Time: 1.234e-6 s" or "t=1200u"
            if let Some(idx) = trimmed.find("Time: ") {
                let rest = &trimmed[idx + 6..];
                if let Some(end) = rest.find(' ') {
                    time_point = Some(rest[..end].to_string());
                    message = format!("t={}", rest[..end].to_string());
                }
            } else if let Some(idx) = trimmed.find("t=") {
                let rest = &trimmed[idx + 2..];
                if let Some(end_idx) = rest.find([' ', '\n', '\t']) {
                    time_point = Some(rest[..end_idx].to_string());
                    message = format!("t={}", rest[..end_idx].to_string());
                } else {
                    time_point = Some(rest.to_string());
                    message = format!("t={}", rest);
                }
            }
        }

        let percent = percent.unwrap_or(0.0);
        if message.is_empty() {
            message = "running".to_string();
        }

        JobEvent::Progress {
            job_id: job_id.to_string(),
            percent,
            message,
            iteration,
            time_point,
        }
    }

    /// Set total expected iterations for progress calculation.
    pub fn set_total_iterations(&mut self, total: u64) {
        self.total_iterations = Some(total);
    }
}

/// Spawn a background thread to poll and emit events.
pub fn spawn_progress_tracker(
    job_id: String,
    log_path: PathBuf,
    interval_ms: u64,
    sink: Arc<dyn JobEventSink>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut tracker = JobProgressTracker::new(job_id.clone(), log_path);
        let interval = Duration::from_millis(interval_ms);

        loop {
            std::thread::sleep(interval);

            match tracker.poll() {
                Ok(Some(event)) => {
                    sink.emit(event);
                    // Stop polling on terminal events
                    match &event {
                        JobEvent::Completed { .. }
                        | JobEvent::Failed { .. }
                        | JobEvent::Cancelled { .. } => break,
                        _ => {}
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    sink.emit(JobEvent::Failed {
                        job_id: job_id.clone(),
                        error: format!("log read error: {e}"),
                    });
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_completion() {
        let content = "Simulation started\nSome output\ncompletes with 0 errors\n";
        let event = JobProgressTracker::parse_log_content("test", content, 100);
        match event {
            JobEvent::Completed { job_id, .. } => {
                assert_eq!(job_id, "test");
            }
            _ => panic!("expected Completed event"),
        }
    }

    #[test]
    fn parse_failure() {
        let content = "Simulation started\nSome output\nError: invalid model parameter\n";
        let event = JobProgressTracker::parse_log_content("test", content, 100);
        match event {
            JobEvent::Failed { job_id, error } => {
                assert_eq!(job_id, "test");
                assert!(error.contains("Error"));
            }
            _ => panic!("expected Failed event"),
        }
    }

    #[test]
    fn parse_progress() {
        let content =
            "Simulation running\nTime: 1.234e-6 s\nIteration 500\nSome output\n";
        let event = JobProgressTracker::parse_log_content("test", content, 100);
        match event {
            JobEvent::Progress { job_id, percent, message, iteration, .. } => {
                assert_eq!(job_id, "test");
                assert!(percent >= 0.0);
                assert!(message.contains("1.234"));
                assert_eq!(iteration, Some(500));
            }
            _ => panic!("expected Progress event"),
        }
    }
}
use crate::error::{Result, VirtuosoError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub status: JobStatus,
    pub netlist_path: String,
    pub raw_dir: Option<String>,
    pub pid: Option<u32>,
    pub created: String,
    pub finished: Option<String>,
    pub error: Option<String>,
}

impl Job {
    fn dir() -> PathBuf {
        let dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("virtuoso_bridge")
            .join("jobs");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn path(id: &str) -> PathBuf {
        Self::dir().join(format!("{id}.json"))
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| VirtuosoError::Execution(e.to_string()))?;
        fs::write(Self::path(&self.id), json).map_err(|e| VirtuosoError::Execution(e.to_string()))
    }

    pub fn load(id: &str) -> Result<Self> {
        let path = Self::path(id);
        let json = fs::read_to_string(&path)
            .map_err(|_| VirtuosoError::NotFound(format!("job '{id}' not found")))?;
        serde_json::from_str(&json)
            .map_err(|e| VirtuosoError::Execution(format!("bad job file: {e}")))
    }

    pub fn list_all() -> Result<Vec<Self>> {
        let dir = Self::dir();
        let mut jobs = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Ok(json) = fs::read_to_string(&path) {
                        if let Ok(job) = serde_json::from_str::<Job>(&json) {
                            jobs.push(job);
                        }
                    }
                }
            }
        }
        jobs.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(jobs)
    }

    /// Check if a running job's process is still alive; update status if not.
    pub fn refresh(&mut self) -> Result<()> {
        if self.status != JobStatus::Running {
            return Ok(());
        }
        if let Some(pid) = self.pid {
            // Check if process exists via kill(pid, 0)
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            if !alive {
                // Process exited — check spectre.out for success/failure
                let log_dir = std::path::Path::new(&self.netlist_path)
                    .parent()
                    .unwrap_or(std::path::Path::new("."));
                let log = log_dir.join("spectre.out");
                if log.exists() {
                    let content = fs::read_to_string(&log).unwrap_or_default();
                    if content.contains("completes with 0 errors") {
                        self.status = JobStatus::Completed;
                    } else {
                        self.status = JobStatus::Failed;
                        // Extract last error line
                        self.error = content
                            .lines()
                            .rev()
                            .find(|l| l.contains("Error") || l.contains("error"))
                            .map(|l| l.trim().to_string());
                    }
                } else {
                    self.status = JobStatus::Failed;
                    self.error = Some("process exited, no log found".into());
                }
                self.finished = Some(chrono::Local::now().to_rfc3339());
                self.save()?;
            }
        }
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<()> {
        if self.status != JobStatus::Running {
            return Err(VirtuosoError::Config(format!(
                "job '{}' is not running (status: {:?})",
                self.id, self.status
            )));
        }
        if let Some(pid) = self.pid {
            // Kill process group
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        self.status = JobStatus::Cancelled;
        self.finished = Some(chrono::Local::now().to_rfc3339());
        self.save()
    }

    pub fn delete(id: &str) -> Result<()> {
        let path = Self::path(id);
        fs::remove_file(&path).map_err(|_| VirtuosoError::NotFound(format!("job '{id}' not found")))
    }
}

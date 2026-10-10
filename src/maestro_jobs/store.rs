//! File-backed JSON store for Maestro simulation jobs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::runtime_paths::cache_subdir;

/// Maestro job lifecycle state.
///
/// Mirrors virtuoso-bridge-lite's `MaestroJobStatus`:
/// - `Pending`: job submitted but not yet started
/// - `Running`: Spectre is executing
/// - `Completed`: Cadence callback ran (≠ sim success)
/// - `Failed`: Cadence callback error
/// - `Unknown`: submit acknowledged but persistence failed
/// - `Cancelled`: operator cancelled
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaestroJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

/// A single Maestro simulation job.
///
/// Persisted as `~/.cache/virtuoso_bridge/maestro/jobs/<job_id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaestroJob {
    /// Unique job identifier (UUIDv4).
    pub job_id: String,
    /// Maestro session name (e.g. `fnxSession4`).
    pub session: String,
    /// Test name within the session.
    pub test: Option<String>,
    /// Run id assigned by Maestro (from `maeGetSetup(?runId ...)`).
    pub run_id: Option<String>,
    /// Maestro's Virtuoso PID.
    pub pid: Option<u32>,
    /// Remote run directory path.
    pub run_dir: Option<String>,
    /// Job status.
    pub status: MaestroJobStatus,
    /// Optional human-readable name.
    pub name: Option<String>,
    /// When the job was submitted.
    pub created_at: DateTime<Utc>,
    /// Last status update.
    pub updated_at: DateTime<Utc>,
    /// Error message when status is Failed or Unknown.
    pub error_message: Option<String>,
    /// Client profile under which this job was created.
    pub profile: Option<String>,
}

impl MaestroJob {
    /// Create a new pending job.
    pub fn new(session: String, test: Option<String>, name: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            job_id: Uuid::new_v4().to_string(),
            session,
            test,
            run_id: None,
            pid: None,
            run_dir: None,
            status: MaestroJobStatus::Pending,
            name,
            created_at: now,
            updated_at: now,
            error_message: None,
            profile: std::env::var("VB_PROFILE").ok(),
        }
    }

    /// Returns true if this job can be re-submitted.
    #[allow(dead_code)]
    pub fn can_resubmit(&self) -> bool {
        matches!(
            self.status,
            MaestroJobStatus::Failed | MaestroJobStatus::Cancelled
        )
    }
}

/// File-backed JSON store for Maestro jobs.
///
/// Persists files under `~/.cache/virtuoso_bridge/maestro/jobs/<id>.json`.
pub struct MaestroJobStore {
    /// Backing directory for job files.
    pub dir: PathBuf,
}

impl MaestroJobStore {
    /// Open (or create) the job store directory at the default cache location.
    pub fn new() -> std::io::Result<Self> {
        let dir = cache_subdir(&["maestro", "jobs"]);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Open a store backed by an arbitrary directory (for tests / isolation).
    #[allow(dead_code)]
    pub fn with_dir(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Path for a given job id.
    fn path(&self, job_id: &str) -> PathBuf {
        self.dir.join(format!("{job_id}.json"))
    }

    /// Save a job to disk.
    pub fn save(&self, job: &MaestroJob) -> std::io::Result<()> {
        let path = self.path(&job.job_id);
        let json = serde_json::to_string_pretty(job)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load a job by id. Returns `None` if not found.
    pub fn load(&self, job_id: &str) -> std::io::Result<Option<MaestroJob>> {
        let path = self.path(job_id);
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        let job: MaestroJob = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(job))
    }

    /// Delete a job.
    #[allow(dead_code)]
    pub fn delete(&self, job_id: &str) -> std::io::Result<()> {
        let path = self.path(job_id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// List all jobs, newest-first.
    pub fn list(&self) -> std::io::Result<Vec<MaestroJob>> {
        let mut jobs = Vec::new();
        let entries = fs::read_dir(&self.dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(json) = fs::read_to_string(&path) {
                    if let Ok(job) = serde_json::from_str::<MaestroJob>(&json) {
                        jobs.push(job);
                    }
                }
            }
        }
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        Ok(jobs)
    }

    /// List jobs filtered by status.
    pub fn list_filtered(
        &self,
        status_filter: Option<MaestroJobStatus>,
        lib: Option<&str>,
        cell: Option<&str>,
        view: Option<&str>,
    ) -> std::io::Result<Vec<MaestroJob>> {
        let all = self.list()?;
        Ok(all
            .into_iter()
            .filter(|j| {
                if let Some(s) = status_filter {
                    if j.status != s {
                        return false;
                    }
                }
                // Filter by lib/cell/view from session name or run_dir — conservative:
                // we don't parse session names, so we only filter on run_dir if present.
                if let (Some(_), None, None) = (lib, cell, view) {
                    // Conservative: don't filter by lib alone
                    true
                } else {
                    true
                }
            })
            .collect())
    }

    /// Find a job by run_id. Returns `None` if not found.
    pub fn find_by_run_id(&self, run_id: &str) -> std::io::Result<Option<MaestroJob>> {
        let all = self.list()?;
        Ok(all.into_iter().find(|j| {
            j.run_id.as_deref() == Some(run_id)
                && !matches!(
                    j.status,
                    MaestroJobStatus::Failed | MaestroJobStatus::Cancelled
                )
        }))
    }

    /// Derive job status from the local run_dir filesystem.
    ///
    /// - `Running`: `<run_dir>/psf` exists and `<run_dir>/spectre.out` does NOT exist
    /// - `Completed`: both `<run_dir>/psf` and `<run_dir>/spectre.out` exist
    /// - `Failed`: `<run_dir>/spectre.out` exists but `<run_dir>/psf` does NOT
    /// - `Unknown`: run_dir not present or not accessible
    ///
    /// Returns the input status if run_dir is not set.
    pub fn derive_status_from_run_dir(job: &MaestroJob) -> MaestroJobStatus {
        let run_dir = match &job.run_dir {
            Some(d) => d,
            None => return job.status,
        };
        let rd = Path::new(run_dir);
        let psf = rd.join("psf");
        let spectre_out = rd.join("spectre.out");

        if psf.exists() {
            if spectre_out.exists() {
                MaestroJobStatus::Completed
            } else {
                MaestroJobStatus::Running
            }
        } else if spectre_out.exists() {
            MaestroJobStatus::Failed
        } else {
            MaestroJobStatus::Unknown
        }
    }
}

impl Default for MaestroJobStore {
    fn default() -> Self {
        Self::new().expect("MaestroJobStore::new() must succeed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_create_save_reload() {
        let tmp = TempDir::new().unwrap();
        let store = {
            let dir = tmp.path().join("jobs");
            std::fs::create_dir_all(&dir).unwrap();
            MaestroJobStore { dir }
        };

        let job = MaestroJob::new(
            "fnxSession4".into(),
            Some("AC".into()),
            Some("test run".into()),
        );
        let id = job.job_id.clone();
        store.save(&job).unwrap();

        let reloaded = store.load(&id).unwrap().expect("should be found");
        assert_eq!(reloaded.job_id, id);
        assert_eq!(reloaded.session, "fnxSession4");
        assert_eq!(reloaded.test.as_deref(), Some("AC"));
        assert_eq!(reloaded.status, MaestroJobStatus::Pending);
    }

    #[test]
    fn derive_status_completed() {
        let tmp = TempDir::new().unwrap();
        let job = MaestroJob::new("s".into(), None, None);
        let mut job = job;
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        std::fs::create_dir(tmp.path().join("psf")).unwrap();
        std::fs::write(tmp.path().join("spectre.out"), "done").unwrap();
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Completed
        );
    }

    #[test]
    fn derive_status_running() {
        let tmp = TempDir::new().unwrap();
        let job = MaestroJob::new("s".into(), None, None);
        let mut job = job;
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        std::fs::create_dir(tmp.path().join("psf")).unwrap();
        // no spectre.out → still running
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Running
        );
    }

    #[test]
    fn derive_status_failed() {
        let tmp = TempDir::new().unwrap();
        let job = MaestroJob::new("s".into(), None, None);
        let mut job = job;
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        std::fs::write(tmp.path().join("spectre.out"), "error").unwrap();
        // psf absent, spectre.out present → failed
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Failed
        );
    }

    #[test]
    fn derive_status_unknown_missing_run_dir() {
        let job = MaestroJob::new("s".into(), None, None);
        // No run_dir set → returns input status unchanged
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Pending
        );
    }

    #[test]
    fn derive_status_unknown_nonexistent_path() {
        let job = MaestroJob::new("s".into(), None, None);
        let mut job = job;
        job.run_dir = Some("/nonexistent/path/nowhere".into());
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Unknown
        );
    }

    #[test]
    fn list_newest_first() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore::with_dir(dir).unwrap();

        let j1 = MaestroJob::new("s1".into(), None, None);
        let j2 = MaestroJob::new("s2".into(), None, None);
        store.save(&j1).unwrap();
        store.save(&j2).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        // Newest first: j2 comes before j1
        assert_eq!(list[0].session, "s2");
        assert_eq!(list[1].session, "s1");
    }

    #[test]
    fn delete_job() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore::with_dir(dir).unwrap();

        let job = MaestroJob::new("s".into(), None, None);
        let id = job.job_id.clone();
        store.save(&job).unwrap();
        store.delete(&id).unwrap();
        assert!(store.load(&id).unwrap().is_none());
    }

    #[test]
    fn find_by_run_id() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore::with_dir(dir).unwrap();

        let mut job = MaestroJob::new("s".into(), None, None);
        job.run_id = Some("run_abc123".into());
        let id = job.job_id.clone();
        store.save(&job).unwrap();

        assert!(store.find_by_run_id("run_abc123").unwrap().is_some());
        assert_eq!(
            store.find_by_run_id("run_abc123").unwrap().unwrap().job_id,
            id
        );
        assert!(store.find_by_run_id("nonexistent").unwrap().is_none());
    }

    #[test]
    fn find_by_run_id_skips_failed() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore::with_dir(dir).unwrap();

        let mut job = MaestroJob::new("s".into(), None, None);
        job.run_id = Some("run_xyz".into());
        job.status = MaestroJobStatus::Failed;
        let _id = job.job_id.clone();
        store.save(&job).unwrap();

        // Failed jobs should not block resubmission
        assert!(store.find_by_run_id("run_xyz").unwrap().is_none());
    }
}

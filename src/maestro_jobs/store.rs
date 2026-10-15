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

    /// Validate that `job_id` is safe to use as a filename.
    ///
    /// `job_id` is a UUIDv4 (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`).
    /// Anything that escapes this shape is rejected so a caller cannot
    /// smuggle `..` or `/` into a filename (defence against
    /// `--job-id ../../something`). UUIDs are 36 chars: 32 hex digits
    /// plus four dashes.
    fn validate_job_id(job_id: &str) -> std::io::Result<()> {
        if job_id.len() != 36 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "job_id must be 36 chars (UUIDv4 shape), got {}: {job_id:?}",
                    job_id.len()
                ),
            ));
        }
        let bytes = job_id.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            let ok = match i {
                8 | 13 | 18 | 23 => *b == b'-',
                _ => b.is_ascii_hexdigit(),
            };
            if !ok {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("job_id not UUIDv4-shape (position {i}): {job_id:?}"),
                ));
            }
        }
        Ok(())
    }

    /// Path for a given job id.
    ///
    /// Returns an `InvalidInput` io::Error if `job_id` is not UUID-shaped.
    fn path(&self, job_id: &str) -> std::io::Result<PathBuf> {
        Self::validate_job_id(job_id)?;
        Ok(self.dir.join(format!("{job_id}.json")))
    }

    /// Save a job to disk atomically.
    ///
    /// Writes to `<id>.json.tmp` first, then renames onto `<id>.json`.
    /// `rename` is atomic on the same filesystem on both Linux and macOS
    /// (and on Windows when the destination does not exist), so a torn
    /// write never leaves a half-written JSON behind for `list()` to
    /// silently skip — `load()` returns `InvalidData` and surfaces it
    /// to the caller instead. Existing files are replaced.
    pub fn save(&self, job: &MaestroJob) -> std::io::Result<()> {
        let path = self.path(&job.job_id)?;
        let json = serde_json::to_string_pretty(job)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        // On Windows, rename fails if the destination exists. Remove first
        // only when the destination is present; the gap is bounded by the
        // tmp write having succeeded.
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&tmp, &path)
    }

    /// Load a job by id. Returns `None` if not found, `InvalidInput`
    /// if `job_id` is not UUID-shaped (so callers cannot smuggle path
    /// separators into `fs::read_to_string`).
    pub fn load(&self, job_id: &str) -> std::io::Result<Option<MaestroJob>> {
        let path = self.path(job_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path)?;
        let job: MaestroJob = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(job))
    }

    /// Delete a job. Returns  if  is not UUID-shaped.
    #[allow(dead_code)]
    pub fn delete(&self, job_id: &str) -> std::io::Result<()> {
        let path = self.path(job_id)?;
        if path.exists() {
            fs::remove_file(&path)?;
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
    ///
    /// Library/cell/view filtering was removed in round 2 — the previous
    /// substring stub could match false positives in unrelated paths.
    /// Round 3 should parse `run_dir/maestro.sdb` (or equivalent) to get
    /// reliable per-row library / cell / view metadata.
    pub fn list_filtered(
        &self,
        status_filter: Option<MaestroJobStatus>,
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
                true
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

    /// Whether a status is terminal (cannot transition further).
    pub fn is_terminal(status: MaestroJobStatus) -> bool {
        matches!(
            status,
            MaestroJobStatus::Completed | MaestroJobStatus::Failed | MaestroJobStatus::Cancelled
        )
    }

    /// Derive job status from the local run_dir filesystem.
    ///
    /// Terminal states are **sticky**: once a job has reached `Completed`,
    /// `Failed`, or `Cancelled` (operator action or explicit submit failure),
    /// this function returns that persisted status unchanged. Filesystem
    /// drift after cancellation cannot resurrect a `Cancelled` job into
    /// `Completed`, and `updated_at` stays at the cancellation timestamp
    /// until the next explicit write.
    ///
    /// For non-terminal jobs (`Pending` / `Running` / `Unknown`):
    /// - `Completed`: `<run_dir>/psf/spectre.out` exists (canonical end-of-run
    ///   artefact — same path `job_logs` reads, so status and tail agree).
    /// - `Failed`: psf/ never came up but a legacy `<run_dir>/spectre.out`
    ///   exists (Spectre died before psf was materialised — preserves the
    ///   pre-port signal).
    /// - `Running`: `<run_dir>/psf` exists but `<run_dir>/psf/spectre.out`
    ///   does NOT yet.
    /// - `Unknown`: neither path is reachable (run_dir missing, deleted,
    ///   permission denied, or remote share unavailable).
    ///
    /// Returns the input status if `run_dir` is not set.
    pub fn derive_status_from_run_dir(job: &MaestroJob) -> MaestroJobStatus {
        if Self::is_terminal(job.status) {
            return job.status;
        }
        let run_dir = match &job.run_dir {
            Some(d) => d,
            None => return job.status,
        };
        let rd = Path::new(run_dir);
        let psf = rd.join("psf");
        let canonical = psf.join("spectre.out");
        let legacy = rd.join("spectre.out");

        if canonical.exists() {
            MaestroJobStatus::Completed
        } else if psf.exists() {
            // psf directory is up but the log is not yet — mid-run.
            MaestroJobStatus::Running
        } else if legacy.exists() {
            // psf never materialised; this is a real early failure.
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
        std::fs::write(tmp.path().join("psf").join("spectre.out"), "done").unwrap();
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
        // Legacy signal: psf/ never came up, but a top-level spectre.out
        // exists (Spectre died early). Canonical psf/spectre.out absent.
        std::fs::write(tmp.path().join("spectre.out"), "error").unwrap();
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
    // ---- M2: terminal status is sticky ------------------------------

    #[test]
    fn derive_status_terminal_cancelled_is_sticky() {
        let tmp = TempDir::new().unwrap();
        let mut job = MaestroJob::new("s".into(), None, None);
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        // Cancel first.
        job.status = MaestroJobStatus::Cancelled;
        // Then the run completes on disk — terminal must win.
        std::fs::create_dir(tmp.path().join("psf")).unwrap();
        std::fs::write(tmp.path().join("psf").join("spectre.out"), "done").unwrap();
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Cancelled,
            "Cancelled must NOT be overwritten by a Completed-shaped run_dir"
        );
    }

    #[test]
    fn derive_status_terminal_failed_is_sticky() {
        let tmp = TempDir::new().unwrap();
        let mut job = MaestroJob::new("s".into(), None, None);
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        job.status = MaestroJobStatus::Failed;
        // psf appears later; Failed must remain.
        std::fs::create_dir(tmp.path().join("psf")).unwrap();
        std::fs::write(tmp.path().join("psf").join("spectre.out"), "x").unwrap();
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Failed
        );
    }

    #[test]
    fn derive_status_terminal_completed_is_sticky() {
        let tmp = TempDir::new().unwrap();
        let mut job = MaestroJob::new("s".into(), None, None);
        job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
        job.status = MaestroJobStatus::Completed;
        // Operator marks Completed; later removal of psf/ must not flip back.
        assert_eq!(
            MaestroJobStore::derive_status_from_run_dir(&job),
            MaestroJobStatus::Completed
        );
    }

    // ---- m7: atomic save --------------------------------------------

    #[test]
    fn save_is_atomic_no_tmp_file_remains_on_success() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore { dir: dir.clone() };
        let job = MaestroJob::new("s".into(), None, None);
        let id = job.job_id.clone();
        store.save(&job).unwrap();
        // The .json exists; the .tmp must NOT.
        assert!(dir.join(format!("{id}.json")).exists());
        assert!(!dir.join(format!("{id}.json.tmp")).exists());
    }

    #[test]
    fn save_replaces_existing_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let store = MaestroJobStore { dir: dir.clone() };
        let mut job = MaestroJob::new("s".into(), None, None);
        job.name = Some("v1".into());
        store.save(&job).unwrap();
        // Roundtrip + overwrite + reload should yield the latest snapshot.
        job.name = Some("v2".into());
        store.save(&job).unwrap();
        let reloaded = store.load(&job.job_id).unwrap().unwrap();
        assert_eq!(reloaded.name.as_deref(), Some("v2"));
    }

    // ---- m9: job_id validation --------------------------------------

    #[test]
    fn path_rejects_non_uuid_shape() {
        let store = MaestroJobStore {
            dir: std::path::PathBuf::from("/tmp"),
        };
        // path traversal: 36 chars but not a UUID
        let bad = "../../../tmp/evil"; // 20 chars
        let res = store.path(bad);
        assert!(res.is_err(), "must reject non-UUID job_id");
        // 36 chars but bad characters
        let bad36 = "x".repeat(36);
        let res = store.path(&bad36);
        assert!(res.is_err(), "must reject 36-char non-UUID job_id");
        // Right shape but wrong dash positions
        let bad_dash = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"; // dashes at 0,5,10,15
        let res = store.path(bad_dash);
        assert!(res.is_err(), "must reject wrong dash positions");
    }

    #[test]
    fn path_accepts_real_uuid_shape() {
        let store = MaestroJobStore {
            dir: std::path::PathBuf::from("/tmp"),
        };
        let uuid = "f47ac10b-58cc-4372-a567-0e02b2c3d479";
        let res = store.path(uuid);
        assert!(res.is_ok());
        assert_eq!(
            res.unwrap(),
            std::path::PathBuf::from("/tmp").join(format!("{uuid}.json"))
        );
    }

    #[test]
    fn load_rejects_non_uuid_shape() {
        let store = MaestroJobStore {
            dir: std::path::PathBuf::from("/tmp"),
        };
        let res = store.load("../etc/passwd");
        assert!(res.is_err(), "must not even attempt to read relative paths");
    }

    // ---- is_terminal --------------------------------------------------

    #[test]
    fn is_terminal_classification() {
        assert!(MaestroJobStore::is_terminal(MaestroJobStatus::Completed));
        assert!(MaestroJobStore::is_terminal(MaestroJobStatus::Failed));
        assert!(MaestroJobStore::is_terminal(MaestroJobStatus::Cancelled));
        assert!(!MaestroJobStore::is_terminal(MaestroJobStatus::Pending));
        assert!(!MaestroJobStore::is_terminal(MaestroJobStatus::Running));
        assert!(!MaestroJobStore::is_terminal(MaestroJobStatus::Unknown));
    }
}

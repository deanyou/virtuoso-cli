//! Integration tests for Maestro persistent job store.
//!
//! Exercises the file-backed JSON store against tempfile directories.

use virtuoso_cli::maestro_jobs::{MaestroJob, MaestroJobStatus, MaestroJobStore};

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir must succeed")
}

fn temp_store() -> (tempfile::TempDir, MaestroJobStore) {
    let tmp = temp_dir();
    let dir = tmp.path().join("jobs");
    std::fs::create_dir_all(&dir).unwrap();
    let store = MaestroJobStore::with_dir(dir).expect("with_dir must succeed");
    (tmp, store)
}

#[test]
fn roundtrip_create_save_reload() {
    let (_tmp, store) = temp_store();
    let job = MaestroJob::new(
        "fnxSession4".into(),
        Some("AC".into()),
        Some("gain test".into()),
    );
    let id = job.job_id.clone();
    store.save(&job).unwrap();

    let reloaded = store.load(&id).unwrap().expect("should be found");
    assert_eq!(reloaded.job_id, id);
    assert_eq!(reloaded.session, "fnxSession4");
    assert_eq!(reloaded.test.as_deref(), Some("AC"));
    assert_eq!(reloaded.status, MaestroJobStatus::Pending);
    assert_eq!(reloaded.name.as_deref(), Some("gain test"));
}

#[test]
fn status_is_pending_by_default() {
    let job = MaestroJob::new("s".into(), None, None);
    assert_eq!(job.status, MaestroJobStatus::Pending);
}

#[test]
fn can_resubmit_only_when_failed_or_cancelled() {
    let mut job = MaestroJob::new("s".into(), None, None);
    assert!(!job.can_resubmit(), "pending should not be resubmittable");

    job.status = MaestroJobStatus::Running;
    assert!(!job.can_resubmit(), "running should not be resubmittable");

    job.status = MaestroJobStatus::Failed;
    assert!(job.can_resubmit(), "failed should be resubmittable");

    job.status = MaestroJobStatus::Cancelled;
    assert!(job.can_resubmit(), "cancelled should be resubmittable");

    job.status = MaestroJobStatus::Completed;
    assert!(!job.can_resubmit(), "completed should not be resubmittable");
}

#[test]
fn derive_status_completed() {
    let tmp = temp_dir();
    let mut job = MaestroJob::new("s".into(), None, None);
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
    let tmp = temp_dir();
    let mut job = MaestroJob::new("s".into(), None, None);
    job.run_dir = Some(tmp.path().to_string_lossy().into_owned());
    std::fs::create_dir(tmp.path().join("psf")).unwrap();
    // No spectre.out → still running
    assert_eq!(
        MaestroJobStore::derive_status_from_run_dir(&job),
        MaestroJobStatus::Running
    );
}

#[test]
fn derive_status_failed() {
    let tmp = temp_dir();
    let mut job = MaestroJob::new("s".into(), None, None);
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
    // No run_dir → returns input status unchanged
    assert_eq!(
        MaestroJobStore::derive_status_from_run_dir(&job),
        MaestroJobStatus::Pending
    );
}

#[test]
fn derive_status_unknown_nonexistent_path() {
    let mut job = MaestroJob::new("s".into(), None, None);
    job.run_dir = Some("/nonexistent/path/nowhere".into());
    assert_eq!(
        MaestroJobStore::derive_status_from_run_dir(&job),
        MaestroJobStatus::Unknown
    );
}

#[test]
fn list_newest_first() {
    let (_tmp, store) = temp_store();
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
fn delete_removes_job() {
    let (_tmp, store) = temp_store();
    let job = MaestroJob::new("s".into(), None, None);
    let id = job.job_id.clone();
    store.save(&job).unwrap();
    store.delete(&id).unwrap();
    assert!(store.load(&id).unwrap().is_none());
}

#[test]
fn find_by_run_id() {
    let (_tmp, store) = temp_store();
    let mut job = MaestroJob::new("s".into(), None, None);
    job.run_id = Some("run_abc123".into());
    let id = job.job_id.clone();
    store.save(&job).unwrap();

    let found = store.find_by_run_id("run_abc123").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().job_id, id);
}

#[test]
fn find_by_run_id_returns_none_for_unknown() {
    let (_tmp, store) = temp_store();
    assert!(store.find_by_run_id("nonexistent").unwrap().is_none());
}

#[test]
fn find_by_run_id_skips_failed_jobs() {
    let (_tmp, store) = temp_store();
    let mut job = MaestroJob::new("s".into(), None, None);
    job.run_id = Some("run_xyz".into());
    job.status = MaestroJobStatus::Failed;
    store.save(&job).unwrap();
    // Failed jobs should not block resubmission
    assert!(store.find_by_run_id("run_xyz").unwrap().is_none());
}

#[test]
fn list_filtered_by_status() {
    let (_tmp, store) = temp_store();

    let mut j1 = MaestroJob::new("s1".into(), None, None);
    j1.status = MaestroJobStatus::Running;
    store.save(&j1).unwrap();

    let mut j2 = MaestroJob::new("s2".into(), None, None);
    j2.status = MaestroJobStatus::Failed;
    store.save(&j2).unwrap();

    let running: Vec<_> = store
        .list_filtered(Some(MaestroJobStatus::Running), None, None, None)
        .unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].session, "s1");

    let failed: Vec<_> = store
        .list_filtered(Some(MaestroJobStatus::Failed), None, None, None)
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].session, "s2");
}

#[test]
fn job_id_is_unique() {
    let job1 = MaestroJob::new("s".into(), None, None);
    let job2 = MaestroJob::new("s".into(), None, None);
    assert_ne!(job1.job_id, job2.job_id);
}

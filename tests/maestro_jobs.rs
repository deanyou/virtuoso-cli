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
    // Canonical end-state: <run_dir>/psf/spectre.out exists.
    let tmp = temp_dir();
    let mut job = MaestroJob::new("s".into(), None, None);
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
fn filter_by_status_matches_the_stored_status_when_nothing_was_derived() {
    let (_tmp, store) = temp_store();

    let mut j1 = MaestroJob::new("s1".into(), None, None);
    j1.status = MaestroJobStatus::Running;
    store.save(&j1).unwrap();

    let mut j2 = MaestroJob::new("s2".into(), None, None);
    j2.status = MaestroJobStatus::Failed;
    store.save(&j2).unwrap();

    let jobs = store.list().unwrap();
    let no_derivations = std::collections::HashMap::new();

    let running =
        MaestroJobStore::filter_by_status(&jobs, &no_derivations, Some(MaestroJobStatus::Running));
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].session, "s1");

    let failed =
        MaestroJobStore::filter_by_status(&jobs, &no_derivations, Some(MaestroJobStatus::Failed));
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].session, "s2");
}

#[test]
fn filter_by_status_uses_the_derived_status() {
    // The whole point: a job whose run has finished is `completed` even
    // though its record still says `running`. Filtering on the stored status
    // made `--status completed` return nothing.
    let (_tmp, store) = temp_store();

    let mut job = MaestroJob::new("s1".into(), None, None);
    job.status = MaestroJobStatus::Running;
    store.save(&job).unwrap();

    let jobs = store.list().unwrap();
    let mut derived = std::collections::HashMap::new();
    derived.insert(job.job_id.clone(), MaestroJobStatus::Completed);

    let completed =
        MaestroJobStore::filter_by_status(&jobs, &derived, Some(MaestroJobStatus::Completed));
    assert_eq!(
        completed.len(),
        1,
        "the derived status must drive the filter"
    );
    assert_eq!(completed[0].job_id, job.job_id);

    let running =
        MaestroJobStore::filter_by_status(&jobs, &derived, Some(MaestroJobStatus::Running));
    assert!(
        running.is_empty(),
        "the stored status must not drive the filter"
    );
}

#[test]
fn find_by_run_id_returns_the_oldest_owner() {
    // Two submits can be handed the same run id. The first submitter owns
    // it; the later record is the duplicate. The store must name the owner
    // deterministically, not "whichever the newest-first list happens to
    // produce".
    let (_tmp, store) = temp_store();

    let mut owner = MaestroJob::new("s".into(), None, None);
    owner.run_id = Some("run_dup".into());
    owner.status = MaestroJobStatus::Running;
    owner.created_at = chrono::Utc::now() - chrono::Duration::seconds(60);
    store.save(&owner).unwrap();

    let mut duplicate = MaestroJob::new("s".into(), None, None);
    duplicate.run_id = Some("run_dup".into());
    duplicate.status = MaestroJobStatus::Unknown;
    store.save(&duplicate).unwrap();

    assert_eq!(
        store.find_by_run_id("run_dup").unwrap().unwrap().job_id,
        owner.job_id
    );
}

#[test]
fn a_new_job_records_the_resolved_client_id() {
    let job = MaestroJob::new("s".into(), None, None);
    assert_eq!(
        job.client_id.as_deref(),
        Some(virtuoso_cli::client::bridge::resolve_client_id().as_str())
    );
}

#[test]
fn job_id_is_unique() {
    let job1 = MaestroJob::new("s".into(), None, None);
    let job2 = MaestroJob::new("s".into(), None, None);
    assert_ne!(job1.job_id, job2.job_id);
}

// ---- m1: the run id comes from the submit itself ---------------------

#[test]
fn submit_skill_reads_the_run_id_from_the_submit() {
    use virtuoso_cli::client::maestro_ops::MaestroOps;

    let submit = MaestroOps.submit_skill("fnxSession4");
    assert!(submit.contains("maeRunSimulation"), "{submit}");
    // The run id has to come from the submit itself: `maeGetSetup` has no
    // `?runId` keyword, so there is no pre-submit probe for it.
    assert!(submit.contains("?returnRunId"), "{submit}");
    assert!(!submit.contains("maeGetSetup"), "{submit}");
}

#[test]
fn job_helpers_use_only_functions_that_exist() {
    use virtuoso_cli::client::maestro_ops::MaestroOps;

    let submit = MaestroOps.submit_skill("sess");
    assert!(submit.contains("ipcGetPid"), "{submit}");
    assert!(!submit.contains("ipcGetProcessID"), "{submit}");
    assert!(!submit.contains("ipcGetStdin"), "{submit}");

    let pid = MaestroOps.pid_skill();
    assert!(pid.contains("ipcGetPid"), "{pid}");
    assert!(!pid.contains("ipcGetProcessID"), "{pid}");
    assert!(!pid.contains("ipcGetStdin"), "{pid}");
}

// ---- m7: the record is never absent while it is being written ---------

#[test]
fn save_never_leaves_the_record_missing() {
    // The job record is the only proof that the job was submitted, so a
    // reader must never see it disappear while it is being replaced. The
    // old save removed the destination and renamed the temporary file in
    // its place, which opens exactly that window.
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    let (_tmp, store) = temp_store();
    let mut job = MaestroJob::new("s".into(), None, None);
    store.save(&job).unwrap();

    let path = store.dir.join(format!("{}.json", job.job_id));
    let stop = Arc::new(AtomicBool::new(false));
    let missing = Arc::new(AtomicUsize::new(0));

    let reader = {
        let stop = Arc::clone(&stop);
        let missing = Arc::clone(&missing);
        let path = path.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if !path.exists() {
                    missing.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
    };

    for i in 0..500 {
        job.status = if i % 2 == 0 {
            MaestroJobStatus::Running
        } else {
            MaestroJobStatus::Pending
        };
        store.save(&job).unwrap();
    }

    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();

    assert_eq!(
        missing.load(Ordering::Relaxed),
        0,
        "the job record must never be absent between saves"
    );
}

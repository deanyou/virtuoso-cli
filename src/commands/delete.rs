//! Two-phase confirmation for the five destructive methods.
//!
//! Phase 1 answers `confirm_required` with a manifest read out of the design
//! database — not out of what the caller claimed — and a token. Phase 2 replays
//! the probe, checks the manifest still hashes to what the token was minted
//! against, and only then deletes. A design that changed in between invalidates
//! the approval, because the thing approved is no longer the thing in front of
//! us.
//!
//! **The tokens are on disk, not in memory.** The approved plan put them in the
//! daemon; `vcli rpc call` is a one-shot process (the only resident mode in
//! `main.rs` is stdio MCP), so an in-memory token would not survive the gap
//! between the two phases by construction. They live under
//! `state_root()/virtuoso_bridge/delete_tokens/`, mode 0600, TTL
//! [`TOKEN_TTL_S`], one use each — and the file is removed *before* the
//! destructive call, so a crash mid-delete cannot leave a token that still
//! works.
//!
//! The token is a random 128-bit value, not a hash of the manifest. A hash
//! would let a caller that knows the manifest mint its own approval, which is
//! the one thing this whole file exists to prevent.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::client::bridge::{VirtuosoClient, OPEN_CELLVIEWS};
use crate::client::delete_ops::{DeleteOps, DeleteTarget, FigureTarget};
use crate::command_log;
use crate::error::{Result, VirtuosoError};
use crate::rpc::dispatcher::parse_skill_json;
use crate::runtime_paths;

/// How long an approval stays good for. Long enough to read a manifest and
/// answer, short enough that an abandoned token is not a loaded gun.
pub const TOKEN_TTL_S: u64 = 300;

const TOKEN_PREFIX: &str = "del-";

// ── The five public entry points ─────────────────────────────────────

/// Delete every view of a cell. Irreversible.
pub fn cell(lib: &str, cell: &str, confirm: Option<&str>) -> Result<Value> {
    let target = DeleteTarget::cell(lib, cell)?;
    let client = VirtuosoClient::from_env()?;
    run(&client, &Op::on_disk(&target), confirm)
}

/// Delete one view of a cell. Irreversible.
pub fn cell_view(lib: &str, cell: &str, view: &str, confirm: Option<&str>) -> Result<Value> {
    let target = DeleteTarget::view(lib, cell, view)?;
    let client = VirtuosoClient::from_env()?;
    run(&client, &Op::on_disk(&target), confirm)
}

/// Remove an instance from the open schematic. In memory until `cell.save`.
pub fn instance(inst: &str, confirm: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let ops = DeleteOps::new();
    run(
        &client,
        &Op {
            action: "schematic.delete_instance",
            probe: ops.probe_instance(inst),
            execute: ops.delete_instance(inst),
            occupancy: None,
        },
        confirm,
    )
}

/// Remove a wire — and the label riding on it — from the open schematic. In
/// memory until `cell.save`.
pub fn figure(
    inst: Option<&str>,
    term: Option<&str>,
    x: Option<f64>,
    y: Option<f64>,
    confirm: Option<&str>,
) -> Result<Value> {
    let target = figure_target(inst, term, x, y)?;
    let client = VirtuosoClient::from_env()?;
    let ops = DeleteOps::new();
    run(
        &client,
        &Op {
            action: "schematic.delete_figure",
            probe: ops.probe_figure(&target),
            execute: ops.delete_figure(&target),
            occupancy: None,
        },
        confirm,
    )
}

/// Pick the addressing mode, and insist on exactly one complete pair.
///
/// A half-given pair (`inst` without `term`) is rejected rather than guessed
/// at, and so is giving both pairs: they are two ways of naming *one* figure,
/// so a caller that supplies both has not decided which figure it means, and
/// silently preferring one would delete something nobody asked for.
fn figure_target(
    inst: Option<&str>,
    term: Option<&str>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<FigureTarget> {
    match (inst, term, x, y) {
        (Some(i), Some(t), None, None) => FigureTarget::term(i, t),
        (None, None, Some(x), Some(y)) => FigureTarget::point(x, y),
        _ => Err(VirtuosoError::Config(
            "schematic.delete_figure: address the figure either by terminal \
             ('inst' + 'term') or by point ('x' + 'y') — exactly one of the two \
             pairs, and both halves of it"
                .into(),
        )),
    }
}

/// Remove a property from an instance in the open schematic. In memory until
/// `cell.save`.
pub fn prop(inst: &str, property: &str, confirm: Option<&str>) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let ops = DeleteOps::new();
    run(
        &client,
        &Op {
            action: "schematic.delete_prop",
            probe: ops.probe_prop(inst, property),
            execute: ops.delete_prop(inst, property),
            occupancy: None,
        },
        confirm,
    )
}

// ── The shared two-phase core ────────────────────────────────────────

/// One destructive operation in the two forms it needs: the probe that builds
/// the manifest, and the call that carries it out.
struct Op {
    action: &'static str,
    probe: String,
    execute: String,
    /// `Some` for the on-disk deletes: the cell whose open cellviews would
    /// block the delete. The schematic edits act *inside* an open cellview by
    /// design, so an occupancy check there would refuse every legitimate call.
    occupancy: Option<DeleteTarget>,
}

impl Op {
    fn on_disk(target: &DeleteTarget) -> Self {
        let ops = DeleteOps::new();
        Self {
            action: target.action(),
            probe: ops.probe(target),
            execute: ops.delete(target),
            occupancy: Some(target.clone()),
        }
    }
}

fn run(client: &VirtuosoClient, op: &Op, confirm: Option<&str>) -> Result<Value> {
    match confirm {
        None => offer(client, op),
        Some(token) => execute(client, op, token),
    }
}

/// Phase 1: read the database, show the damage, mint a token.
fn offer(client: &VirtuosoClient, op: &Op) -> Result<Value> {
    let manifest = probe(client, op)?;
    let open = open_cellviews(client, op)?;
    if !open.is_empty() {
        // Refused here rather than at phase 2 so the nod is not spent on a
        // delete that cannot run: `ddDeleteObj` takes an exclusive lock over
        // the whole subtree and would fail anyway, with a less useful message.
        return Err(VirtuosoError::Conflict(format!(
            "{}: {} is open in Virtuoso ({}) — close it first ('cell.close', or the window in the GUI). \
             ddDeleteObj cannot lock a subtree that is in use.",
            op.action,
            describe(&manifest),
            open.iter()
                .map(|c| format!(
                    "{}/{}/{} mode={}{}",
                    c["lib"].as_str().unwrap_or("?"),
                    c["cell"].as_str().unwrap_or("?"),
                    c["view"].as_str().unwrap_or("?"),
                    c["mode"].as_str().unwrap_or("?"),
                    if c["window"] == json!(true) { " (windowed)" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let token = mint(op.action, &manifest)?;
    Ok(json!({
        "status": "confirm_required",
        "action": op.action,
        "manifest": manifest,
        "warning": "不可撤销 / irreversible",
        "confirm_token": token,
        "expires_in_s": TOKEN_TTL_S,
        "next": format!(
            "re-send the same call with \"confirm\": \"{token}\" — only after a human has read the manifest above"
        ),
    }))
}

/// Phase 2: check the approval still describes reality, then delete.
fn execute(client: &VirtuosoClient, op: &Op, token: &str) -> Result<Value> {
    let record = load_token(token)?;

    if record["action"] != json!(op.action) {
        remove_token(token);
        return Err(VirtuosoError::Conflict(format!(
            "confirm token was issued for '{}', not '{}' — a token is not transferable between operations",
            record["action"].as_str().unwrap_or("?"),
            op.action
        )));
    }

    let manifest = probe(client, op)?;
    let fresh = manifest_hash(&manifest);
    if record["manifest_hash"] != json!(fresh) {
        // Whatever the human approved is not what is in front of us now.
        remove_token(token);
        return Err(VirtuosoError::Conflict(format!(
            "the design changed since this token was issued — approval withdrawn.\n\
             approved: {}\n\
             now:      {}\n\
             Re-run without 'confirm' to get a fresh manifest.",
            serde_json::to_string(&record["manifest"]).unwrap_or_default(),
            serde_json::to_string(&manifest).unwrap_or_default()
        )));
    }

    let open = open_cellviews(client, op)?;
    if !open.is_empty() {
        // Not a token problem — the token stays valid, so closing the cellview
        // and retrying works without asking the human to approve twice.
        return Err(VirtuosoError::Conflict(format!(
            "{}: {} was opened in Virtuoso after the manifest was approved — close it and retry with the same token",
            op.action,
            describe(&manifest)
        )));
    }

    // One-shot, and spent *before* the irreversible part. A token that survived
    // a crash mid-delete would be an approval for a state nobody has seen.
    remove_token(token);

    command_log::log_command(
        "DELETE",
        &format!(
            "{} {} token={} manifest={}",
            op.action,
            describe(&manifest),
            token,
            serde_json::to_string(&manifest).unwrap_or_default()
        ),
        None,
    );

    let r = client
        .execute_skill_unchecked(&op.execute, Some(client.read_timeout()))?
        .ok_or_exec(op.action)?;

    let mut out = parse_skill_json(&r.output).unwrap_or_else(|_| json!({"output": r.output}));
    if let Value::Object(map) = &mut out {
        map.insert("status".into(), json!("ok"));
        map.insert("confirmed".into(), json!(true));
        map.insert("manifest".into(), Value::Array(manifest));
    }
    Ok(out)
}

/// Run the read-only probe and wrap its single object as a one-entry manifest.
///
/// A list, even of one, because the shape is what a human reads before nodding —
/// and it is the same shape whether this grows to take several targets or not.
fn probe(client: &VirtuosoClient, op: &Op) -> Result<Vec<Value>> {
    let r = client
        .execute_skill_unchecked(&op.probe, Some(client.read_timeout()))?
        .ok_or_exec(&format!("{} (probe)", op.action))?;
    Ok(vec![parse_skill_json(&r.output)?])
}

/// Cellviews of the delete target that Virtuoso currently holds open.
fn open_cellviews(client: &VirtuosoClient, op: &Op) -> Result<Vec<Value>> {
    let Some(target) = &op.occupancy else {
        return Ok(Vec::new());
    };
    let r = client
        .execute_skill_unchecked(OPEN_CELLVIEWS, Some(client.read_timeout()))?
        .ok_or_exec("list open cellviews")?;
    let all = parse_skill_json(&r.output)?;
    let Some(list) = all.as_array() else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter(|c| {
            c["lib"] == json!(target.lib_name())
                && c["cell"] == json!(target.cell_name())
                // A whole-cell delete is blocked by any view of it; a
                // single-view delete only by that view.
                && target
                    .view_name()
                    .is_none_or(|v| c["view"] == json!(v))
        })
        .cloned()
        .collect())
}

// ── Token store ──────────────────────────────────────────────────────

fn token_dir() -> PathBuf {
    runtime_paths::state_root()
        .join("virtuoso_bridge")
        .join("delete_tokens")
}

/// `del-` + 32 hex characters.
///
/// Two v4 UUIDs through SHA-256: `Uuid::new_v4` alone is 122 random bits, and
/// the plan called for 128. The hash is a width adapter, not the secret — the
/// entropy is in the UUIDs.
fn mint(action: &str, manifest: &[Value]) -> Result<String> {
    let mut h = Sha256::new();
    h.update(uuid::Uuid::new_v4().as_bytes());
    h.update(uuid::Uuid::new_v4().as_bytes());
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(&h.finalize()[..16]));

    let now = unix_now();
    let record = json!({
        "token": token,
        "action": action,
        "manifest": manifest,
        "manifest_hash": manifest_hash(manifest),
        "created_at": now,
        "expires_at": now + TOKEN_TTL_S,
    });

    let dir = token_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{token}.json"));
    write_private(&path, &serde_json::to_vec_pretty(&record)?)?;
    sweep_expired(&dir);
    Ok(token)
}

/// 0600 from the moment the file exists — not created-then-chmodded, which
/// leaves a window where another user on this shared box can read a valid
/// approval.
fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)?;
    Ok(())
}

fn load_token(token: &str) -> Result<Value> {
    let path = token_dir().join(format!("{}.json", validate_token(token)?));
    let raw = fs::read_to_string(&path).map_err(|_| {
        VirtuosoError::NotFound(format!(
            "unknown confirm token '{token}' — it was already used, it expired (TTL {TOKEN_TTL_S}s), \
             or it was never issued. Re-run without 'confirm' to get a fresh manifest."
        ))
    })?;
    let record: Value = serde_json::from_str(&raw)?;

    let expires = record["expires_at"].as_u64().unwrap_or(0);
    if unix_now() >= expires {
        remove_token(token);
        return Err(VirtuosoError::Conflict(format!(
            "confirm token '{token}' expired ({TOKEN_TTL_S}s TTL) — re-run without 'confirm' to get a fresh manifest"
        )));
    }
    Ok(record)
}

/// The token arrives from the caller and becomes part of a path, so it is
/// checked against a strict charset before it is ever joined onto a directory.
/// `del-../../etc/shadow` must not even be looked up.
fn validate_token(token: &str) -> Result<&str> {
    let bad = || {
        VirtuosoError::Config(format!(
            "malformed confirm token '{token}' — expected '{TOKEN_PREFIX}' followed by 32 hex characters"
        ))
    };
    let hex_part = token.strip_prefix(TOKEN_PREFIX).ok_or_else(bad)?;
    if hex_part.len() != 32 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    Ok(token)
}

fn remove_token(token: &str) {
    if let Ok(t) = validate_token(token) {
        let _ = fs::remove_file(token_dir().join(format!("{t}.json")));
    }
}

/// Drop tokens whose TTL has passed. Housekeeping, not security — an expired
/// token is already refused by [`load_token`]; this just keeps the directory
/// from growing without bound on a long-lived box.
fn sweep_expired(dir: &PathBuf) {
    let now = unix_now();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let expired = fs::read_to_string(e.path())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["expires_at"].as_u64())
            .is_none_or(|exp| now >= exp);
        if expired {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// What the token is bound to. Canonical because `serde_json::Map` is a
/// `BTreeMap` in this build (no `preserve_order` feature), so key order is
/// fixed and two equal manifests always hash the same.
fn manifest_hash(manifest: &[Value]) -> String {
    let canonical = Value::Array(manifest.to_vec()).to_string();
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// One line naming the target, for error messages.
fn describe(manifest: &[Value]) -> String {
    manifest
        .iter()
        .map(|m| {
            let mut s = format!(
                "{}/{}",
                m["lib"].as_str().unwrap_or("?"),
                m["cell"].as_str().unwrap_or("?")
            );
            if let Some(v) = m["view"].as_str() {
                s.push('/');
                s.push_str(v);
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Points the token store at a scratch directory and restores the env on
    /// drop. `VB_STATE_DIR` is the first thing `state_root()` looks at.
    struct Store {
        _dir: tempfile::TempDir,
        saved: Option<std::ffi::OsString>,
    }

    impl Drop for Store {
        fn drop(&mut self) {
            match self.saved.take() {
                Some(o) => std::env::set_var("VB_STATE_DIR", o),
                None => std::env::remove_var("VB_STATE_DIR"),
            }
        }
    }

    fn store() -> Store {
        let dir = tempfile::tempdir().expect("tempdir");
        let saved = std::env::var_os("VB_STATE_DIR");
        std::env::set_var("VB_STATE_DIR", dir.path());
        Store { _dir: dir, saved }
    }

    fn manifest() -> Vec<Value> {
        vec![json!({
            "action": "cell.delete",
            "lib": "DESIGN_LIB",
            "cell": "_rbscratch",
            "views": ["schematic"],
            "path": "/designs/DESIGN_LIB/_rbscratch",
            "instances": 3,
            "writable": true,
        })]
    }

    #[test]
    #[serial]
    fn a_minted_token_loads_once_and_then_is_gone() {
        let _s = store();
        let t = mint("cell.delete", &manifest()).expect("mints");
        assert!(load_token(&t).is_ok());
        remove_token(&t);
        assert!(load_token(&t).is_err(), "a spent token must not load again");
    }

    #[test]
    #[serial]
    fn the_token_is_random_not_derived_from_the_manifest() {
        let _s = store();
        let a = mint("cell.delete", &manifest()).expect("mints");
        let b = mint("cell.delete", &manifest()).expect("mints");
        // Same action, same manifest. If the token were a hash of them, a
        // caller who knows the manifest could mint its own approval.
        assert_ne!(a, b);
    }

    #[test]
    #[serial]
    fn a_token_is_128_bits_of_hex_behind_a_recognisable_prefix() {
        let _s = store();
        let t = mint("cell.delete", &manifest()).expect("mints");
        assert!(t.starts_with(TOKEN_PREFIX));
        assert_eq!(t.len(), TOKEN_PREFIX.len() + 32);
        assert!(validate_token(&t).is_ok());
    }

    /// The token becomes a filename. Anything that is not 32 hex characters is
    /// refused before it touches the filesystem.
    #[test]
    #[serial]
    fn a_token_cannot_be_a_path() {
        for bad in [
            "del-../../../etc/shadow",
            "del-/absolute",
            "../escape",
            "del-",
            "del-deadbeef",
            "del-DEADBEEFdeadbeefDEADBEEFdeadbeefZZ",
            "del-0123456789abcdef0123456789abcdef0",
        ] {
            assert!(validate_token(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    #[serial]
    fn an_expired_token_is_refused_and_cleaned_up() {
        let _s = store();
        let t = mint("cell.delete", &manifest()).expect("mints");
        let path = token_dir().join(format!("{t}.json"));

        // Backdate it past its TTL rather than sleeping for five minutes.
        let mut record: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("reads")).expect("parses");
        record["expires_at"] = json!(unix_now() - 1);
        fs::write(&path, serde_json::to_vec(&record).expect("serialises")).expect("writes");

        assert!(load_token(&t).is_err());
        assert!(!path.exists(), "an expired token should not be left lying around");
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn a_token_file_is_not_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt;
        let _s = store();
        let t = mint("cell.delete", &manifest()).expect("mints");
        let mode = fs::metadata(token_dir().join(format!("{t}.json")))
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        // Shared box: another user reading a valid approval is the whole point
        // of 0600.
        assert_eq!(mode, 0o600, "token file mode {mode:o}");
    }

    /// The hash is what makes the approval content-bound: a design that moved
    /// on invalidates it.
    #[test]
    #[serial]
    fn the_hash_changes_when_the_manifest_does() {
        let before = manifest_hash(&manifest());
        let mut changed = manifest();
        changed[0]["instances"] = json!(4);
        assert_ne!(before, manifest_hash(&changed));

        let mut more_views = manifest();
        more_views[0]["views"] = json!(["schematic", "symbol"]);
        assert_ne!(before, manifest_hash(&more_views));
    }

    #[test]
    #[serial]
    fn the_hash_is_stable_across_equal_manifests() {
        assert_eq!(manifest_hash(&manifest()), manifest_hash(&manifest()));
    }

    #[test]
    #[serial]
    fn the_record_remembers_which_operation_was_approved() {
        let _s = store();
        let t = mint("cell.delete_view", &manifest()).expect("mints");
        let record = load_token(&t).expect("loads");
        assert_eq!(record["action"], "cell.delete_view");
        assert_eq!(record["manifest_hash"], manifest_hash(&manifest()));
    }

    #[test]
    #[serial]
    fn the_sweep_removes_expired_tokens_and_keeps_live_ones() {
        let _s = store();
        let live = mint("cell.delete", &manifest()).expect("mints");
        let dead = mint("cell.delete", &manifest()).expect("mints");
        let dead_path = token_dir().join(format!("{dead}.json"));
        let mut record: Value =
            serde_json::from_str(&fs::read_to_string(&dead_path).expect("reads")).expect("parses");
        record["expires_at"] = json!(unix_now() - 1);
        fs::write(&dead_path, serde_json::to_vec(&record).expect("ser")).expect("writes");

        sweep_expired(&token_dir());
        assert!(!dead_path.exists());
        assert!(token_dir().join(format!("{live}.json")).exists());
    }

    /// Junk in the token directory is not a reason to fail a delete, but it is
    /// also not something to keep.
    #[test]
    #[serial]
    fn the_sweep_discards_files_it_cannot_read_as_tokens() {
        let _s = store();
        let dir = token_dir();
        fs::create_dir_all(&dir).expect("mkdir");
        let junk = dir.join("not-a-token.json");
        fs::write(&junk, b"{ this is not json").expect("writes");
        sweep_expired(&dir);
        assert!(!junk.exists());
    }

    #[test]
    #[serial]
    fn the_allow_set_is_enforced_before_any_bridge_call() {
        // No client, no tunnel, no Virtuoso: `DeleteTarget` refuses first, so a
        // delete aimed at the PDK never reaches the network.
        for bad in ["pdkLib", "analogLib", "basic"] {
            let e = cell(bad, "nmos", None).expect_err("must refuse");
            assert!(
                e.to_string().contains("allow-set"),
                "wrong reason for {bad}: {e}"
            );
        }
    }

    #[test]
    #[serial]
    fn deleting_a_library_is_not_expressible() {
        let e = cell("DESIGN_LIB", "", None).expect_err("must refuse");
        assert!(e.to_string().contains("cell name is required"), "{e}");
    }

    #[test]
    #[serial]
    fn the_manifest_description_names_cells_and_views() {
        assert_eq!(describe(&manifest()), "DESIGN_LIB/_rbscratch");
        let mut with_view = manifest();
        with_view[0]["view"] = json!("schematic");
        assert_eq!(describe(&with_view), "DESIGN_LIB/_rbscratch/schematic");
    }

    #[test]
    #[serial]
    fn a_figure_is_addressed_by_terminal_or_by_point() {
        assert_eq!(
            figure_target(Some("M1"), Some("G"), None, None).expect("terminal pair"),
            FigureTarget::term("M1", "G").unwrap()
        );
        assert_eq!(
            figure_target(None, None, Some(2.6), Some(3.0)).expect("point pair"),
            FigureTarget::point(2.6, 3.0).unwrap()
        );
    }

    /// Half a pair is a typo, and both pairs means the caller has not decided
    /// which figure it meant. Either way, guessing would delete something
    /// nobody asked for.
    #[test]
    #[serial]
    fn an_incomplete_or_doubled_address_is_refused() {
        let refused = |inst, term, x, y| {
            let e = figure_target(inst, term, x, y)
                .expect_err(&format!("{inst:?} {term:?} {x:?} {y:?} must be refused"));
            assert!(e.to_string().contains("exactly one of the two"), "{e}");
        };
        refused(None, None, None, None);
        refused(Some("M1"), None, None, None);
        refused(None, Some("G"), None, None);
        refused(None, None, Some(2.6), None);
        refused(None, None, None, Some(3.0));
        refused(Some("M1"), Some("G"), Some(2.6), Some(3.0));
    }
}

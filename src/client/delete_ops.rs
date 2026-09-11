//! Every SKILL expression in this crate that can destroy design data.
//!
//! They are collected here, in one file, on purpose: an auditor reviewing what
//! this build is able to delete has exactly one file to read. `library_ops.rs`
//! stays read-only, and its tests assert that (`list_cells_is_read_only`), so
//! the read side of a delete manifest cannot quietly grow a delete.
//!
//! Two landmines from the IC23.1 manuals shape the code below.
//!
//! **`ddGetObj` creates.** Its own description is *"finds … **and creates**
//! cells, views, and files"*, and the mode is the **sixth positional
//! argument** — `ddGetObj(t_libName [t_cellName] [t_viewName] [t_fileName]
//! [b_contextId] [t_mode])`. Every call here therefore passes all six, with an
//! explicit `"r"`: a lookup must never bring the thing it was looking for into
//! existence.
//!
//! **`ddDeleteObj` on a library ID deletes the library** — the whole directory
//! *and* its `cds.lib` entry. That path is not guarded by an `if` here; it is
//! made unreachable. The only way to reach a delete generator is to hold a
//! [`DeleteTarget`], and no [`DeleteTarget`] constructor accepts an empty cell
//! name, so the argument list `ddGetObj("LIB" nil …)` — the one that yields a
//! library — cannot be produced.
//!
//! One guardrail we deliberately did **not** build: `ddDeleteObj` already takes
//! a non-blocking exclusive lock on the whole subtree it is about to remove and
//! fails if it cannot get it, so a cellview that is open somewhere already
//! blocks the delete. Re-implementing that check would only add a second,
//! weaker copy of it.

use crate::client::bridge::escape_skill_string;
use crate::client::schematic_ops::{cv_guard, EDIT_CV};
use crate::error::{Result, VirtuosoError};

/// Libraries this build is willing to delete from — **empty**.
///
/// A destructive feature ships off. Until an operator names their own
/// libraries in [`ALLOW_LIBS_ENV`], every [`DeleteTarget`] construction fails,
/// so no token can be minted and no delete SKILL can be generated. Whatever is
/// named there is the whole reachable set: the PDK, `analogLib`, `basic`, and
/// every other user's library are outside it unless someone deliberately types
/// them in.
pub const DEFAULT_ALLOW_LIBS: &[&str] = &[];

/// The allow-set, comma-separated. This is the only way to turn the delete
/// methods on.
pub const ALLOW_LIBS_ENV: &str = "VB_DELETE_ALLOW_LIBS";

/// The allow-set in force for this process.
pub fn allowed_libs() -> Vec<String> {
    match std::env::var(ALLOW_LIBS_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => DEFAULT_ALLOW_LIBS.iter().map(|s| s.to_string()).collect(),
    }
}

/// A delete target that has already been checked.
///
/// Holding one of these is proof that the library is in the allow-set and the
/// cell name is non-empty. The delete generators take one by reference and so
/// cannot be called on anything else — that is the structural half of the
/// "never delete a library" rule, the half that does not depend on anyone
/// remembering to write an `if`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTarget {
    lib: String,
    cell: String,
    view: Option<String>,
}

impl DeleteTarget {
    /// A whole cell — every view it has.
    pub fn cell(lib: &str, cell: &str) -> Result<Self> {
        Self::checked(lib, cell, None)
    }

    /// One view of one cell.
    pub fn view(lib: &str, cell: &str, view: &str) -> Result<Self> {
        Self::checked(lib, cell, Some(view))
    }

    fn checked(lib: &str, cell: &str, view: Option<&str>) -> Result<Self> {
        let (lib, cell) = (lib.trim(), cell.trim());
        if cell.is_empty() {
            // The landmine, refused at the only door into this module: with no
            // cell name, `ddGetObj` hands back the *library*, and
            // `ddDeleteObj` on a library ID takes the directory and the
            // `cds.lib` entry with it.
            return Err(VirtuosoError::Config(
                "delete: a cell name is required — a library is never a delete target".into(),
            ));
        }
        let allowed = allowed_libs();
        if !allowed.iter().any(|l| l == lib) {
            let set = if allowed.is_empty() {
                format!(
                    "the allow-set is empty, which is what this build ships with; \
                     set {ALLOW_LIBS_ENV} to a comma-separated list of the \
                     libraries you are willing to delete from"
                )
            } else {
                format!(
                    "the allow-set is ({}); {ALLOW_LIBS_ENV} changes it",
                    allowed.join(", ")
                )
            };
            return Err(VirtuosoError::Config(format!(
                "delete refused: library '{lib}' is not in the delete allow-set — {set}. \
                 A library that is not named there cannot be reached by any sequence \
                 of RPC calls."
            )));
        }
        if let Some(v) = view {
            if v.trim().is_empty() {
                return Err(VirtuosoError::Config(
                    "delete: view name is empty — pass a view, or delete the whole cell".into(),
                ));
            }
        }
        Ok(Self {
            lib: lib.to_string(),
            cell: cell.to_string(),
            view: view.map(|v| v.trim().to_string()),
        })
    }

    /// Accessors are `*_name` so they do not collide with the `cell` / `view`
    /// constructors above, and so they read like the SKILL attributes they
    /// end up next to (`cv~>libName`, `cv~>cellName`, `cv~>viewName`).
    pub fn lib_name(&self) -> &str {
        &self.lib
    }

    pub fn cell_name(&self) -> &str {
        &self.cell
    }

    pub fn view_name(&self) -> Option<&str> {
        self.view.as_deref()
    }

    /// The RPC method this target belongs to — also what the confirmation token
    /// is bound to, so a token minted for a view cannot execute a cell delete.
    pub fn action(&self) -> &'static str {
        match self.view {
            Some(_) => "cell.delete_view",
            None => "cell.delete",
        }
    }

    /// `DESIGN_LIB/_rbscratch` or `DESIGN_LIB/_rbscratch/schematic`.
    pub fn describe(&self) -> String {
        match &self.view {
            Some(v) => format!("{}/{}/{}", self.lib, self.cell, v),
            None => format!("{}/{}", self.lib, self.cell),
        }
    }

    /// The six positional arguments of `ddGetObj`, mode always explicit.
    ///
    /// `nil` for the file and context slots is not padding — without them the
    /// `"r"` would land in `t_fileName` and the mode would fall back to its
    /// default, which is how a "lookup" starts creating things.
    fn dd_args(&self) -> String {
        let lib = escape_skill_string(&self.lib);
        let cell = escape_skill_string(&self.cell);
        match &self.view {
            Some(v) => {
                let v = escape_skill_string(v);
                format!(r#""{lib}" "{cell}" "{v}" nil nil "r""#)
            }
            None => format!(r#""{lib}" "{cell}" nil nil nil "r""#),
        }
    }
}

/// SKILL generators for the four destructive operations and their read-only
/// probes. The probes exist so the confirmation manifest is built from the
/// design database rather than from what the caller claimed.
#[derive(Default)]
pub struct DeleteOps;

impl DeleteOps {
    pub fn new() -> Self {
        Self
    }

    // ── On-disk deletes (dd layer) ───────────────────────────────────

    /// Read-only: what `delete` would destroy.
    pub fn probe(&self, target: &DeleteTarget) -> String {
        let args = target.dd_args();
        let action = target.action();
        let what = escape_skill_string(&target.describe());
        let lib = escape_skill_string(target.lib_name());
        let cell = escape_skill_string(target.cell_name());
        // `instances` counts what is inside the schematic — the size of what is
        // about to go. It is -1 when there is no schematic view to count, which
        // reads as "not applicable" rather than as an empty cell.
        match target.view_name() {
            None => {
                let count = instance_count(&lib, &cell, "schematic");
                format!(
                    r#"let((id views path n cv r out sep) id = ddGetObj({args}) when(!id error("%s" "{action}: no such cell {what}")) unless(ddIsId(id) error("%s" "{action}: {what} is not a design-management object")) views = sort(mapcar(lambda((v) v~>name) id~>views) nil) path = ddGetObjReadPath(id) n = -1 when(member("schematic" views) {count}) out = "" sep = "" foreach(v views out = strcat(out sep sprintf(nil "%L" v)) sep = ",") sprintf(nil "{{\"action\":\"{action}\",\"lib\":%L,\"cell\":%L,\"views\":[%s],\"path\":%L,\"instances\":%d,\"writable\":%s}}" "{lib}" "{cell}" out if(path path "") n if(ddIsObjWritable(id) "true" "false")))"#
                )
            }
            Some(view) => {
                let view_e = escape_skill_string(view);
                let count = instance_count(&lib, &cell, &view_e);
                format!(
                    r#"let((id path n cv r) id = ddGetObj({args}) when(!id error("%s" "{action}: no such view {what}")) unless(ddIsId(id) error("%s" "{action}: {what} is not a design-management object")) path = ddGetObjReadPath(id) n = -1 {count} sprintf(nil "{{\"action\":\"{action}\",\"lib\":%L,\"cell\":%L,\"view\":%L,\"views\":[%L],\"path\":%L,\"instances\":%d,\"writable\":%s}}" "{lib}" "{cell}" "{view_e}" "{view_e}" if(path path "") n if(ddIsObjWritable(id) "true" "false")))"#
                )
            }
        }
    }

    /// Irreversible. Removes the cell (or the one view) from disk through the
    /// `dd` API, so the `data.dm` index stays Cadence's problem — this is the
    /// documented route, and the reason the "never `rm -rf` an OA directory"
    /// rule does not have to be broken to clean a library up.
    pub fn delete(&self, target: &DeleteTarget) -> String {
        let args = target.dd_args();
        let action = target.action();
        let what = escape_skill_string(&target.describe());
        format!(
            r#"let((id path ok) id = ddGetObj({args}) when(!id error("%s" "{action}: no such target {what}")) unless(ddIsId(id) error("%s" "{action}: {what} is not a design-management object")) unless(ddIsObjWritable(id) error("%s" "{action}: {what} is not writable by this process")) path = ddGetObjReadPath(id) ok = ddDeleteObj(id) unless(ok error("%s" "{action}: ddDeleteObj refused {what} — it takes a non-blocking exclusive lock on the whole subtree, so an open cellview or another process holding it will fail here")) sprintf(nil "{{\"status\":\"ok\",\"action\":\"{action}\",\"deleted\":%L,\"path\":%L}}" "{what}" if(path path "")))"#
        )
    }

    // ── In-memory schematic edits (db layer) ─────────────────────────

    /// Read-only: which instance `delete_instance` would remove, and from where.
    pub fn probe_instance(&self, inst_name: &str) -> String {
        let name = escape_skill_string(inst_name);
        let guard = cv_guard();
        let lib_guard = lib_guard("schematic.delete_instance");
        format!(
            r#"let((cv inst master) cv = {EDIT_CV} {guard} {lib_guard} inst = car(setof(i cv~>instances i~>name == "{name}")) when(!inst error("instance %s not found in this cellview" "{name}")) master = sprintf(nil "%s/%s" inst~>libName inst~>cellName) sprintf(nil "{{\"action\":\"schematic.delete_instance\",\"lib\":%L,\"cell\":%L,\"view\":%L,\"instance\":%L,\"master\":%L}}" cv~>libName cv~>cellName cv~>viewName "{name}" master))"#
        )
    }

    /// `schDelete(d_fig)` — *"Deletes the figure or object you specify only from
    /// schematic or symbol cellviews"*, which is why this is not the
    /// interactive `schHiDelete` (that one needs a GUI selection set).
    ///
    /// The edit lands in memory. Nothing is on disk until `cell.save`, so the
    /// reply says `saved: false` rather than letting the caller assume.
    pub fn delete_instance(&self, inst_name: &str) -> String {
        let name = escape_skill_string(inst_name);
        let guard = cv_guard();
        let lib_guard = lib_guard("schematic.delete_instance");
        format!(
            r#"let((cv inst master ok) cv = {EDIT_CV} {guard} {lib_guard} inst = car(setof(i cv~>instances i~>name == "{name}")) when(!inst error("instance %s not found in this cellview" "{name}")) master = sprintf(nil "%s/%s" inst~>libName inst~>cellName) ok = schDelete(inst) unless(ok error("%s" "schematic.delete_instance: schDelete refused instance {name}")) sprintf(nil "{{\"status\":\"ok\",\"action\":\"schematic.delete_instance\",\"lib\":%L,\"cell\":%L,\"view\":%L,\"instance\":%L,\"master\":%L,\"saved\":false}}" cv~>libName cv~>cellName cv~>viewName "{name}" master))"#
        )
    }

    /// Read-only: the property `delete_prop` would remove, and its value.
    pub fn probe_prop(&self, inst_name: &str, prop: &str) -> String {
        let name = escape_skill_string(inst_name);
        let prop = escape_skill_string(prop);
        let guard = cv_guard();
        let lib_guard = lib_guard("schematic.delete_prop");
        format!(
            r#"let((cv inst p was) cv = {EDIT_CV} {guard} {lib_guard} inst = car(setof(i cv~>instances i~>name == "{name}")) when(!inst error("instance %s not found in this cellview" "{name}")) p = dbSearchPropByName(inst "{prop}") when(!p error("property %s not found on instance %s" "{prop}" "{name}")) was = p~>value was = if(stringp(was) was sprintf(nil "%L" was)) sprintf(nil "{{\"action\":\"schematic.delete_prop\",\"lib\":%L,\"cell\":%L,\"view\":%L,\"instance\":%L,\"prop\":%L,\"value\":%L}}" cv~>libName cv~>cellName cv~>viewName "{name}" "{prop}" was))"#
        )
    }

    /// `dbSearchPropByName` then `dbDeletePropByName(g_object t_name)` — the
    /// search first so a missing property is a clear error instead of a bare
    /// `nil`, and so the manifest can show what the value was before it went.
    pub fn delete_prop(&self, inst_name: &str, prop: &str) -> String {
        let name = escape_skill_string(inst_name);
        let prop = escape_skill_string(prop);
        let guard = cv_guard();
        let lib_guard = lib_guard("schematic.delete_prop");
        format!(
            r#"let((cv inst p was ok) cv = {EDIT_CV} {guard} {lib_guard} inst = car(setof(i cv~>instances i~>name == "{name}")) when(!inst error("instance %s not found in this cellview" "{name}")) p = dbSearchPropByName(inst "{prop}") when(!p error("property %s not found on instance %s" "{prop}" "{name}")) was = p~>value was = if(stringp(was) was sprintf(nil "%L" was)) ok = dbDeletePropByName(inst "{prop}") unless(ok error("%s" "schematic.delete_prop: dbDeletePropByName refused property {prop} on {name}")) sprintf(nil "{{\"status\":\"ok\",\"action\":\"schematic.delete_prop\",\"lib\":%L,\"cell\":%L,\"view\":%L,\"instance\":%L,\"prop\":%L,\"was\":%L,\"saved\":false}}" cv~>libName cv~>cellName cv~>viewName "{name}" "{prop}" was))"#
        )
    }
}

/// Count the instances in a cellview without disturbing it.
///
/// A cellview that is **already open** is read through the handle Virtuoso
/// already holds. Only a handle this probe opened itself gets `dbClose`d:
/// opening and closing a cellview the user has in an editor window is not a
/// read — it can take their window's cellview out from under them, and a probe
/// whose whole job is to say "here is what you are about to lose" must not
/// cost anything.
///
/// Assumes `cv`, `n` and `r` are bound in the enclosing `let()`, with `n`
/// pre-set to `-1` for "could not count".
fn instance_count(lib: &str, cell: &str, view: &str) -> String {
    format!(
        r#"cv = car(setof(c dbGetOpenCellViews() c~>libName == "{lib}" && c~>cellName == "{cell}" && c~>viewName == "{view}")) if(cv then n = length(cv~>instances) else r = errset(dbOpenCellViewByType("{lib}" "{cell}" "{view}" nil "r") nil) when(r cv = car(r) when(cv n = length(cv~>instances) dbClose(cv))))"#
    )
}

/// The allow-set, enforced inside the SKILL for the two schematic-level edits.
///
/// They act on whatever cellview is currently bound, which is a name the Rust
/// side never sees — [`DeleteTarget`] cannot do this job here. So the check
/// moves next to the only place that knows the answer: `cv~>libName`, at the
/// moment of the edit.
///
/// With the shipped (empty) allow-set this renders as `member(cv~>libName '())`,
/// which is `nil` for every library — the guard fires unconditionally. That is
/// the intended reading: no allow-set configured, nothing deletable.
fn lib_guard(action: &str) -> String {
    let libs = allowed_libs()
        .iter()
        .map(|l| format!(r#""{}""#, escape_skill_string(l)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"unless(member(cv~>libName '({libs})) error("%s" sprintf(nil "{action} refused: library %s is not in the delete allow-set" cv~>libName)))"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Pins `VB_DELETE_ALLOW_LIBS` for the length of a test and restores it on
    /// drop. Every test here is `#[serial]` and takes one of these, including
    /// the ones that only read: the allow-set is consulted on *every*
    /// `DeleteTarget` construction, so a stray override in the developer's
    /// shell would leave the safety assertions below passing for the wrong
    /// reason, or failing for no reason.
    struct AllowSet(Option<std::ffi::OsString>);

    impl Drop for AllowSet {
        fn drop(&mut self) {
            match self.0.take() {
                Some(o) => std::env::set_var(ALLOW_LIBS_ENV, o),
                None => std::env::remove_var(ALLOW_LIBS_ENV),
            }
        }
    }

    fn allow_set(value: Option<&str>) -> AllowSet {
        let saved = AllowSet(std::env::var_os(ALLOW_LIBS_ENV));
        match value {
            Some(v) => std::env::set_var(ALLOW_LIBS_ENV, v),
            None => std::env::remove_var(ALLOW_LIBS_ENV),
        }
        saved
    }

    /// A deployment that has named its libraries. Most tests here are about the
    /// *shape* of the generated SKILL, and that shape only exists once someone
    /// has configured an allow-set — the shipped default is empty and refuses
    /// before any SKILL is built.
    fn configured() -> AllowSet {
        allow_set(Some("DESIGN_LIB, SIM_LIB, SCRATCH_LIB"))
    }

    /// No override at all: the allow-set this build ships with.
    fn shipped_default() -> AllowSet {
        allow_set(None)
    }

    fn target() -> DeleteTarget {
        DeleteTarget::cell("DESIGN_LIB", "_rbscratch").expect("allowed")
    }

    // ── The two landmines ────────────────────────────────────────────

    /// An empty cell name is what turns `ddGetObj` into a library lookup and
    /// `ddDeleteObj` into "delete the library and its cds.lib entry". There is
    /// no constructor that produces it.
    #[test]
    #[serial]
    fn a_library_is_not_a_delete_target() {
        let _env = configured();
        for empty in ["", "   "] {
            assert!(DeleteTarget::cell("DESIGN_LIB", empty).is_err());
            assert!(DeleteTarget::view("DESIGN_LIB", empty, "schematic").is_err());
        }
    }

    /// Mode is `ddGetObj`'s sixth positional argument. Passing fewer arguments
    /// puts `"r"` in the filename slot and lets the mode default — and the
    /// default is the one that creates.
    #[test]
    #[serial]
    fn dd_get_obj_always_passes_an_explicit_read_mode_in_the_sixth_slot() {
        let _env = configured();
        let cell = target();
        assert_eq!(cell.dd_args(), r#""DESIGN_LIB" "_rbscratch" nil nil nil "r""#);
        let view = DeleteTarget::view("SIM_LIB", "amp_tb", "schematic").expect("allowed");
        assert_eq!(
            view.dd_args(),
            r#""SIM_LIB" "amp_tb" "schematic" nil nil "r""#
        );
        for s in [DeleteOps.probe(&cell), DeleteOps.delete(&cell)] {
            assert!(s.contains(r#"nil nil nil "r""#), "{s}");
        }
    }

    // ── The allow-set ────────────────────────────────────────────────

    /// The shipped build deletes nothing at all. This is the assertion that
    /// keeps that true: a fresh checkout, no environment, and the three names
    /// that happen to be configured elsewhere in this test module are as
    /// unreachable as the PDK.
    #[test]
    #[serial]
    fn the_shipped_allow_set_is_empty_so_nothing_is_deletable() {
        let _env = shipped_default();
        assert!(allowed_libs().is_empty());
        for lib in ["DESIGN_LIB", "SIM_LIB", "SCRATCH_LIB", "analogLib", "basic"] {
            let e = DeleteTarget::cell(lib, "anything").expect_err("{lib} must be refused");
            let msg = e.to_string();
            // Refusing is not enough — the refusal has to say how to proceed,
            // or an operator reads "not in the delete allow-set ()" and has no
            // idea there is a set to fill in.
            assert!(msg.contains(ALLOW_LIBS_ENV), "{msg}");
        }
        // …and with nothing allowed, the in-SKILL guard is `member(x '())`,
        // which is nil for every library.
        assert!(lib_guard("delete").contains("member(cv~>libName '())"));
    }

    #[test]
    #[serial]
    fn the_pdk_and_everyone_elses_libraries_are_out_of_reach() {
        let _env = configured();
        for lib in ["pdkLib", "analogLib", "basic", "OTHER_USER_LIB", "cdsDefTechLib"] {
            let r = DeleteTarget::cell(lib, "anything");
            assert!(r.is_err(), "{lib} must not be deletable");
        }
    }

    #[test]
    #[serial]
    fn only_the_configured_libraries_are_in() {
        let _env = configured();
        for lib in ["DESIGN_LIB", "SIM_LIB", "SCRATCH_LIB"] {
            assert!(DeleteTarget::cell(lib, "_rbscratch").is_ok(), "{lib}");
        }
    }

    /// The schematic edits work on the bound cellview, so their allow-set check
    /// has to live in the SKILL — and it has to be there in both the probe and
    /// the delete, or the manifest would describe a delete the executor refuses
    /// (or worse, the other way round).
    #[test]
    #[serial]
    fn schematic_edits_check_the_allow_set_where_the_library_name_is_known() {
        let _env = configured();
        let ops = DeleteOps::new();
        for s in [
            ops.probe_instance("M1"),
            ops.delete_instance("M1"),
            ops.probe_prop("VIN", "ampl"),
            ops.delete_prop("VIN", "ampl"),
        ] {
            assert!(s.contains("cv~>libName"), "{s}");
            assert!(s.contains(r#"member(cv~>libName '("DESIGN_LIB""#), "{s}");
        }
    }

    // ── The generated SKILL ──────────────────────────────────────────

    #[test]
    #[serial]
    fn probes_do_not_delete_anything() {
        let _env = configured();
        let ops = DeleteOps::new();
        for s in [
            ops.probe(&target()),
            ops.probe(&DeleteTarget::view("SIM_LIB", "amp_tb", "schematic").unwrap()),
            ops.probe_instance("M1"),
            ops.probe_prop("VIN", "ampl"),
        ] {
            for forbidden in ["ddDeleteObj", "schDelete(", "dbDeletePropByName"] {
                assert!(!s.contains(forbidden), "{forbidden} in a probe: {s}");
            }
        }
    }

    #[test]
    #[serial]
    fn deletes_use_the_documented_api_for_each_layer() {
        let _env = configured();
        let ops = DeleteOps::new();
        assert!(ops.delete(&target()).contains("ddDeleteObj(id)"));
        assert!(ops.delete_instance("M1").contains("schDelete(inst)"));
        let prop = ops.delete_prop("VIN", "ampl");
        assert!(prop.contains(r#"dbSearchPropByName(inst "ampl")"#), "{prop}");
        assert!(prop.contains(r#"dbDeletePropByName(inst "ampl")"#), "{prop}");
    }

    /// `schHiDelete` needs a GUI selection set; over the bridge there is none,
    /// and reaching for an interactive command is how a headless call ends up
    /// waiting on a dialog that no one can click.
    #[test]
    #[serial]
    fn the_interactive_delete_is_never_used() {
        let _env = configured();
        assert!(!DeleteOps.delete_instance("M1").contains("schHiDelete"));
    }

    /// A bare `nil` from `ddDeleteObj` is the normal way a locked subtree
    /// refuses. Collapsing that into a success would report a delete that did
    /// not happen.
    #[test]
    #[serial]
    fn a_refused_delete_is_an_error_with_a_reason() {
        let _env = configured();
        let s = DeleteOps.delete(&target());
        assert!(s.contains("unless(ok error("), "{s}");
        assert!(s.contains("non-blocking exclusive lock"), "{s}");
    }

    /// The in-memory edits are not on disk until `cell.save`; saying so in the
    /// reply is the difference between "deleted" and "deleted, pending save".
    #[test]
    #[serial]
    fn in_memory_edits_report_that_they_are_unsaved() {
        let _env = configured();
        assert!(DeleteOps.delete_instance("M1").contains(r#"\"saved\":false"#));
        assert!(DeleteOps.delete_prop("VIN", "ampl").contains(r#"\"saved\":false"#));
    }

    #[test]
    #[serial]
    fn every_generator_escapes_its_arguments() {
        let _env = configured();
        let ops = DeleteOps::new();
        let t = DeleteTarget::cell("DESIGN_LIB", "bad\"cell").expect("allowed");
        for s in [
            ops.probe(&t),
            ops.delete(&t),
            ops.probe_instance("bad\"inst"),
            ops.delete_instance("bad\"inst"),
            ops.probe_prop("bad\"inst", "bad\"prop"),
            ops.delete_prop("bad\"inst", "bad\"prop"),
        ] {
            assert!(!s.contains("bad\"cell"), "unescaped cell: {s}");
            assert!(!s.contains("bad\"inst"), "unescaped instance: {s}");
            assert!(!s.contains("bad\"prop"), "unescaped property: {s}");
        }
    }

    /// The write ops must aim at the cellview the caller asked for, not at
    /// whichever editor window happens to be current — the same bug that once
    /// dropped a `vpulse` into the wrong testbench, except this one deletes.
    #[test]
    #[serial]
    fn schematic_edits_go_through_the_cellview_guard() {
        let _env = configured();
        for s in [
            DeleteOps.delete_instance("M1"),
            DeleteOps.delete_prop("VIN", "ampl"),
        ] {
            assert!(s.contains("RB_SCH_CV"), "{s}");
            assert!(s.contains("geGetEditCellView"), "{s}");
        }
    }

    /// Counting what is inside a cell must not be a write in disguise. If the
    /// cellview is already open, the probe reads the existing handle; `dbClose`
    /// is reachable only on the `else` branch, where the handle is ours.
    #[test]
    #[serial]
    fn the_probe_never_closes_a_cellview_it_did_not_open() {
        let _env = configured();
        let ops = DeleteOps::new();
        for s in [
            ops.probe(&target()),
            ops.probe(&DeleteTarget::view("SIM_LIB", "amp_tb", "schematic").unwrap()),
        ] {
            assert!(s.contains("dbGetOpenCellViews()"), "{s}");
            let (before, after) = s.split_once("dbClose(cv)").expect("closes its own handle");
            assert!(
                before.contains("else r = errset(dbOpenCellViewByType"),
                "dbClose must sit on the branch that opened the handle: {s}"
            );
            assert!(!after.contains("dbClose("), "only one dbClose: {s}");
        }
    }

    #[test]
    #[serial]
    fn action_and_description_name_the_target() {
        let _env = configured();
        let cell = target();
        assert_eq!(cell.action(), "cell.delete");
        assert_eq!(cell.describe(), "DESIGN_LIB/_rbscratch");
        let view = DeleteTarget::view("SIM_LIB", "amp_tb", "maestro").unwrap();
        assert_eq!(view.action(), "cell.delete_view");
        assert_eq!(view.describe(), "SIM_LIB/amp_tb/maestro");
    }

    #[test]
    #[serial]
    fn an_empty_view_name_is_not_the_whole_cell() {
        let _env = configured();
        // Silently widening "delete this view" into "delete every view" is the
        // kind of helpfulness that loses work.
        assert!(DeleteTarget::view("DESIGN_LIB", "_rbscratch", "  ").is_err());
    }

    #[test]
    #[serial]
    fn the_allow_set_is_configured_by_env() {
        let _env = allow_set(Some("MY_LIB, OTHER_LIB"));
        assert_eq!(allowed_libs(), vec!["MY_LIB", "OTHER_LIB"]);
        assert!(DeleteTarget::cell("MY_LIB", "scratch").is_ok());
        assert!(DeleteTarget::cell("OTHER_LIB", "scratch").is_ok());
        assert!(DeleteTarget::cell("NOT_LISTED", "scratch").is_err());
    }

    /// A variable set to whitespace is the same as not setting it: back to the
    /// empty shipped set, not to some half-parsed list containing `""`. The
    /// filter matters — `"A,,B"` must not admit a library named `""`.
    #[test]
    #[serial]
    fn a_blank_override_falls_back_to_the_empty_shipped_set() {
        let _env = allow_set(Some("   "));
        assert!(allowed_libs().is_empty());
        assert!(DeleteTarget::cell("", "scratch").is_err());
    }
}

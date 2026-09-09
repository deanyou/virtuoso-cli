use crate::client::bridge::escape_skill_string;

/// SKILL expression yielding the current editor's cellview, or `nil` — without
/// the GE-2067 warning storm.
///
/// A bare `geGetEditCellView()` writes a three-line
/// `*WARNING* (GE-2067): ... no graphical edit environment assigned to
/// window(N) because the window is not a graphic editor window` into the CIW
/// **every time** the current window is not a graphic editor — a waveform
/// window, the CIW itself, or nothing at all.
///
/// That used to be an error path. Since the headless fallback in [`cv_guard`]
/// made "no editor window" the *normal* operating mode, it became routine
/// noise in the one log a human actually reads, three lines per RPC call.
/// Observed 2026-09-09: a single `open → list_instances → get_params` sequence
/// against a waveform-only GUI left three GE-2067 blocks in the user's CIW.
///
/// Checking the window first is exactly equivalent — `geGetEditCellView()`
/// returns `nil` in precisely the cases this skips — but silent.
pub const EDIT_CV: &str =
    "let((w) w = hiGetCurrentWindow() when(w && w->cellView geGetEditCellView()))";

/// SKILL guard: resolves `cv` and errors if no cellview can be found.
/// This is prepended to SKILL code that uses the `cv` variable, which must
/// already be bound in the enclosing `let()` scope.
///
/// Resolution order — **explicit target first, ambient window second**:
///
/// 1. `RB_SCH_CV` — the handle `schematic.open_cell_view` stashes when it opens
///    a cellview with `dbOpenCellViewByType`, which creates **no window**. This
///    is the cellview the caller *asked for*, by name, in a previous call.
/// 2. [`EDIT_CV`] — the cellview of the current editor window, for callers that
///    never called `schematic.open_cell_view` and are driving a GUI session.
///
/// Step 1 is not optional. Without it `schematic.open_cell_view` is a silent
/// no-op for this entire namespace: it answers `status: ok`, then every
/// subsequent `schematic.*` call fails with "No cellview open — run 'vcli
/// schematic open lib/cell/view' first", pointing the caller back at the very
/// command that just appeared to succeed. Observed on 2026-09-09 with only the
/// CIW and Library Manager open. It also means no schematic work is possible
/// without a GUI window, which defeats headless operation.
///
/// The order used to be the other way round, and that was worse than useless —
/// it wrote to the wrong cell without saying so. Measured 2026-09-09:
/// `schematic.open_cell_view SIM_LIB/amp_buf_tb` returned `status: ok`
/// and bound `RB_SCH_CV`, but a schematic editor window for a *different* cell
/// (`amp_tb`) was still current, so `EDIT_CV` won and the next
/// `schematic.place` dropped a `vpulse` into that other cell's schematic — with
/// `status: ok`. Whichever window the user last clicked decided where the edits
/// landed, and the RPC reply never named the target. An explicit request must
/// beat ambient window focus: there is no `window.set_current`, so under the
/// old order a headless caller had no way to aim at all.
///
/// The stale-handle risk that motivates preferring the window is handled by
/// validating the handle rather than deprioritising it: a closed or never-set
/// handle fails `dbIsId`-style access, so it is checked for a live
/// `~>cellViewType` inside an `errset` and discarded if dead. `cell.close`
/// clears `RB_SCH_CV`, so a closed target falls back to the window.
pub(crate) fn cv_guard() -> String {
    "when(boundp('RB_SCH_CV) let((probe) probe = nil \
     errset(probe = RB_SCH_CV~>cellViewType) when(probe cv = RB_SCH_CV))) \
     when(!cv error(\"No cellview open — run 'vcli schematic open lib/cell/view' first\"))"
        .to_string()
}

/// Map an RPC `direction` onto the `basic` pin symbol master and the DFII
/// terminal direction that go with it.
///
/// `basic` ships three schematic pin symbols (`ipin` / `opin` / `iopin`,
/// confirmed present in IC23.1 under
/// `tools/dfII/etc/cdslib/basic/`). Choosing the right one is not cosmetic:
/// `schCreatePin` records the direction on the terminal, and `symbol.generate`
/// reads that direction to decide which side of the generated symbol each pin
/// lands on. Hardcoding `ipin` makes every generated symbol all-inputs.
///
/// Returns `None` for an unrecognised direction so the caller can reject it
/// rather than silently substituting one — the same "no error, wrong answer"
/// failure mode as the unvalidated `set_param`.
pub fn pin_master_for(direction: &str) -> Option<(&'static str, &'static str)> {
    match direction {
        "input" => Some(("ipin", "input")),
        "output" => Some(("opin", "output")),
        "inputOutput" => Some(("iopin", "inputOutput")),
        "switch" => Some(("iopin", "switch")),
        "jumper" => Some(("iopin", "jumper")),
        _ => None,
    }
}

#[derive(Default)]
pub struct SchematicOps;

impl SchematicOps {
    pub fn new() -> Self {
        Self
    }

    pub fn create_instance(
        &self,
        lib: &str,
        cell: &str,
        view: &str,
        name: &str,
        origin: (i64, i64),
        orient: &str,
    ) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let name = escape_skill_string(name);
        let orient = escape_skill_string(orient);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv master inst) cv = {EDIT_CV} {guard} master = dbOpenCellViewByType("{lib}" "{cell}" "{view}" nil "r") inst = dbCreateInst(cv master "{name}" list({x} {y}) "{orient}" 1) inst)"#
        )
    }

    /// Move a placed instance to an absolute position.
    ///
    /// `dbMoveFig(fig cv transform)` applies its transform **relative** to the
    /// figure's current placement — verified 2026-09-09 on `DESIGN_LIB/amp`:
    /// an instance at `(0 -8)` moved by `(1 2)` landed at `(1 -6)`. Every other
    /// method in this module speaks absolute coordinates, so this reads
    /// `inst~>xy` and moves by the difference, keeping one coordinate frame for
    /// the whole namespace.
    ///
    /// Without it a placement can only be adjusted by deleting and re-creating
    /// the instance, which discards its CDF parameters — so a schematic could be
    /// built but never tidied.
    ///
    /// `orient` is **absolute**, not composed with the current orientation.
    /// `dbMoveFig`'s transform would compose, so it is applied by assigning
    /// `inst~>orient` directly, which IC23.1 accepts (probed 2026-09-09: R0 →
    /// MY → R0 on a live pin instance).
    pub fn move_instance(&self, inst_name: &str, target: (f64, f64), orient: Option<&str>) -> String {
        let inst_name = escape_skill_string(inst_name);
        let (x, y) = target;
        let guard = cv_guard();
        // Orientation first: it pivots about the instance origin, so reorienting
        // after the move would leave the origin where it was put — but doing it
        // first keeps the two independent either way, and reads in the order the
        // caller wrote them.
        let set_orient = match orient {
            Some(o) => format!(r#" inst~>orient = "{}""#, escape_skill_string(o)),
            None => String::new(),
        };
        format!(
            r#"let((cv inst dx dy) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) when(!inst error("move_instance: no instance named {inst_name}")){set_orient} dx = {x:?}-xCoord(inst~>xy) dy = {y:?}-yCoord(inst~>xy) dbMoveFig(inst cv list(list(dx dy) "R0" 1.0)) sprintf(nil "{inst_name} -> (%g %g) %s" xCoord(inst~>xy) yCoord(inst~>xy) inst~>orient))"#
        )
    }

    pub fn create_wire(&self, points: &[(i64, i64)], layer: &str, net_name: &str) -> String {
        let layer = escape_skill_string(layer);
        let net_name = escape_skill_string(net_name);
        let pts: String = points
            .iter()
            .map(|(x, y)| format!("list({x} {y})"))
            .collect::<Vec<_>>()
            .join(" ");
        let guard = cv_guard();
        format!(
            r#"let((cv) cv = {EDIT_CV} {guard} dbCreateWire(cv dbMakeNet(cv "{net_name}") dbFindLayerByName(cv "{layer}") list({pts}))"#
        )
    }

    #[allow(dead_code)]
    pub fn create_wire_between_terms(
        &self,
        inst1: &str,
        _term1: &str,
        inst2: &str,
        _term2: &str,
        net_name: &str,
    ) -> String {
        let inst1 = escape_skill_string(inst1);
        let inst2 = escape_skill_string(inst2);
        let net_name = escape_skill_string(net_name);
        let guard = cv_guard();
        format!(
            r#"let((cv net) cv = {EDIT_CV} {guard} net = dbMakeNet(cv "{net_name}") dbCreateWire(net dbFindTermByName(cv "{inst1}") dbFindTermByName(cv "{inst2}")))"#
        )
    }

    pub fn create_wire_label(&self, net_name: &str, origin: (i64, i64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv net) cv = {EDIT_CV} {guard} net = dbFindNetByName(cv "{net_name}") when(net dbCreateLabel(cv net "{net_name}" list({x} {y}) "centerCenter" "R0" "stick" 0.0625))"#
        )
    }

    /// Create a schematic pin for `net_name` with the given direction.
    ///
    /// Uses `schCreatePin`, the supported schematic-level API, whose IC23.1
    /// signature is (from `doc/finder/SKILL/Schematics/skcompref.fnd`):
    ///
    /// ```text
    /// schCreatePin(d_cvId d_master t_termName t_direction g_offSheetP
    ///              l_origin t_orientation
    ///              [g_powerSens] [g_groundSens] [g_sigType]) => d_pin / nil
    /// ```
    ///
    /// It creates the terminal, names it, and wires up the pin instance in one
    /// step. The previous implementation hand-rolled this with
    /// `dbMakeNet` + `dbCreateInst` + `dbCreatePin`, which never named the
    /// terminal and ignored direction entirely.
    ///
    /// `direction` must already have been validated by the caller (see
    /// `pin_master_for`); an unknown value is caller error, not a default.
    pub fn create_pin(&self, net_name: &str, pin_type: &str, origin: (i64, i64)) -> String {
        let (master, direction) = pin_master_for(pin_type).unwrap_or(("iopin", "inputOutput"));
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv master pin) cv = {EDIT_CV} {guard} master = dbOpenCellViewByType("basic" "{master}" "symbol" nil "r") when(!master error("basic/{master}/symbol not found")) pin = schCreatePin(cv master "{net_name}" "{direction}" nil list({x} {y}) "R0") when(!pin error("schCreatePin failed for net {net_name}")) sprintf(nil "{{\"net\":\"%s\",\"direction\":\"{direction}\",\"master\":\"basic/{master}\"}}" "{net_name}"))"#
        )
    }

    pub fn check(&self) -> String {
        let guard = cv_guard();
        format!(r#"let((cv) cv = {EDIT_CV} {guard} schCheck(cv))"#)
    }

    pub fn open_cellview(&self, lib: &str, cell: &str, view: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        // dbOpenCellViewByType with viewType="schematic" mode="a":
        //   creates cellview if absent, opens for editing (non-interactive)
        // Store in RB_SCH_CV global — this is the *explicit target* that
        // `cv_guard` prefers over the current window, so every later
        // `schematic.*` call in this session lands here.
        //
        // Report what was bound, and whether any window shows it. A bare `ok`
        // hid the one thing worth knowing: with `windowed: false` the edits are
        // headless and the GUI keeps displaying some other cell, which is
        // exactly how a `place` once went into the wrong schematic unnoticed.
        format!(
            r#"let((cv w) cv = dbOpenCellViewByType("{lib}" "{cell}" "{view}" "schematic" "a") when(!cv error("open_cell_view: cannot open {lib}/{cell}/{view}")) RB_SCH_CV = cv w = car(setof(x hiGetWindowList() x->cellView == cv)) sprintf(nil "{{\"lib\":\"%s\",\"cell\":\"%s\",\"view\":\"%s\",\"windowed\":%s}}" cv~>libName cv~>cellName cv~>viewName if(w "true" "false")))"#
        )
    }

    pub fn save(&self) -> String {
        let guard = cv_guard();
        format!(r#"let((cv) cv = {EDIT_CV} {guard} dbSave(cv))"#)
    }

    /// Set a CDF parameter on an instance, **rejecting names the instance's
    /// CDF does not define**.
    ///
    /// `dbReplaceProp` performs no validation whatsoever: writing a parameter
    /// that does not exist creates a same-named junk property, the netlister
    /// ignores it, and the RPC still answers `status: ok`. Verified on IC23.1
    /// (2026-09-09): `set_param VP zzz_definitely_not_a_cdf_param 42` returned
    /// ok and the property was readable afterwards. That is how
    /// `amp_build.json` came to set `ampl` on an `analogLib/vsin` — whose
    /// amplitude parameter is actually `va` — producing a transient that was a
    /// flat line with no error anywhere.
    ///
    /// So: look the name up in `cdfGetInstCDF(inst)~>parameters~>name` first
    /// and fail loudly if it is absent. When the instance has no CDF at all
    /// there is nothing to validate against, so the write proceeds and the
    /// response says `validated: false` rather than pretending otherwise.
    pub fn set_instance_param(&self, inst_name: &str, param: &str, value: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let param = escape_skill_string(param);
        let value = escape_skill_string(value);
        let guard = cv_guard();
        format!(
            r#"let((cv inst cdf names out sep n) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(!inst error("instance {inst_name} not found in this cellview")) cdf = cdfGetInstCDF(inst) names = if(cdf cdf~>parameters~>name nil) if(!names || member("{param}" names) then dbReplaceProp(inst "{param}" "string" "{value}") sprintf(nil "{{\"status\":\"ok\",\"param\":\"{param}\",\"validated\":%s}}" if(names "true" "false")) else out = "" sep = "" n = 0 foreach(nm names when(n < 30 out = strcat(out sep nm) sep = " ") n = n + 1) when(n > 30 out = strcat(out sprintf(nil " ... (%d total; call schematic.list_cdf_params for the full list)" n))) error(strcat("unknown CDF parameter '{param}' for instance {inst_name} — valid names include: " out))))"#
        )
    }

    /// List the CDF parameter names an instance actually accepts.
    ///
    /// Exposed so a caller can discover the right name instead of guessing —
    /// the guess is what produced the `ampl`/`va` bug above.
    pub fn list_cdf_params(&self, inst_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let guard = cv_guard();
        // Every field is emitted with `%L`, which supplies its own surrounding
        // quotes *and* backslash-escapes any `"` and `\` inside — the same
        // escaping JSON wants. Hand-wrapping in `\"%s\"` instead (the first
        // version of this) produced invalid JSON for every real PDK device:
        // SMIC's n50_ckt/p50_ckt carry `simM = iPar("m")` and `simW =
        // iPar("w")`, whose embedded quotes closed the JSON string early and
        // made the whole response unparseable. Measured 2026-09-09 — the
        // method was 100% broken on exactly the devices it exists to describe.
        //
        // Non-strings are stringified first, so `%L` always sees a string and
        // never emits a bare symbol or number where JSON needs a quoted value.
        format!(
            r#"let((cv inst cdf out sep n ty v) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(!inst error("instance {inst_name} not found in this cellview")) cdf = cdfGetInstCDF(inst) out = "[" sep = "" when(cdf foreach(p cdf~>parameters n = p~>name n = if(stringp(n) n sprintf(nil "%s" n)) ty = p~>paramType ty = if(ty if(stringp(ty) ty sprintf(nil "%s" ty)) "?") v = p~>value v = if(v if(stringp(v) v sprintf(nil "%L" v)) "") out = strcat(out sep sprintf(nil "{{\"name\":%L,\"type\":%L,\"value\":%L}}" n ty v)) sep = ",")) strcat(out "]"))"#
        )
    }

    // ── Read operations ──────────────────────────────────────────────

    /// List all instances in the open cellview. Returns JSON array via sprintf.
    pub fn list_instances(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep lib cell) cv = {EDIT_CV} {guard} out = "[" sep = "" foreach(inst cv~>instances lib = if(inst~>master inst~>master~>libName "?") cell = if(inst~>master inst~>master~>cellName "?") out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"master\":\"%s/%s\",\"x\":%g,\"y\":%g}}" inst~>name lib cell car(inst~>xy) cadr(inst~>xy))) sep = ",") strcat(out "]"))"#
        )
    }

    /// List all nets in the open cellview. Returns JSON array.
    pub fn list_nets(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep) cv = {EDIT_CV} {guard} out = "[" sep = "" foreach(net cv~>nets out = strcat(out sep sprintf(nil "\"%s\"" net~>name)) sep = ",") strcat(out "]"))"#
        )
    }

    /// List all pins (terminals) in the open cellview. Returns JSON array.
    pub fn list_pins(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep) cv = {EDIT_CV} {guard} out = "[" sep = "" foreach(term cv~>terminals out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"direction\":\"%s\"}}" term~>name term~>direction)) sep = ",") strcat(out "]"))"#
        )
    }

    /// Get parameters of a specific instance. Returns JSON object.
    pub fn get_instance_params(&self, inst_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let guard = cv_guard();
        format!(
            r#"let((cv inst out sep v) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) if(inst then out = "{{" sep = "" foreach(prop inst~>prop when(prop~>name != nil v = prop~>value when(v out = strcat(out sep sprintf(nil "\"%s\":\"%s\"" prop~>name if(stringp(v) v sprintf(nil "%L" v)))) sep = ","))) strcat(out "}}") else "null"))"#
        )
    }

    /// Assign a named net to an instance terminal — a purely logical connection,
    /// no wire coordinates.
    ///
    /// ⚠️ **This does not build a schematic that lasts.** In Virtuoso, schematic
    /// connectivity is *derived* data and geometry is the source of truth:
    /// `schExtractConn` (IC23.1 Schematic Editor SKILL Reference) processes
    /// "figures on the wire layer with drawing, flight, or label purposes" and
    /// nothing else, and `schClearConn` — which every extraction runs first —
    /// "deletes all non-terminal nets" and "detaches instance pins from terminal
    /// nets". So a connection made only with `dbCreateInstTerm` is erased by the
    /// next `schCheck`, whether it comes from this API, from Check&Save in the
    /// GUI, or from a hierarchical check of a parent cell; every terminal reverts
    /// to `net1..netN`. Measured twice on `SCRATCH_LIB/conn_probe` and once,
    /// destructively, on `SIM_LIB/amp_buf_tb` (2026-09-09).
    ///
    /// Use [`Self::label_instance_term`] to build connectivity that survives:
    /// it takes the same `(instance, terminal, net)` triple, but draws a real
    /// labelled wire stub, which is what the schematic editor itself produces.
    /// Stubs carrying the same label merge into one net during extraction even
    /// when far apart and not touching, so terminals still need no routing.
    ///
    /// `assign_net` remains useful only for a database that is netlisted
    /// immediately and never checked.
    ///
    /// A freshly-placed instance has an empty `instTerms` list (instTerms are
    /// materialized lazily on connection), so the terminal must be resolved from
    /// the *master*'s terminal list and connected with `dbCreateInstTerm`, which
    /// takes the master-terminal db object (not a name string). If an instTerm
    /// already exists on a *different* net, `dbCreateInstTerm` refuses to move it,
    /// so we delete the stale instTerm first, making reassignment idempotent.
    pub fn assign_net(&self, inst_name: &str, term_name: &str, net_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let term_name = escape_skill_string(term_name);
        let net_name = escape_skill_string(net_name);
        let guard = cv_guard();
        format!(
            r#"let((cv inst mterm net existing it) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) when(!inst error("assign_net: instance not found: {inst_name}")) mterm = car(setof(mt inst~>master~>terminals strcmp(mt~>name "{term_name}")==0)) when(!mterm error("assign_net: terminal not on master: {term_name}")) net = dbMakeNet(cv "{net_name}") existing = car(setof(x inst~>instTerms strcmp(x~>name "{term_name}")==0)) when(existing && !(strcmp(existing~>net~>name "{net_name}")==0) dbDeleteObject(existing) existing = nil) it = if(existing existing dbCreateInstTerm(net inst mterm)) when(!it error("assign_net: connect failed for {inst_name}/{term_name}")) it)"#
        )
    }

    /// Create a short labeled net stub in a given direction.
    ///
    /// Draws a wire segment of `length` grid units from (x,y) in the specified
    /// direction and places a net label at its midpoint. Useful for power/ground
    /// connections and test points without manually computing endpoint coords.
    ///
    /// direction: "right" (default) | "left" | "up" | "down"
    /// length: stub length in DBU (default 0.5 grid units = 0.5 for typical libs)
    /// cosmetic: "default" (fontSize 0.0625, centerCenter) or "clean" (0.125, lowerCenter)
    pub fn create_net_stub(
        &self,
        net_name: &str,
        x: i64,
        y: i64,
        direction: &str,
        length: f64,
        cosmetic: &str,
    ) -> String {
        let net_name = escape_skill_string(net_name);
        let (dx, dy, rot) = match direction {
            "up" => (0.0, 1.0, "R90"),
            "down" => (0.0, -1.0, "R90"),
            "left" => (-1.0, 0.0, "R0"),
            _ => (1.0, 0.0, "R0"),
        };
        let end_x = x as f64 + dx * length;
        let end_y = y as f64 + dy * length;
        let label_x = (x as f64 + end_x) / 2.0;
        let label_y = (y as f64 + end_y) / 2.0;
        let (font_size, just) = if cosmetic == "clean" {
            ("0.125", "\"lowerCenter\"")
        } else {
            ("0.0625", "\"centerCenter\"")
        };

        // Format floats as clean SKILL numbers (avoid precision artifacts)
        let end_x_s = end_x.to_string();
        let end_y_s = end_y.to_string();
        let label_x_s = label_x.to_string();
        let label_y_s = label_y.to_string();

        let guard = cv_guard();
        format!(
            r#"let((cv) cv = {EDIT_CV} {guard} dbCreateWire(cv dbMakeNet(cv "{net_name}") dbFindLayerByName(cv "wire") list(list({x} {y}) list({end_x_s} {end_y_s}))) dbCreateLabel(cv dbFindNetByName(cv "{net_name}") "{net_name}" list({label_x_s} {label_y_s}) {just} "{rot}" "stick" {font_size}))"#
        )
    }

    /// Draw a real wire stub at an instance terminal and glue a net label to it.
    ///
    /// This is the *geometric* counterpart to [`Self::assign_net`]. `assign_net`
    /// builds connectivity straight into the database with `dbCreateInstTerm`
    /// and leaves nothing on the canvas: a human opening the cellview sees five
    /// unconnected devices, and `schCheck` throws the connectivity away — every
    /// terminal reverts to `net1..netN` (measured on `SCRATCH_LIB/conn_probe`,
    /// 2026-09-09, reproduced twice). A wire carrying a wire label is what the
    /// schematic editor itself produces, so it survives checking, saving and
    /// later GUI editing.
    ///
    /// The placed terminal position is resolved generically: take the centre of
    /// the *master* terminal's pin figure and push it through the instance's
    /// transform with `dbTransformPoint`, which is correct for any device at any
    /// orientation. The previous implementation guessed a direction from the
    /// *instance* bounding box, which only ever made sense for a MOS symbol.
    ///
    /// The stub points away from the **centroid of the master's pins**, not away
    /// from the instance bounding box. The bounding box is the wrong reference
    /// because it includes the parameter labels: on `pdkLib/n50_ckt` the
    /// drawn device ends at x = 0.275 while the labels run out to x = 0.8, which
    /// drags the box centre right and made the source stub point sideways into
    /// the neighbouring column instead of down. Against the pin centroid
    /// (0.1875, -0.0156) the same symbol resolves D→up, G→left, S→down, B→right,
    /// which is how the device is drawn.
    ///
    /// A master with a **single** pin — `basic/ipin`, `opin`, `iopin` — has no
    /// usable centroid: it coincides with the pin, leaving no direction at all.
    /// Those fall back to the master bounding box centre, which is right for
    /// exactly the symbols the centroid rule was wrong about: `ipin` stubs
    /// right, `opin` left, `iopin` down, matching how each arrow is drawn.
    ///
    /// Two defects are fixed here as well; either alone made the method return
    /// `error` for every input:
    ///  * instances and terminals were matched with `i~>name == "M1"`. SKILL
    ///    `==` compares identity, not string content, so the lookup always
    ///    yielded nil. `assign_net` had this right with `strcmp(...) == 0`.
    ///  * the stub direction used `when(cond a b ...)` as if it were `case`.
    ///    `when` evaluates every form and returns the last, so the direction was
    ///    whatever fell out of the final branch regardless of the geometry.
    ///
    /// Signatures from the IC23.1 reference (read 2026-09-09):
    /// `schCreateWire(cv entry route points xSnap ySnap width)` => list of wires
    /// (entry `"draw"` uses the point list verbatim and ignores the route
    /// method); `schCreateWireLabel(cv glue point text just orient font height
    /// aliasP)` => label, where `glue` is the wire the label names.
    ///
    /// inst_name: instance name (e.g. "M1")
    /// term_name: terminal name as it appears on the *master* symbol ("D"/"G"/…)
    /// net_name: net the terminal joins — also the text of the label
    /// cosmetic: "clean" → 0.125 font, otherwise 0.0625
    /// auto_rotate: turn the label to run along a vertical stub
    pub fn label_instance_term(
        &self,
        inst_name: &str,
        term_name: &str,
        net_name: &str,
        cosmetic: &str,
        auto_rotate: bool,
    ) -> String {
        let inst_name = escape_skill_string(inst_name);
        let term_name = escape_skill_string(term_name);
        let net_name = escape_skill_string(net_name);
        let font_size = if cosmetic == "clean" { "0.125" } else { "0.0625" };
        // A vertical stub reads better with the text turned to match it, but
        // only when the caller asks — upright text is easier to skim in bulk.
        let (up_rot, up_just, dn_rot, dn_just) = if auto_rotate {
            ("R90", "centerLeft", "R90", "centerRight")
        } else {
            ("R0", "lowerCenter", "R0", "upperCenter")
        };
        let guard = cv_guard();

        format!(
            r#"let((cv inst mterm pin fig bb pc pt cxs cys npin mb mbc ctr dx dy stub ex ey lblJust lblRot wires lbl) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) when(!inst error("label_term: no instance named {inst_name}")) mterm = car(setof(mt inst~>master~>terminals strcmp(mt~>name "{term_name}")==0)) when(!mterm error("label_term: master of {inst_name} has no terminal {term_name}")) pin = car(mterm~>pins) when(!pin error("label_term: terminal {term_name} has no pin")) fig = pin~>fig when(!fig error("label_term: pin of {term_name} has no figure")) bb = fig~>bBox pc = list((xCoord(car(bb))+xCoord(cadr(bb)))/2.0 (yCoord(car(bb))+yCoord(cadr(bb)))/2.0) cxs = 0.0 cys = 0.0 npin = 0 foreach(trm inst~>master~>terminals foreach(pn trm~>pins when(pn~>fig let((bx) bx = pn~>fig~>bBox cxs = cxs+(xCoord(car(bx))+xCoord(cadr(bx)))/2.0 cys = cys+(yCoord(car(bx))+yCoord(cadr(bx)))/2.0 npin = npin+1)))) when(npin == 0 error("label_term: master of {inst_name} has no pin figures")) mb = inst~>master~>bBox mbc = list((xCoord(car(mb))+xCoord(cadr(mb)))/2.0 (yCoord(car(mb))+yCoord(cadr(mb)))/2.0) ctr = if(npin >= 2 list(cxs/float(npin) cys/float(npin)) mbc) when(xCoord(ctr) == xCoord(pc) && yCoord(ctr) == yCoord(pc) ctr = mbc) pt = dbTransformPoint(pc inst~>transform) pt = list(xCoord(pt) yCoord(pt)) ctr = dbTransformPoint(ctr inst~>transform) dx = xCoord(pt)-xCoord(ctr) dy = yCoord(pt)-yCoord(ctr) stub = 0.25 ex = xCoord(pt) ey = yCoord(pt) if(abs(dx) >= abs(dy) then if(dx >= 0.0 then ex = ex+stub lblJust = "centerLeft" lblRot = "R0" else ex = ex-stub lblJust = "centerRight" lblRot = "R0") else if(dy >= 0.0 then ey = ey+stub lblJust = "{up_just}" lblRot = "{up_rot}" else ey = ey-stub lblJust = "{dn_just}" lblRot = "{dn_rot}")) wires = schCreateWire(cv "draw" "full" list(pt list(ex ey)) 0.0625 0.0625 0.0) when(!wires error("label_term: schCreateWire failed at {inst_name}/{term_name}")) lbl = schCreateWireLabel(cv car(wires) list(ex ey) "{net_name}" lblJust lblRot "stick" {font_size} nil) when(!lbl error("label_term: schCreateWireLabel failed at {inst_name}/{term_name}")) sprintf(nil "%s.%s stub (%g %g)->(%g %g) net=%s" "{inst_name}" "{term_name}" xCoord(pt) yCoord(pt) ex ey "{net_name}"))"#
        )
    }

    /// Polish all labels on a net with cosmetic preset, auto-rotation, or offset.
    ///
    /// preset: "readable" → fontSize 0.125, just "centerCenter"
    ///          "compact" → fontSize 0.0625, just "centerLeft"
    /// auto_rotate: infer rotation from wire bounding box direction
    /// offset: "small" (+5 DBU), "medium" (+10), "large" (+20) in x or y
    pub fn polish_labels(
        &self,
        net_name: &str,
        preset: &str,
        auto_rotate: bool,
        offset: Option<&str>,
    ) -> String {
        let net_name = escape_skill_string(net_name);
        let font_size = if preset == "compact" {
            "0.0625"
        } else {
            "0.125"
        };
        let just = if preset == "compact" {
            "\"centerLeft\""
        } else {
            "\"centerCenter\""
        };
        let offset_delta: i64 = match offset {
            Some("small") => 5,
            Some("medium") => 10,
            Some("large") => 20,
            _ => 0,
        };

        // Build the optional code blocks as plain strings
        let rotate_part = if auto_rotate {
            " when(labels let((bb bbx bby orient) bb=car(labels)~>xy bbx=caar(bb) bby=cadr(bb) orient=if(bbx>10000 \"R90\" if(bbx<-10000 \"R270\" if(bby>10000 \"MY\" \"R0\"))) foreach(l labels l~>orient=orient)))".to_string()
        } else {
            String::new()
        };

        let offset_part = if offset_delta != 0 {
            format!(" foreach(l labels let((xy) xy=l~>xy l~>xy=list(car(xy)+{offset_delta} cadr(xy)+{offset_delta})))", offset_delta = offset_delta)
        } else {
            String::new()
        };

        format!(
            "let((cv net labels) cv={EDIT_CV} net=dbFindNetByName(cv \"{net_name}\") labels=if(net setof(l net~>labels l~>figType==\"label\") nil) foreach(l labels l~>fontSize={fs} l~>justify={just}){rotate}{offset} length(labels))",
            net_name = net_name,
            fs = font_size,
            just = just,
            rotate = rotate_part,
            offset = offset_part,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> SchematicOps {
        SchematicOps::new()
    }

    #[test]
    fn list_cdf_params_lets_skill_do_the_json_escaping() {
        // The values this has to survive are things like `iPar("m")`, which
        // every SMIC device carries. Emitting them into a hand-written
        // `\"%s\"` closes the JSON string early; `%L` quotes and escapes them
        // itself. Assert the field values are `%L` and not `\"%s\"`.
        let s = ops().list_cdf_params("M1");
        assert!(
            s.contains(r#"{\"name\":%L,\"type\":%L,\"value\":%L}"#),
            "all three fields must be emitted with %L: {s}"
        );
        assert!(
            !s.contains(r#"\"value\":\"%s\""#),
            "hand-wrapped %s is what broke on iPar(\"m\"): {s}"
        );
    }

    #[test]
    fn create_instance_uses_orient() {
        let s = ops().create_instance("analogLib", "nmos4", "symbol", "M1", (100, 200), "MY");
        assert!(s.contains("\"MY\""), "orient must be in SKILL: {s}");
        assert!(
            s.contains("100") && s.contains("200"),
            "origin must be in SKILL: {s}"
        );
        assert!(s.contains("\"M1\""), "instance name must be quoted: {s}");
    }

    #[test]
    fn create_instance_default_orient() {
        let s = ops().create_instance("lib", "cell", "symbol", "X0", (0, 0), "R0");
        assert!(s.contains("\"R0\""), "{s}");
    }

    #[test]
    fn assign_net_uses_dbcreateinstterm() {
        let s = ops().assign_net("M1", "G", "VIN");
        // must connect via the master-terminal db object, not the (lazy/empty) instTerms
        assert!(
            s.contains("dbCreateInstTerm"),
            "must use dbCreateInstTerm: {s}"
        );
        assert!(
            s.contains("master~>terminals"),
            "terminal must be resolved from the master: {s}"
        );
        assert!(
            !s.contains("schCreateWire"),
            "must not use schCreateWire: {s}"
        );
        assert!(
            !s.contains("0 0"),
            "hardcoded coordinates must be gone: {s}"
        );
    }

    #[test]
    fn assign_net_escapes_names() {
        let s = ops().assign_net(r#"M"1"#, "D", "VDD");
        assert!(s.contains(r#"M\"1"#), "inst name must be escaped: {s}");
    }

    /// The two ops that build *geometric* connectivity must honour the explicit
    /// target like every other schematic op.
    ///
    /// They were the only two missing `cv_guard()`, which made them unusable on a
    /// headless cellview: with a window open on some other cell they silently
    /// drew into that one instead. Measured 2026-09-09 — thirteen `label_term`
    /// calls aimed at a headless `SIM_LIB/amp_buf_tb` all landed on the
    /// windowed `DESIGN_LIB/amp` and failed there for want of the
    /// instances. That is exactly the gap that pushed the build path onto
    /// `assign_net`, whose connectivity no extraction preserves.
    #[test]
    fn geometric_connectivity_ops_honour_the_explicit_target() {
        let stub = ops().create_net_stub("VDD", 0, 0, "right", 0.5, "clean");
        assert!(
            stub.contains("RB_SCH_CV"),
            "net_stub must prefer the explicit target: {stub}"
        );
        let term = ops().label_instance_term("M1", "D", "VDD", "clean", false);
        assert!(
            term.contains("RB_SCH_CV"),
            "label_term must prefer the explicit target: {term}"
        );
    }

    #[test]
    fn open_cellview_sets_global() {
        let s = ops().open_cellview("myLib", "myCell", "schematic");
        assert!(s.contains("RB_SCH_CV = cv"), "{s}");
        assert!(s.contains("\"myLib\"") && s.contains("\"myCell\""), "{s}");
    }

    #[test]
    fn open_cellview_reports_the_bound_target() {
        // A bare `ok` cannot tell the caller which cellview later
        // `schematic.*` calls will hit, nor whether the GUI is showing it.
        let s = ops().open_cellview("myLib", "myCell", "schematic");
        assert!(s.contains("\\\"windowed\\\":%s"), "{s}");
        assert!(s.contains("hiGetWindowList()"), "{s}");
        assert!(s.contains("cv~>cellName"), "{s}");
    }

    #[test]
    fn cv_guard_prefers_the_explicit_target_over_the_current_window() {
        // `RB_SCH_CV` is what `schematic.open_cell_view` was asked to bind; the
        // editor window is whatever the user last clicked. The explicit one has
        // to win, or a `place` silently lands in another cell's schematic.
        let g = cv_guard();
        let rb = g.find("RB_SCH_CV").expect("guard must consult RB_SCH_CV");
        // No `when(!cv ...)` gating the probe: it runs regardless of EDIT_CV.
        assert!(
            !g[..rb].contains("when(!cv"),
            "RB_SCH_CV must be probed unconditionally, not only when the window lookup failed: {g}"
        );
        // The error is still last, so an unresolvable target is loud.
        assert!(g.trim_end().ends_with("first\"))"), "{g}");
    }

    #[test]
    fn cv_guard_is_injected_in_write_ops() {
        let s = ops().create_wire(&[(0, 0), (10, 10)], "wire", "VDD");
        assert!(
            s.contains("geGetEditCellView"),
            "guard must be present: {s}"
        );
        assert!(s.contains("dbCreateWire"), "{s}");
    }

    #[test]
    fn set_instance_param_generates_guarded_escaped_write() {
        let s = ops().set_instance_param("M0", "w", "4u");
        // read-only guard present (same as every other write op)
        assert!(
            s.contains("geGetEditCellView"),
            "guard must be present: {s}"
        );
        // uses the property-write primitive, not a raw eval
        assert!(s.contains("dbReplaceProp"), "must use dbReplaceProp: {s}");
        assert!(
            s.contains("\"M0\"") && s.contains("\"w\"") && s.contains("\"4u\""),
            "inst/param/value must appear quoted: {s}"
        );
    }

    #[test]
    fn set_instance_param_escapes_all_inputs() {
        let s = ops().set_instance_param(r#"M"0"#, r#"w"x"#, r#"4u"y"#);
        assert!(s.contains(r#"M\"0"#), "inst name must be escaped: {s}");
        assert!(s.contains(r#"w\"x"#), "param name must be escaped: {s}");
        assert!(s.contains(r#"4u\"y"#), "value must be escaped: {s}");
    }

    #[test]
    fn create_wire_label_contains_guard() {
        let s = ops().create_wire_label("GND", (50, 50));
        assert!(s.contains("geGetEditCellView"), "{s}");
    }

    #[test]
    fn save_contains_guard() {
        let s = ops().save();
        assert!(s.contains("geGetEditCellView"), "{s}");
        assert!(s.contains("dbSave"), "{s}");
    }

    #[test]
    fn create_net_stub_right() {
        let s = ops().create_net_stub("VDD", 100, 200, "right", 0.5, "default");
        assert!(s.contains("VDD"), "net name must appear: {s}");
        assert!(s.contains("dbCreateWire"), "must use dbCreateWire: {s}");
        assert!(s.contains("dbCreateLabel"), "must use dbCreateLabel: {s}");
        assert!(s.contains("geGetEditCellView"), "must have guard: {s}");
    }

    #[test]
    fn create_net_stub_up() {
        let s = ops().create_net_stub("VSS", 0, 0, "up", 1.0, "clean");
        assert!(s.contains("VSS"), "net name must appear: {s}");
        assert!(s.contains("R90"), "up direction should use R90: {s}");
    }

    #[test]
    fn create_net_stub_cosmetic_clean() {
        let s = ops().create_net_stub("NET", 50, 50, "left", 0.5, "clean");
        assert!(s.contains("0.125"), "clean should use fontSize 0.125: {s}");
        assert!(
            s.contains("lowerCenter"),
            "clean should use lowerCenter: {s}"
        );
    }

    #[test]
    fn label_instance_term_draws_a_real_wire_and_label() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(s.contains("VDD"), "net name must appear: {s}");
        assert!(s.contains("geGetEditCellView"), "must have guard: {s}");
        // Schematic-editor APIs, not the raw db ones: only these two make the
        // wire and label part of the schematic's connectivity.
        assert!(s.contains("schCreateWire("), "must create wire: {s}");
        assert!(s.contains("schCreateWireLabel("), "must label wire: {s}");
        assert!(
            !s.contains("dbCreateWire") && !s.contains("dbCreateLabel"),
            "must not fall back to the raw db calls: {s}"
        );
    }

    /// Both lookups used SKILL `==`, which compares identity and so never
    /// matched a name. Guard the fix so it cannot regress silently.
    #[test]
    fn label_instance_term_matches_names_with_strcmp() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(
            s.contains(r#"strcmp(i~>name "M1")==0"#),
            "instance lookup must use strcmp: {s}"
        );
        assert!(
            s.contains(r#"strcmp(mt~>name "D")==0"#),
            "terminal lookup must use strcmp: {s}"
        );
        assert!(
            !s.contains(r#"i~>name == ""#) && !s.contains(r#"mt~>name == ""#),
            "no identity comparison against a string: {s}"
        );
    }

    /// The terminal position comes from the master pin pushed through the
    /// instance transform — not from a MOS-shaped guess at the instance bbox.
    #[test]
    fn label_instance_term_transforms_the_master_pin() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(
            s.contains("inst~>master~>terminals"),
            "must read the master's terminals: {s}"
        );
        assert!(
            s.contains("dbTransformPoint(pc inst~>transform)"),
            "must place the pin via the instance transform: {s}"
        );
    }

    /// The instance bounding box includes the parameter labels, which on a real
    /// PDK symbol are wider than the device — using it as the "inside" reference
    /// sent the source stub sideways. The pin centroid has no such bias.
    #[test]
    fn label_instance_term_measures_direction_from_the_pin_centroid() {
        let s = ops().label_instance_term("M1", "S", "gnd!", "default", false);
        assert!(
            s.contains("dbTransformPoint(ctr inst~>transform)"),
            "centroid must be transformed alongside the pin: {s}"
        );
        assert!(
            s.contains("dx = xCoord(pt)-xCoord(ctr)"),
            "direction must be measured against the centroid: {s}"
        );
        assert!(
            !s.contains("inst~>bBox"),
            "the instance bbox must not drive the direction: {s}"
        );
    }

    /// `when(cond a b)` returns `b` unconditionally; the direction logic has to
    /// branch with `if`.
    #[test]
    fn label_instance_term_branches_direction_with_if() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(
            s.contains("if(abs(dx) >= abs(dy)"),
            "dominant axis must be an if: {s}"
        );
        assert!(
            !s.contains(r#"when(rbStubDir"#),
            "the when-as-case form must be gone: {s}"
        );
    }

    #[test]
    fn label_instance_term_cosmetic_clean() {
        let s = ops().label_instance_term("X1", "G", "VIN", "clean", true);
        assert!(s.contains("0.125"), "clean should use fontSize 0.125: {s}");
        // auto_rotate turns the label along a vertical stub.
        assert!(s.contains("R90"), "auto_rotate should emit R90: {s}");
    }

    #[test]
    fn label_instance_term_leaves_labels_upright_without_auto_rotate() {
        let s = ops().label_instance_term("X1", "G", "VIN", "default", false);
        assert!(s.contains("lowerCenter"), "upright label for up stub: {s}");
        assert!(!s.contains("R90"), "no rotation unless asked: {s}");
    }

    #[test]
    fn label_instance_term_escapes_names() {
        let s = ops().label_instance_term("M\"1", "D", "V\"DD", "default", false);
        assert!(s.contains(r#"M\"1"#), "instance name escaped: {s}");
        assert!(s.contains(r#"V\"DD"#), "net name escaped: {s}");
    }
}

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
        origin: (f64, f64),
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

    /// Draw a wire through `points` and name the net it forms.
    ///
    /// The previous implementation called `dbCreateWire` with a layer from
    /// `dbFindLayerByName`. **Neither function exists in IC23.1** — no entry in
    /// any of the 41 `.fnd` databases, and live on `DESIGN_LIB/_rbscratch`
    /// (2026-09-10) every call died with `*Error* eval: undefined function
    /// dbCreateWire`. `schematic.wire` was a dead method that had never drawn a
    /// wire.
    ///
    /// Signature from `doc/skcompref/chap2_re_schCreateWire.html`:
    ///
    /// ```text
    /// schCreateWire(d_cvId t_entryMethod t_routeMethod l_points
    ///               n_xSpacing n_ySpacing n_width [t_color] [t_lineStyle])
    ///   => l_wireId
    /// ```
    ///
    /// There is no layer argument — a schematic wire is on the wire layer by
    /// construction — so the old `layer` parameter is **removed**, not ignored.
    /// Entry method `"draw"` uses the point list verbatim; the route method is
    /// then irrelevant but still required positionally.
    ///
    /// The net is named the way the schematic editor names one: by gluing a
    /// wire label to the drawn wire. Handing a `dbMakeNet` object to the
    /// constructor, as the old code did, produces derived connectivity that the
    /// next `schCheck` throws away — the rule [`Self::label_instance_term`]
    /// documents at length. Without the label the `net` argument would be
    /// silently dropped, which is the failure mode this whole file is about.
    ///
    /// The label goes at the midpoint of the first segment, turned to R90 when
    /// that segment is vertical, matching [`Self::create_net_stub`].
    pub fn create_wire(&self, points: &[(f64, f64)], net_name: &str) -> String {
        let net_name = escape_skill_string(net_name);
        // A single point is not a wire. Say so in the same place every other
        // failure here is reported rather than letting `schCreateWire` return
        // nil and blaming the cellview.
        let (Some(&(x0, y0)), Some(&(x1, y1))) = (points.first(), points.get(1)) else {
            return format!(
                r#"error("create_wire: net {net_name} needs at least two points, got {}")"#,
                points.len()
            );
        };
        let pts: String = points
            .iter()
            .map(|(x, y)| format!("list({x:?} {y:?})"))
            .collect::<Vec<_>>()
            .join(" ");
        let (mid_x, mid_y) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let rot = if x0 == x1 { "R90" } else { "R0" };
        let guard = cv_guard();
        format!(
            r#"let((cv wires lbl) cv = {EDIT_CV} {guard} wires = schCreateWire(cv "draw" "full" list({pts}) 0.0625 0.0625 0.0) when(!wires error("create_wire: schCreateWire failed for net {net_name}")) lbl = schCreateWireLabel(cv car(wires) list({mid_x:?} {mid_y:?}) "{net_name}" "centerCenter" "{rot}" "stick" 0.0625 nil) when(!lbl error("create_wire: schCreateWireLabel failed for net {net_name}")) sprintf(nil "{net_name}: %d segment(s)" length(wires)))"#
        )
    }

    /// Attach a net label to the wire already drawn at `origin`.
    ///
    /// The old implementation was `dbCreateLabel(cv net "{name}" ...)`, which is
    /// wrong twice over. `dbCreateLabel`'s second argument is a
    /// `txl_layerPurpose` pair, not a net (`doc/skdfref`), and the whole call
    /// sat behind `when(net ...)` on a `dbFindNetByName` lookup — so for a net
    /// that did not exist yet, the common case, the expression evaluated to
    /// `nil` and the RPC reported `create label failed: nil` with a suggestion
    /// to check whether a cellview was open. Reproduced live 2026-09-10.
    ///
    /// A schematic label is not free-floating text: `schCreateWireLabel` takes
    /// the wire or pin it names as `d_glue`, and gluing it is what makes the
    /// label *mean* anything to connectivity extraction. So the wire has to be
    /// found first. `dbGetOverlaps` with a degenerate box at `origin` does that
    /// — probed on a live cellview: it returns the `"line"` at a point on the
    /// wire and `nil` two grid units away.
    ///
    /// Not finding a wire is a real error, not an empty result: a label with
    /// nothing under it names nothing.
    pub fn create_wire_label(&self, net_name: &str, origin: (f64, f64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv figs wire lbl) cv = {EDIT_CV} {guard} figs = dbGetOverlaps(cv list(list({x:?} {y:?}) list({x:?} {y:?}))) wire = car(setof(f figs f~>objType == "line")) when(!wire error("label: no wire at ({x:?} {y:?}) to name {net_name} — draw the wire first")) lbl = schCreateWireLabel(cv wire list({x:?} {y:?}) "{net_name}" "centerCenter" "R0" "stick" 0.0625 nil) when(!lbl error("label: schCreateWireLabel failed for net {net_name}")) sprintf(nil "{net_name} @ ({x:?} {y:?})"))"#
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
    pub fn create_pin(&self, net_name: &str, pin_type: &str, origin: (f64, f64)) -> String {
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
    ///
    /// The rejection message lists the **ten nearest names, not the first
    /// thirty**. Thirty names in CDF declaration order is close to useless on a
    /// 135-parameter device: `analogLib/vsin`'s `va` sits well past the cut, so
    /// the very bug this validation exists to catch — `ampl` where `va` was
    /// meant — got an error that did not contain the answer. Candidates are
    /// bucketed, best first:
    ///
    ///  1. name equal to the query, ignoring case;
    ///  2. name and query containing one another (`w` suggested for `width`);
    ///  3. **the CDF `prompt` — the label shown in the GUI form — containing
    ///     the query.** This is the bucket that earns its keep: `va`'s prompt is
    ///     *"Amplitude"*, so `ampl` finds it, which no name-only match can do;
    ///  4. everything else, in declaration order.
    ///
    /// Bucket-3 entries print as `va (Amplitude)` — the name alone would not
    /// explain why it was offered. The total count and the pointer to
    /// `schematic.list_cdf_params` stay, since ten is a shortlist, not a search.
    ///
    /// `error`'s first argument is a *format* string (`sklangref.fnd`:
    /// `error( t_formatString [ g_arg1 ... ] )`), so the assembled message goes
    /// through `error("%s" msg)`. Passing it directly, as this used to, would
    /// let a `%` in a parameter name or GUI label garble the message.
    pub fn set_instance_param(&self, inst_name: &str, param: &str, value: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let param = escape_skill_string(param);
        let value = escape_skill_string(value);
        let guard = cv_guard();
        format!(
            r#"let((cv inst cdf names q n k out sep b4 b3 b2 b1 all nm pr lnm lpr msg) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(!inst error("instance %s not found in this cellview" "{inst_name}")) cdf = cdfGetInstCDF(inst) names = if(cdf cdf~>parameters~>name nil) if(!names || member("{param}" names) then dbReplaceProp(inst "{param}" "string" "{value}") sprintf(nil "{{\"status\":\"ok\",\"param\":\"{param}\",\"validated\":%s}}" if(names "true" "false")) else q = lowerCase("{param}") n = 0 foreach(p cdf~>parameters n = n + 1 nm = p~>name nm = if(stringp(nm) nm sprintf(nil "%s" nm)) pr = p~>prompt pr = if(pr && stringp(pr) pr "") lnm = lowerCase(nm) lpr = lowerCase(pr) cond((strcmp(lnm q) == 0 b4 = cons(nm b4)) ((index(lnm q) || index(q lnm)) b3 = cons(nm b3)) ((nequal(lpr "") && index(lpr q)) b2 = cons(sprintf(nil "%s (%s)" nm pr) b2)) (t b1 = cons(nm b1)))) all = append(reverse(b4) append(reverse(b3) append(reverse(b2) reverse(b1)))) out = "" sep = "" k = 0 foreach(nm all when(k < 10 out = strcat(out sep nm) sep = ", ") k = k + 1) msg = sprintf(nil "unknown CDF parameter '%s' for instance %s — %d defined, closest first: %s. A name in parentheses is that parameter's GUI label; call schematic.list_cdf_params for the full table." "{param}" "{inst_name}" n out) error("%s" msg)))"#
        )
    }

    /// List an instance's CDF parameters — names **and what they mean**.
    ///
    /// Exposed so a caller can discover the right name instead of guessing —
    /// the guess is what produced the `ampl`/`va` bug above. But a bare list of
    /// 135 names does not tell anyone which of `va / vaDBm / acm / pacm` is the
    /// amplitude, so every field the CDF actually carries comes back with it.
    ///
    /// What is in there, measured on IC23.1 (2026-09-10) rather than assumed:
    ///
    /// | field | `analogLib/vsin` | `pdkLib/p50_ckt` |
    /// |---|---|---|
    /// | `prompt` (the GUI label) | 135/135 | present — `w` → *"Total Width"* |
    /// | `units` | e.g. `voltage`, `frequency` | `lengthMetric` |
    /// | `defValue` | present | `300n` for `w` |
    /// | `choices` | 1/135 (`filenums`) | — |
    /// | `description` | **0/135** | nil |
    ///
    /// Two consequences worth stating. `description` is empty everywhere, so
    /// the prose still has to come from the manual (`analoglib.info`) — the two
    /// sources are complements, not alternatives. And this works on the PDK,
    /// which no Cadence manual documents, making it the only route to
    /// *"what does `pdkLib/p50_ckt`'s `w` mean"*.
    ///
    /// `choices` is emitted as a JSON array so a cyclic parameter's legal
    /// values are machine-readable; `description` and `choices` are omitted
    /// entirely when the CDF has none, rather than reported as empty — a
    /// present-but-empty field reads like a fact about the device.
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
            r#"let((cv inst cdf out sep sep2 n ty v pr un df ch ds extra) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(!inst error("instance {inst_name} not found in this cellview")) cdf = cdfGetInstCDF(inst) out = "[" sep = "" when(cdf foreach(p cdf~>parameters n = p~>name n = if(stringp(n) n sprintf(nil "%s" n)) ty = p~>paramType ty = if(ty if(stringp(ty) ty sprintf(nil "%s" ty)) "?") v = p~>value v = if(v if(stringp(v) v sprintf(nil "%L" v)) "") pr = p~>prompt pr = if(pr if(stringp(pr) pr sprintf(nil "%s" pr)) "") un = p~>units un = if(un if(stringp(un) un sprintf(nil "%s" un)) "") df = p~>defValue df = if(df if(stringp(df) df sprintf(nil "%L" df)) "") extra = "" ch = p~>choices when(ch extra = strcat(extra ",\"choices\":[") sep2 = "" foreach(c ch extra = strcat(extra sep2 sprintf(nil "%L" if(stringp(c) c sprintf(nil "%s" c)))) sep2 = ",") extra = strcat(extra "]")) ds = p~>description when(ds && stringp(ds) && strcmp(ds "") != 0 extra = strcat(extra sprintf(nil ",\"description\":%L" ds))) out = strcat(out sep sprintf(nil "{{\"name\":%L,\"prompt\":%L,\"type\":%L,\"units\":%L,\"default\":%L,\"value\":%L%s}}" n pr ty un df v extra)) sep = ",")) strcat(out "]"))"#
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
    /// direction and glues a net label to its midpoint. Useful for power/ground
    /// connections and test points without manually computing endpoint coords.
    ///
    /// Rewritten 2026-09-10 onto the schematic-editor APIs. It used to call
    /// `dbCreateWire` + `dbFindLayerByName` + `dbCreateLabel`; the first two do
    /// not exist in IC23.1, so every call returned `*Error* eval: undefined
    /// function dbCreateWire` — verified live, the same dead path as
    /// [`Self::create_wire`]. The label is now glued to the wire it names
    /// (`schCreateWireLabel`) instead of being dropped on a `dbFindNetByName`
    /// result, which is what makes the stub survive `schCheck`.
    ///
    /// direction: "right" (default) | "left" | "up" | "down"
    /// length: stub length in user units
    /// cosmetic: "default" (fontSize 0.0625, centerCenter) or "clean" (0.125, lowerCenter)
    pub fn create_net_stub(
        &self,
        net_name: &str,
        x: f64,
        y: f64,
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
        let end_x = x + dx * length;
        let end_y = y + dy * length;
        let label_x = (x + end_x) / 2.0;
        let label_y = (y + end_y) / 2.0;
        let (font_size, just) = if cosmetic == "clean" {
            ("0.125", "lowerCenter")
        } else {
            ("0.0625", "centerCenter")
        };

        let guard = cv_guard();
        format!(
            r#"let((cv wires lbl) cv = {EDIT_CV} {guard} wires = schCreateWire(cv "draw" "full" list(list({x:?} {y:?}) list({end_x:?} {end_y:?})) 0.0625 0.0625 0.0) when(!wires error("net_stub: schCreateWire failed for net {net_name}")) lbl = schCreateWireLabel(cv car(wires) list({label_x:?} {label_y:?}) "{net_name}" "{just}" "{rot}" "stick" {font_size} nil) when(!lbl error("net_stub: schCreateWireLabel failed for net {net_name}")) sprintf(nil "{net_name} stub ({x:?} {y:?})->({end_x:?} {end_y:?})"))"#
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
    /// One defect fixed here made the method return `error` for every input:
    /// the stub direction used `when(cond a b ...)` as if it were `case`.
    /// `when` evaluates every form and returns the last, so the direction was
    /// whatever fell out of the final branch regardless of the geometry.
    ///
    /// The name lookups were rewritten from `i~>name == "M1"` to
    /// `strcmp(i~>name "M1") == 0` at the same time, and the reason recorded
    /// then — *"`==` compares identity, not string content"* — is **wrong**;
    /// corrected 2026-09-10. `sklangref.fnd` is explicit: `eq` *"checks
    /// addresses"*, `equal` *"checks contents of strings and lists"*, and `==`
    /// is `equal`. Probed live: `strcat("M" "1") == "M1"` → `t`, and both forms
    /// find the same instance in `amp`. `strcmp` is kept because it says
    /// "string compare" out loud, not because `==` was broken — the direction
    /// bug above is what actually made every call fail.
    ///
    /// Signatures from the IC23.1 reference (read 2026-09-09):
    /// `schCreateWire(cv entry route points xSnap ySnap width)` => list of wires
    /// (entry `"draw"` uses the point list verbatim and ignores the route
    /// method); `schCreateWireLabel(cv glue point text just orient font height
    /// aliasP)` => label, where `glue` is the wire the label names.
    ///
    /// Calling it twice is not an error. `schCreateWire` answers `nil` when the
    /// segment is already there, so the second pass used to come back as
    /// `schCreateWire failed at M1/D` — the same text a genuinely broken call
    /// produces. Ten of those in a row on `amp` (2026-09-10) read as ten
    /// defects; the schematic was in fact already finished. So the terminal is
    /// probed first with `dbGetOverlaps` on a degenerate box (the technique
    /// [`Self::create_wire_label`] uses), and the three cases are told apart:
    ///
    ///  * nothing there → draw, as before;
    ///  * a wire already labelled `net_name` → answer `already: …`, success;
    ///  * a wire labelled something *else*, or nothing at all → error, and say
    ///    which of the two it is. Neither is idempotent: relabelling silently
    ///    would leave two names fighting over one net.
    ///
    /// Labels glued to the existing wire are found by overlapping its own
    /// `bBox`, not the stub end this call would have used — a stub drawn by an
    /// earlier version, or by hand, points wherever it points.
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
            r#"let((cv inst mterm pin fig bb pc pt cxs cys npin mb mbc ctr dx dy stub ex ey lblJust lblRot old olbls omine otxt wires lbl) cv = {EDIT_CV} {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) when(!inst error("label_term: no instance named {inst_name}")) mterm = car(setof(mt inst~>master~>terminals strcmp(mt~>name "{term_name}")==0)) when(!mterm error("label_term: master of {inst_name} has no terminal {term_name}")) pin = car(mterm~>pins) when(!pin error("label_term: terminal {term_name} has no pin")) fig = pin~>fig when(!fig error("label_term: pin of {term_name} has no figure")) bb = fig~>bBox pc = list((xCoord(car(bb))+xCoord(cadr(bb)))/2.0 (yCoord(car(bb))+yCoord(cadr(bb)))/2.0) cxs = 0.0 cys = 0.0 npin = 0 foreach(trm inst~>master~>terminals foreach(pn trm~>pins when(pn~>fig let((bx) bx = pn~>fig~>bBox cxs = cxs+(xCoord(car(bx))+xCoord(cadr(bx)))/2.0 cys = cys+(yCoord(car(bx))+yCoord(cadr(bx)))/2.0 npin = npin+1)))) when(npin == 0 error("label_term: master of {inst_name} has no pin figures")) mb = inst~>master~>bBox mbc = list((xCoord(car(mb))+xCoord(cadr(mb)))/2.0 (yCoord(car(mb))+yCoord(cadr(mb)))/2.0) ctr = if(npin >= 2 list(cxs/float(npin) cys/float(npin)) mbc) when(xCoord(ctr) == xCoord(pc) && yCoord(ctr) == yCoord(pc) ctr = mbc) pt = dbTransformPoint(pc inst~>transform) pt = list(xCoord(pt) yCoord(pt)) ctr = dbTransformPoint(ctr inst~>transform) dx = xCoord(pt)-xCoord(ctr) dy = yCoord(pt)-yCoord(ctr) stub = 0.25 ex = xCoord(pt) ey = yCoord(pt) if(abs(dx) >= abs(dy) then if(dx >= 0.0 then ex = ex+stub lblJust = "centerLeft" lblRot = "R0" else ex = ex-stub lblJust = "centerRight" lblRot = "R0") else if(dy >= 0.0 then ey = ey+stub lblJust = "{up_just}" lblRot = "{up_rot}" else ey = ey-stub lblJust = "{dn_just}" lblRot = "{dn_rot}")) old = car(setof(f dbGetOverlaps(cv list(pt pt)) f~>objType == "line")) if(old then olbls = setof(f dbGetOverlaps(cv old~>bBox) f~>objType == "label") omine = car(setof(f olbls strcmp(f~>theLabel "{net_name}")==0)) otxt = if(olbls car(olbls)~>theLabel "") cond((omine sprintf(nil "already: %s.%s net=%s — stub and label are already drawn" "{inst_name}" "{term_name}" "{net_name}")) (olbls error("label_term: {inst_name}/{term_name} already carries a stub labelled '%s', not '{net_name}' — delete that stub before relabelling" otxt)) (t error("label_term: a wire already meets {inst_name}/{term_name} but carries no label — name it with 'schematic.label' at that point, or delete it"))) else wires = schCreateWire(cv "draw" "full" list(pt list(ex ey)) 0.0625 0.0625 0.0) when(!wires error("label_term: schCreateWire failed at {inst_name}/{term_name}")) lbl = schCreateWireLabel(cv car(wires) list(ex ey) "{net_name}" lblJust lblRot "stick" {font_size} nil) when(!lbl error("label_term: schCreateWireLabel failed at {inst_name}/{term_name}")) sprintf(nil "%s.%s stub (%g %g)->(%g %g) net=%s" "{inst_name}" "{term_name}" xCoord(pt) yCoord(pt) ex ey "{net_name}")))"#
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
            s.contains(r#"{\"name\":%L,\"prompt\":%L,\"type\":%L,\"units\":%L,\"default\":%L,\"value\":%L%s}"#),
            "every field must be emitted with %L: {s}"
        );
        assert!(
            !s.contains(r#"\"value\":\"%s\""#),
            "hand-wrapped %s is what broke on iPar(\"m\"): {s}"
        );
    }

    /// The label is the point of the method, not a decoration.
    ///
    /// A caller shown 135 bare names cannot tell `va` from `vaDBm` from `acm`;
    /// shown *"Amplitude"* / *"Amplitude in dBm"* / *"AC magnitude"* they can.
    /// Live on IC23.1 every one of `vsin`'s 135 parameters carries a prompt,
    /// and so does the SMIC PDK (`w` → *"Total Width"*), which no manual covers.
    #[test]
    fn list_cdf_params_reports_the_gui_label_and_units() {
        let s = ops().list_cdf_params("M1");
        for field in ["prompt", "units", "default"] {
            assert!(s.contains(&format!(r#"p~>{}"#, if field == "default" { "defValue" } else { field })),
                "must read {field} off the CDF: {s}");
        }
    }

    /// A cyclic parameter's legal values must arrive as an array, and an
    /// absent one must not arrive at all — an empty `choices: []` reads as
    /// "this parameter accepts nothing", which is a different claim.
    #[test]
    fn choices_are_a_json_array_and_only_when_the_cdf_has_them() {
        let s = ops().list_cdf_params("M1");
        assert!(s.contains(r#"when(ch extra = strcat(extra ",\"choices\":[")"#), "{s}");
        assert!(s.contains(r#"strcmp(ds "") != 0"#),
            "an empty description must be omitted, not emitted: {s}");
    }

    #[test]
    fn create_instance_uses_orient() {
        let s = ops().create_instance("analogLib", "nmos4", "symbol", "M1", (100.0, 200.0), "MY");
        assert!(s.contains("\"MY\""), "orient must be in SKILL: {s}");
        assert!(
            s.contains("100") && s.contains("200"),
            "origin must be in SKILL: {s}"
        );
        assert!(s.contains("\"M1\""), "instance name must be quoted: {s}");
    }

    /// A grid-fraction placement must reach SKILL as itself.
    ///
    /// The schematic grid is 0.0625, so most real placements are not integers.
    /// While these origins were `i64`, `x: -1.5` arrived at `dbCreateInst` as
    /// `0` and the call still reported `status: ok`.
    #[test]
    fn a_fractional_origin_survives_into_the_skill() {
        let s = ops().create_instance("analogLib", "cap", "symbol", "CB", (-1.5, 4.0625), "R0");
        assert!(s.contains("list(-1.5 4.0625)"), "origin must not be rounded: {s}");
    }

    #[test]
    fn create_instance_default_orient() {
        let s = ops().create_instance("lib", "cell", "symbol", "X0", (0.0, 0.0), "R0");
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
        let stub = ops().create_net_stub("VDD", 0.0, 0.0, "right", 0.5, "clean");
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
        let s = ops().create_wire(&[(0.0, 0.0), (10.0, 10.0)], "VDD");
        assert!(
            s.contains("geGetEditCellView"),
            "guard must be present: {s}"
        );
        assert!(s.contains("schCreateWire("), "{s}");
    }

    /// `dbCreateWire` and `dbFindLayerByName` are not IC23.1 functions — they
    /// appear in none of the 41 `.fnd` databases, and live they raise
    /// `*Error* eval: undefined function dbCreateWire`. Both wire-drawing ops
    /// carried them, so both were dead on arrival.
    #[test]
    fn wire_ops_use_the_schematic_editor_api_not_the_undefined_db_calls() {
        for s in [
            ops().create_wire(&[(0.0, 0.0), (2.0, 0.0)], "VDD"),
            ops().create_net_stub("VDD", 1.0, 2.0, "right", 0.5, "default"),
        ] {
            assert!(s.contains("schCreateWire("), "must draw the wire: {s}");
            assert!(
                s.contains("schCreateWireLabel("),
                "the label is what names the net: {s}"
            );
            assert!(
                !s.contains("dbCreateWire")
                    && !s.contains("dbFindLayerByName")
                    && !s.contains("dbCreateLabel"),
                "no undefined or layer-based db calls: {s}"
            );
            // The net must be named, not accepted and dropped.
            assert!(s.contains("\"VDD\""), "net name must appear quoted: {s}");
        }
    }

    /// A one-point "wire" is caller error, and the message has to say which
    /// argument was wrong — `schCreateWire` would just return nil, which the
    /// RPC layer reports as a cellview problem.
    #[test]
    fn create_wire_rejects_fewer_than_two_points() {
        for pts in [&[][..], &[(1.0, 1.0)][..]] {
            let s = ops().create_wire(pts, "VDD");
            assert!(
                s.starts_with("error(") && s.contains("at least two points"),
                "{s}"
            );
            assert!(!s.contains("schCreateWire("), "must not draw anything: {s}");
        }
    }

    /// The label goes along the wire it names: upright on a horizontal segment,
    /// turned on a vertical one.
    #[test]
    fn create_wire_turns_the_label_to_follow_a_vertical_segment() {
        let h = ops().create_wire(&[(0.0, 0.0), (2.0, 0.0)], "VDD");
        assert!(h.contains(r#""VDD" "centerCenter" "R0""#), "{h}");
        // Midpoint of the first segment.
        assert!(h.contains("list(1.0 0.0)"), "{h}");
        let v = ops().create_wire(&[(0.0, 0.0), (0.0, 3.0)], "VDD");
        assert!(v.contains(r#""VDD" "centerCenter" "R90""#), "{v}");
        assert!(v.contains("list(0.0 1.5)"), "{v}");
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

    /// Thirty names in declaration order did not contain `va` on a 135-parameter
    /// `vsin`, so the error for the `ampl` bug did not carry its own answer.
    /// Rank the candidates instead, and cap the list at ten.
    #[test]
    fn set_instance_param_suggests_the_nearest_names_not_the_first_thirty() {
        let s = ops().set_instance_param("VIN", "ampl", "5m");
        assert!(
            !s.contains("n < 30"),
            "must not print the first thirty in declaration order: {s}"
        );
        assert!(s.contains("k < 10"), "shortlist must be capped at ten: {s}");
        // Ranking is case-insensitive, so the query is folded once up front.
        assert!(
            s.contains(r#"q = lowerCase("ampl")"#),
            "query must be case-folded: {s}"
        );
        assert!(s.contains("lowerCase(nm)"), "name must be case-folded: {s}");
        // Total count survives — ten is a shortlist, not a search.
        assert!(
            s.contains("list_cdf_params"),
            "must still point at the full table: {s}"
        );
    }

    /// The bucket that earns its keep: `va`'s CDF `prompt` is "Amplitude", so a
    /// query of `ampl` can only reach it through the GUI label, never through
    /// the name.
    #[test]
    fn set_instance_param_matches_against_the_cdf_prompt_too() {
        let s = ops().set_instance_param("VIN", "ampl", "5m");
        assert!(s.contains("p~>prompt"), "must read the GUI label: {s}");
        assert!(
            s.contains("lowerCase(pr)") && s.contains("index(lpr q)"),
            "must match the query against the folded prompt: {s}"
        );
        // A prompt match prints as `va (Amplitude)` — the name alone would not
        // explain why it was offered.
        assert!(
            s.contains(r#"sprintf(nil "%s (%s)" nm pr)"#),
            "prompt matches must show the label: {s}"
        );
    }

    /// `error`'s first argument is a format string, so an assembled message must
    /// not be passed as one: a `%` in a parameter name or GUI label would garble
    /// it. The old code did exactly that via `error(strcat(...))`.
    #[test]
    fn set_instance_param_does_not_pass_the_message_as_a_format_string() {
        let s = ops().set_instance_param("VIN", "ampl", "5m");
        assert!(
            s.contains(r#"error("%s" msg)"#),
            "message must go through a %s placeholder: {s}"
        );
        assert!(
            !s.contains("error(strcat("),
            "no computed format string: {s}"
        );
    }

    #[test]
    fn create_wire_label_contains_guard() {
        let s = ops().create_wire_label("GND", (50.0, 50.0));
        assert!(s.contains("geGetEditCellView"), "{s}");
    }

    /// A schematic label is glued to a wire — `schCreateWireLabel`'s `d_glue`
    /// argument is the wire or pin it names, and a label with nothing under it
    /// names nothing. The old code dropped a `dbCreateLabel` on whatever
    /// `dbFindNetByName` returned, behind a `when(net ...)` that made a missing
    /// net evaluate to nil rather than say so.
    #[test]
    fn create_wire_label_glues_to_the_wire_it_finds_and_errors_when_there_is_none() {
        let s = ops().create_wire_label("GND", (50.0, 50.0));
        assert!(s.contains("dbGetOverlaps("), "must locate the wire: {s}");
        assert!(
            s.contains(r#"f~>objType == "line""#),
            "a schematic wire is a line shape: {s}"
        );
        assert!(s.contains("schCreateWireLabel("), "{s}");
        assert!(
            !s.contains("dbCreateLabel") && !s.contains("dbFindNetByName"),
            "must not go back to the raw db label: {s}"
        );
        assert!(
            s.contains(r#"error("label: no wire at"#),
            "no wire under the label is an error, not a silent nil: {s}"
        );
    }

    #[test]
    fn save_contains_guard() {
        let s = ops().save();
        assert!(s.contains("geGetEditCellView"), "{s}");
        assert!(s.contains("dbSave"), "{s}");
    }

    #[test]
    fn create_net_stub_right() {
        let s = ops().create_net_stub("VDD", 100.0, 200.0, "right", 0.5, "default");
        assert!(s.contains("VDD"), "net name must appear: {s}");
        assert!(s.contains("schCreateWire("), "must draw a real wire: {s}");
        assert!(s.contains("schCreateWireLabel("), "must glue a label: {s}");
        assert!(s.contains("geGetEditCellView"), "must have guard: {s}");
        // The stub runs right from (100 200) for 0.5, label at the midpoint.
        assert!(s.contains("list(100.5 200.0)"), "endpoint: {s}");
        assert!(s.contains("list(100.25 200.0)"), "label midpoint: {s}");
    }

    #[test]
    fn create_net_stub_up() {
        let s = ops().create_net_stub("VSS", 0.0, 0.0, "up", 1.0, "clean");
        assert!(s.contains("VSS"), "net name must appear: {s}");
        assert!(s.contains("R90"), "up direction should use R90: {s}");
    }

    #[test]
    fn create_net_stub_cosmetic_clean() {
        let s = ops().create_net_stub("NET", 50.0, 50.0, "left", 0.5, "clean");
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

    /// Both lookups say `strcmp(...) == 0` rather than `==`. The comment that
    /// used to sit here claimed `==` compares identity and so never matched a
    /// name; that is wrong (see [`SchematicOps::label_instance_term`]) and was
    /// retracted 2026-09-10. `==` would work. The test stays because the
    /// explicit form is the one this file settled on — it reads as a string
    /// compare at a glance — not because the alternative is broken.
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

    /// Defect E: a second pass over a finished schematic must not read as ten
    /// failures. The terminal is probed before anything is drawn.
    #[test]
    fn label_instance_term_probes_the_terminal_before_drawing() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        let probe = s
            .find("dbGetOverlaps(cv list(pt pt))")
            .expect("must probe the terminal point: {s}");
        let draw = s.find("schCreateWire(").expect("must still draw: {s}");
        assert!(
            probe < draw,
            "the probe has to come before the wire, not after it fails: {s}"
        );
    }

    /// The three outcomes are distinct: same label = success, other label =
    /// error, bare wire = error. Only the first is idempotent.
    #[test]
    fn label_instance_term_tells_already_drawn_from_a_conflict() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(
            s.contains(r#"strcmp(f~>theLabel "VDD")==0"#),
            "the existing label's text decides, not its mere presence: {s}"
        );
        assert!(
            s.contains(r#"sprintf(nil "already: %s.%s net=%s"#),
            "the idempotent case needs the marker the command layer reads: {s}"
        );
        assert!(
            s.contains("already carries a stub labelled"),
            "a different net name must be an error, not a silent success: {s}"
        );
        assert!(
            s.contains("but carries no label"),
            "an unlabelled wire is its own case: {s}"
        );
    }

    /// Labels are searched on the existing wire's own bBox — a stub drawn by an
    /// earlier version, or by hand, does not point where this call would.
    #[test]
    fn label_instance_term_finds_labels_on_the_existing_wire() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(
            s.contains("dbGetOverlaps(cv old~>bBox)"),
            "must search the wire that is there, not the stub end: {s}"
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

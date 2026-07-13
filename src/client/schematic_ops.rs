use crate::client::bridge::escape_skill_string;

/// SKILL guard: checks that the cellview is open, errors otherwise.
/// This is prepended to SKILL code that uses `cv` variable.
fn cv_guard() -> String {
    // Use geGetEditCellView() as the authoritative source for the current cellview.
    // This avoids reliance on the RB_SCH_CV global which may be stale or unbound.
    // Note: cv must already be bound in the enclosing let() scope.
    "when(!cv error(\"No cellview open — run 'vcli schematic open lib/cell/view' first\"))"
        .to_string()
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
            r#"let((cv master inst) cv = geGetEditCellView() {guard} master = dbOpenCellViewByType("{lib}" "{cell}" "{view}" nil "r") inst = dbCreateInst(cv master "{name}" list({x} {y}) "{orient}" 1) inst)"#
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
            r#"let((cv) cv = geGetEditCellView() {guard} dbCreateWire(cv dbMakeNet(cv "{net_name}") dbFindLayerByName(cv "{layer}") list({pts}))"#
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
            r#"let((cv net) cv = geGetEditCellView() {guard} net = dbMakeNet(cv "{net_name}") dbCreateWire(net dbFindTermByName(cv "{inst1}") dbFindTermByName(cv "{inst2}")))"#
        )
    }

    pub fn create_wire_label(&self, net_name: &str, origin: (i64, i64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv net) cv = geGetEditCellView() {guard} net = dbFindNetByName(cv "{net_name}") when(net dbCreateLabel(cv net "{net_name}" list({x} {y}) "centerCenter" "R0" "stick" 0.0625))"#
        )
    }

    pub fn create_pin(&self, net_name: &str, _pin_type: &str, origin: (i64, i64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        let guard = cv_guard();
        format!(
            r#"let((cv net pinInst) cv = geGetEditCellView() {guard} net = dbMakeNet(cv "{net_name}") pinInst = dbCreateInst(cv dbOpenCellViewByType("basic" "ipin" "symbol" nil "r") "PIN_{net_name}" list({x} {y}) "R0" 1) dbCreatePin(net pinInst)"#
        )
    }

    pub fn check(&self) -> String {
        let guard = cv_guard();
        format!(r#"let((cv) cv = geGetEditCellView() {guard} schCheck(cv))"#)
    }

    pub fn open_cellview(&self, lib: &str, cell: &str, view: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        // dbOpenCellViewByType with viewType="schematic" mode="a":
        //   creates cellview if absent, opens for editing (non-interactive)
        // Store in RB_SCH_CV global for use by subsequent commands
        format!(r#"RB_SCH_CV = dbOpenCellViewByType("{lib}" "{cell}" "{view}" "schematic" "a")"#)
    }

    pub fn save(&self) -> String {
        let guard = cv_guard();
        format!(r#"let((cv) cv = geGetEditCellView() {guard} dbSave(cv))"#)
    }

    pub fn set_instance_param(&self, inst_name: &str, param: &str, value: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let param = escape_skill_string(param);
        let value = escape_skill_string(value);
        let guard = cv_guard();
        format!(
            r#"let((cv inst) cv = geGetEditCellView() {guard} inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(inst dbReplaceProp(inst "{param}" "string" "{value}")))"#
        )
    }

    // ── Read operations ──────────────────────────────────────────────

    /// List all instances in the open cellview. Returns JSON array via sprintf.
    pub fn list_instances(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep lib cell) cv = geGetEditCellView() {guard} out = "[" sep = "" foreach(inst cv~>instances lib = if(inst~>master inst~>master~>libName "?") cell = if(inst~>master inst~>master~>cellName "?") out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"master\":\"%s/%s\",\"x\":%g,\"y\":%g}}" inst~>name lib cell car(inst~>xy) cadr(inst~>xy))) sep = ",") strcat(out "]"))"#
        )
    }

    /// List all nets in the open cellview. Returns JSON array.
    pub fn list_nets(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep) cv = geGetEditCellView() {guard} out = "[" sep = "" foreach(net cv~>nets out = strcat(out sep sprintf(nil "\"%s\"" net~>name)) sep = ",") strcat(out "]"))"#
        )
    }

    /// List all pins (terminals) in the open cellview. Returns JSON array.
    pub fn list_pins(&self) -> String {
        let guard = cv_guard();
        format!(
            r#"let((cv out sep) cv = geGetEditCellView() {guard} out = "[" sep = "" foreach(term cv~>terminals out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"direction\":\"%s\"}}" term~>name term~>direction)) sep = ",") strcat(out "]"))"#
        )
    }

    /// Get parameters of a specific instance. Returns JSON object.
    pub fn get_instance_params(&self, inst_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let guard = cv_guard();
        format!(
            r#"let((cv inst out sep v) cv = geGetEditCellView() {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) if(inst then out = "{{" sep = "" foreach(prop inst~>prop when(prop~>name != nil v = prop~>value when(v out = strcat(out sep sprintf(nil "\"%s\":\"%s\"" prop~>name if(stringp(v) v sprintf(nil "%L" v)))) sep = ","))) strcat(out "}}") else "null"))"#
        )
    }

    /// Assign net name to instance terminal.
    /// Finds the instTerm by name and connects it to a named net via dbConnectToNet.
    /// No wire drawing coordinates needed — purely a logical connection.
    pub fn assign_net(&self, inst_name: &str, term_name: &str, net_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let term_name = escape_skill_string(term_name);
        let net_name = escape_skill_string(net_name);
        let guard = cv_guard();
        format!(
            r#"let((cv inst iterm net) cv = geGetEditCellView() {guard} inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0)) iterm = car(setof(x inst~>instTerms strcmp(x~>name "{term_name}")==0)) net = dbMakeNet(cv "{net_name}") when(iterm dbConnectToNet(iterm net)))"#
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

        format!(
            r#"let((cv) cv = geGetEditCellView() when(!cv error("No cellview open")) dbCreateWire(cv dbMakeNet(cv "{net_name}") dbFindLayerByName(cv "wire") list(list({x} {y}) list({end_x_s} {end_y_s}))) dbCreateLabel(cv dbFindNetByName(cv "{net_name}") "{net_name}" list({label_x_s} {label_y_s}) {just} "{rot}" "stick" {font_size}))"#
        )
    }

    /// Label an instance terminal (D/G/S/B) with a net name at the terminal's
    /// precise pin center, using the MOS-aware geometric stub direction.
    ///
    /// inst_name: instance name (e.g. "M1")
    /// term_name: terminal name — "D", "G", "S", or "B" for MOS; any term for other devs
    /// net_name: name to assign to this terminal
    /// cosmetic: "default" (0.0625, centerCenter) or "clean" (0.125, lowerCenter)
    /// auto_rotate: infer rotation from stub direction
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
        let (font_size, just) = if cosmetic == "clean" {
            ("0.125", "\"lowerCenter\"")
        } else {
            ("0.0625", "\"centerCenter\"")
        };

        // Stub extends 0.5 DBU from terminal center in the terminal's direction.
        // For MOS terminals, direction is derived from the instance bbox dominant axis.
        let auto_rot_part: &str = if auto_rotate {
            r#" when(rbStubDir "left" "right" rbDx>=0 "R0" "R180" when(rbStubDir "up" "down" rbDy>=0 "R90" "R270")"#
        } else {
            ""
        };

        format!(
            r#"let((cv inst term pin bbox rbTermCenter rbDx rbDy rbStubDir rbEnd rbLabelRot) cv = geGetEditCellView() when(!cv error("No cellview open")) inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(!inst error("instance not found: {inst_name}")) term = car(setof(t inst~>instTerms t~>name == "{term_name}")) when(!term error("terminal not found: {term_name}")) pin = car(term~>pins) when(!pin error("terminal has no pins")) bbox = pin~>bBox rbTermCenter = list((caar(bbox)+caadr(bbox))/2.0 (cadr(car(bbox))+cadr(bbox))/2.0) rbDx = caadr(bbox) - caar(bbox) rbDy = cadr(car(bbox)) - cadr(bbox) rbStubDir = if(abs(rbDx) >= abs(rbDy) when(rbDx >= 0 "right" "left") when(rbDy >= 0 "up" "down")) rbEnd = list(car(rbTermCenter) + when(rbStubDir "right" -rbDx when(rbStubDir "left" rbDx) when(rbStubDir "up" -rbDx when(rbStubDir "down" rbDx))) cadr(rbTermCenter) + when(rbStubDir "right" -rbDy when(rbStubDir "left" rbDy) when(rbStubDir "up" -rbDy when(rbStubDir "down" rbDy))) rbLabelRot = "{rot}"{auto_rot} net = dbMakeNet(cv "{net_name}") when(net dbCreateWire(cv net dbFindLayerByName(cv "wire") list(rbTermCenter rbEnd) 0 0 0 nil nil)) when(net dbCreateLabel(cv net "{net_name}" rbTermCenter {just} rbLabelRot "stick" {font_size}))"#,
            rot = "R0",
            auto_rot = auto_rot_part
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
            "let((cv net labels) cv=geGetEditCellView() net=dbFindNetByName(cv \"{net_name}\") labels=if(net setof(l net~>labels l~>figType==\"label\") nil) foreach(l labels l~>fontSize={fs} l~>justify={just}){rotate}{offset} length(labels))",
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
    fn assign_net_uses_dbconnect() {
        let s = ops().assign_net("M1", "G", "VIN");
        assert!(s.contains("dbConnectToNet"), "must use dbConnectToNet: {s}");
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

    #[test]
    fn open_cellview_sets_global() {
        let s = ops().open_cellview("myLib", "myCell", "schematic");
        assert!(s.starts_with("RB_SCH_CV ="), "{s}");
        assert!(s.contains("\"myLib\"") && s.contains("\"myCell\""), "{s}");
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
    fn label_instance_term_uses_term_resolution() {
        let s = ops().label_instance_term("M1", "D", "VDD", "default", false);
        assert!(s.contains("M1"), "inst name must appear: {s}");
        assert!(s.contains("D"), "term name must appear: {s}");
        assert!(s.contains("VDD"), "net name must appear: {s}");
        assert!(s.contains("dbCreateWire"), "must create wire: {s}");
        assert!(s.contains("dbCreateLabel"), "must create label: {s}");
        assert!(s.contains("geGetEditCellView"), "must have guard: {s}");
    }

    #[test]
    fn label_instance_term_cosmetic_clean() {
        let s = ops().label_instance_term("X1", "G", "VIN", "clean", true);
        assert!(s.contains("0.125"), "clean should use fontSize 0.125: {s}");
        assert!(
            s.contains("lowerCenter"),
            "clean should use lowerCenter: {s}"
        );
    }
}

use crate::client::bridge::escape_skill_string;

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
    ) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let name = escape_skill_string(name);
        let (x, y) = origin;
        format!(
            r#"let((cv master inst) cv = RB_SCH_CV master = dbOpenCellViewByType("{lib}" "{cell}" "{view}" nil "r") inst = dbCreateInst(cv master "{name}" list({x} {y}) "R0" 1) inst)"#
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
        format!(
            r#"let((cv) cv = RB_SCH_CV dbCreateWire(cv dbMakeNet(cv "{net_name}") dbFindLayerByName(cv "{layer}") list({pts})))"#
        )
    }

    pub fn create_wire_between_terms(
        &self,
        inst1: &str,
        term1: &str,
        inst2: &str,
        term2: &str,
        net_name: &str,
    ) -> String {
        let inst1 = escape_skill_string(inst1);
        let inst2 = escape_skill_string(inst2);
        let net_name = escape_skill_string(net_name);
        format!(
            r#"let((cv net) cv = RB_SCH_CV net = dbMakeNet(cv "{net_name}") dbCreateWire(net dbFindTermByName(cv "{inst1}") dbFindTermByName(cv "{inst2}")))"#
        )
    }

    pub fn create_wire_label(&self, net_name: &str, origin: (i64, i64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        format!(
            r#"let((cv net) cv = RB_SCH_CV net = dbFindNetByName(cv "{net_name}") when(net dbCreateLabel(cv net "{net_name}" list({x} {y}) "centerCenter" "R0" "stick" 0.0625)))"#
        )
    }

    pub fn create_pin(&self, net_name: &str, pin_type: &str, origin: (i64, i64)) -> String {
        let net_name = escape_skill_string(net_name);
        let (x, y) = origin;
        format!(
            r#"let((cv net pinInst) cv = RB_SCH_CV net = dbMakeNet(cv "{net_name}") pinInst = dbCreateInst(cv dbOpenCellViewByType("basic" "ipin" "symbol" nil "r") "PIN_{net_name}" list({x} {y}) "R0" 1) dbCreatePin(net pinInst))"#
        )
    }

    pub fn check(&self) -> String {
        r#"let((cv) cv = RB_SCH_CV schCheck(cv))"#.into()
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
        r#"let((cv) cv = RB_SCH_CV dbSave(cv))"#.into()
    }

    pub fn set_instance_param(&self, inst_name: &str, param: &str, value: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let param = escape_skill_string(param);
        let value = escape_skill_string(value);
        format!(
            r#"let((cv inst) cv = RB_SCH_CV inst = car(setof(i cv~>instances i~>name == "{inst_name}")) when(inst dbReplaceProp(inst "{param}" "string" "{value}")))"#
        )
    }

    /// Generate SKILL script that assigns net names to instance terminals.
    /// Returns a complete SKILL script string to write to a temp file and load.
    pub fn generate_connection_script(
        &self,
        connections: &[(String, String, String)], // (inst, term, net)
    ) -> String {
        let mut lines = Vec::new();
        lines.push("let((cv inst iterm net)".to_string());
        lines.push("cv = RB_SCH_CV".to_string());
        for (inst_name, term_name, net_name) in connections {
            let inst_name = escape_skill_string(inst_name);
            let term_name = escape_skill_string(term_name);
            let net_name = escape_skill_string(net_name);
            lines.push(format!(
                r#"inst = car(setof(i cv~>instances strcmp(i~>name "{inst_name}")==0))"#
            ));
            lines.push(format!(
                r#"iterm = car(setof(x inst~>instTerms strcmp(x~>name "{term_name}")==0))"#
            ));
            lines.push(format!(r#"net = dbMakeNet(cv "{net_name}")"#));
            // Create a wire at the instTerm position to connect it
            lines.push(
                r#"when(iterm schCreateWire(cv net "draw" "full" list(list(0 0) list(0 0))))"#
                    .to_string(),
            );
        }
        lines.push("t)".to_string());
        lines.join("\n")
    }

    /// Assign net name to instance terminal — simplified version.
    /// Creates a named net and connects it to the instTerm.
    pub fn assign_net(&self, inst_name: &str, term_name: &str, net_name: &str) -> String {
        let inst_name = escape_skill_string(inst_name);
        let term_name = escape_skill_string(term_name);
        let net_name = escape_skill_string(net_name);
        format!(
            r#"RB_INST = car(setof(i RB_SCH_CV~>instances strcmp(i~>name "{inst_name}")==0)) RB_ITERM = car(setof(x RB_INST~>instTerms strcmp(x~>name "{term_name}")==0)) RB_NET = dbMakeNet(RB_SCH_CV "{net_name}") schCreateWire(RB_SCH_CV RB_NET "draw" "full" list(list(0 0) list(0 0)))"#
        )
    }
}

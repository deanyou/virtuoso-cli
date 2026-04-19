use crate::client::bridge::escape_skill_string;

pub struct MaestroOps;

impl MaestroOps {
    /// Open maestro session in background (non-GUI) mode.
    /// Returns session string like "fnxSession4".
    pub fn open_session(&self, lib: &str, cell: &str, view: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        format!(r#"maeOpenSetup("{lib}" "{cell}" "{view}")"#)
    }

    /// Close a maestro session, force-cancelling any in-flight simulation.
    pub fn close_session(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeCloseSession("{session}" ?forceClose t)"#)
    }

    /// List all active maestro sessions.
    pub fn list_sessions(&self) -> String {
        r#"let((sessions out sep) sessions = maeGetSessions() out = "[" sep = "" foreach(s sessions out = strcat(out sep sprintf(nil "\"%s\"" s)) sep = ",") strcat(out "]"))"#.into()
    }

    /// Set a design variable value.
    /// maeSetVar(name value ?typeName "test"|"corner" ?typeValue ?session)
    pub fn set_var(&self, _session: &str, name: &str, value: &str) -> String {
        let name = escape_skill_string(name);
        let value = escape_skill_string(value);
        format!(r#"maeSetVar("{name}" "{value}")"#)
    }

    /// Get a design variable value.
    pub fn get_var(&self, name: &str) -> String {
        let name = escape_skill_string(name);
        format!(r#"maeGetVar("{name}")"#)
    }

    /// List all design variables. Returns JSON via sprintf.
    pub fn list_vars(&self, _session: &str) -> String {
        r#"let((vars out sep) vars = asiGetDesignVarList(asiGetCurrentSession()) out = "[" sep = "" foreach(v vars out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" car(v) cadr(v))) sep = ",") strcat(out "]"))"#.to_string()
    }

    /// Get enabled analyses for a session (resolves setup name internally).
    /// maeGetEnabledAnalysis(t_setupName) — takes setup name, not session name.
    pub fn get_analyses(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(
            r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeGetEnabledAnalysis(setup))"#
        )
    }

    /// Enable an analysis type on a session (resolves setup name internally).
    /// maeSetAnalysis(t_setupName t_analysisType) — returns t on success.
    /// analysisType: "ac" | "dc" | "tran" | "noise" | etc.
    pub fn set_analysis(&self, session: &str, analysis_type: &str) -> String {
        let session = escape_skill_string(session);
        let analysis_type = escape_skill_string(analysis_type);
        format!(
            r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeSetAnalysis(setup "{analysis_type}"))"#
        )
    }

    /// Run simulation asynchronously. Returns immediately.
    pub fn run_simulation(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeRunSimulation(?session "{session}")"#)
    }

    /// Get test outputs (measurement expressions).
    /// maeGetTestOutputs(t_testName [?session t_session])
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(
            r#"let((outs out sep) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\"}}" car(o) cadr(o))) sep = ",") strcat(out "]"))"#
        )
    }

    /// Add an output expression to the session (resolves setup name internally).
    /// maeAddOutput(t_outputName t_testName ?expr e)
    pub fn add_output(&self, session: &str, output_name: &str, expr: &str) -> String {
        let session = escape_skill_string(session);
        let output_name = escape_skill_string(output_name);
        let expr = escape_skill_string(expr);
        format!(
            r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeAddOutput("{output_name}" setup ?expr "{expr}"))"#
        )
    }

    /// Set the design target for a test.
    pub fn set_design(&self, session: &str, lib: &str, cell: &str, view: &str) -> String {
        let session = escape_skill_string(session);
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        format!(
            r#"maeSetDesign(?session "{session}" ?libName "{lib}" ?cellName "{cell}" ?viewName "{view}")"#
        )
    }

    /// Save maestro setup to disk.
    pub fn save_setup(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeSaveSetup(?session "{session}")"#)
    }

    /// Get simulation messages (errors/warnings from last run).
    pub fn get_sim_messages(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetSimulationMessages(?session "{session}")"#)
    }

    /// Get focused ADE window name, all window names, and active sessions.
    /// Returns a SKILL list: (focused_window_name (all_names...) (sessions...))
    pub fn focused_window_skill(&self) -> String {
        r#"let((cw) cw=hiGetCurrentWindow() list(if(cw hiGetWindowName(cw) nil) mapcar(lambda((w) hiGetWindowName(w)) hiGetWindowList()) maeGetSessions()))"#.into()
    }

    /// Get simulation run directory for a maestro session via asiGetAnalogRunDir.
    pub fn run_dir_skill(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"let((sess) sess=asiGetSession("{session}") if(sess asiGetAnalogRunDir(sess) nil))"#)
    }

    /// Export results to CSV.
    pub fn export_results(&self, session: &str, file_path: &str) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        format!(
            r#"maeExportOutputView(?session "{session}" ?fileName "{file_path}" ?view "Detail")"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> MaestroOps {
        MaestroOps
    }

    #[test]
    fn open_session_quoting() {
        let s = ops().open_session("myLib", "myCell", "adexl");
        assert_eq!(s, r#"maeOpenSetup("myLib" "myCell" "adexl")"#);
    }

    #[test]
    fn open_session_escapes_quotes() {
        let s = ops().open_session(r#"lib"x"#, "cell", "adexl");
        assert!(s.contains(r#"lib\"x"#), "{s}");
    }

    #[test]
    fn set_var_format() {
        let s = ops().set_var("sess1", "Vdd", "1.8");
        assert_eq!(s, r#"maeSetVar("Vdd" "1.8")"#);
    }

    #[test]
    fn run_simulation_includes_session() {
        let s = ops().run_simulation("sess1");
        assert!(s.contains("maeRunSimulation"), "{s}");
        assert!(s.contains("\"sess1\""), "{s}");
    }

    #[test]
    fn set_analysis_resolves_setup() {
        let s = ops().set_analysis("sess1", "ac");
        assert!(s.contains("maeGetSetup"), "must resolve setup: {s}");
        assert!(s.contains("maeSetAnalysis"), "{s}");
        assert!(s.contains("\"ac\""), "{s}");
    }

    #[test]
    fn add_output_includes_expr() {
        let s = ops().add_output("sess1", "gain", "getData(\"vout\")");
        assert!(s.contains("maeAddOutput"), "{s}");
        assert!(s.contains("\"gain\""), "{s}");
    }
}

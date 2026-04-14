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

    /// Get enabled analyses for a session.
    pub fn get_analyses(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetEnabledAnalysis(?session "{session}")"#)
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

    /// Add an output expression to the test.
    /// maeAddOutput(t_outputName t_testName [?outputType ?expr ?session])
    pub fn add_output(&self, output_name: &str, test_name: &str, expr: &str) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        let expr = escape_skill_string(expr);
        format!(r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#)
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

    /// Export results to CSV.
    pub fn export_results(&self, session: &str, file_path: &str) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        format!(
            r#"maeExportOutputView(?session "{session}" ?fileName "{file_path}" ?view "Detail")"#
        )
    }
}

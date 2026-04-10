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

    /// Set a design variable value in a maestro session.
    pub fn set_var(&self, session: &str, name: &str, value: &str) -> String {
        let session = escape_skill_string(session);
        let name = escape_skill_string(name);
        let value = escape_skill_string(value);
        format!(r#"maeSetVar("{session}" "{name}" "{value}")"#)
    }

    /// Get a design variable value.
    pub fn get_var(&self, session: &str, name: &str) -> String {
        let session = escape_skill_string(session);
        let name = escape_skill_string(name);
        format!(r#"maeGetVar("{session}" "{name}")"#)
    }

    /// List all design variables. Returns JSON via sprintf.
    pub fn list_vars(&self, _session: &str) -> String {
        r#"let((vars out sep) vars = asiGetDesignVarList(asiGetCurrentSession()) out = "[" sep = "" foreach(v vars out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" car(v) cadr(v))) sep = ",") strcat(out "]"))"#.to_string()
    }

    /// Get enabled analyses for a session.
    pub fn get_analyses(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetEnabledAnalysis("{session}")"#)
    }

    /// Run simulation asynchronously. Returns immediately.
    pub fn run_simulation(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeRunSimulation("{session}")"#)
    }

    /// Get test outputs (measurement expressions).
    pub fn get_outputs(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(
            r#"let((outs out sep) outs = maeGetTestOutputs("{session}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\"}}" car(o) cadr(o))) sep = ",") strcat(out "]"))"#
        )
    }

    /// Add an output expression to the test.
    pub fn add_output(&self, session: &str, name: &str, expr: &str) -> String {
        let session = escape_skill_string(session);
        let name = escape_skill_string(name);
        let expr = escape_skill_string(expr);
        format!(r#"maeAddOutput("{session}" ?name "{name}" ?expression "{expr}")"#)
    }

    /// Set the design target for a test.
    pub fn set_design(&self, session: &str, lib: &str, cell: &str, view: &str) -> String {
        let session = escape_skill_string(session);
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        format!(r#"maeSetDesign("{session}" "{lib}" "{cell}" "{view}")"#)
    }

    /// Save maestro setup to disk.
    pub fn save_setup(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeSaveSetup("{session}")"#)
    }

    /// Get simulation messages (errors/warnings from last run).
    pub fn get_sim_messages(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetSimulationMessages("{session}")"#)
    }

    /// Export results to CSV.
    pub fn export_results(&self, session: &str, file_path: &str) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        format!(r#"maeExportOutputView("{session}" "{file_path}" "Detail")"#)
    }
}

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
    pub fn set_var(&self, name: &str, value: &str) -> String {
        let name = escape_skill_string(name);
        let value = escape_skill_string(value);
        format!(r#"maeSetVar("{name}" "{value}")"#)
    }

    /// Get a design variable value.
    /// maeGetVar(t_varname)
    pub fn get_var(&self, name: &str) -> String {
        let name = escape_skill_string(name);
        format!(r#"maeGetVar("{name}")"#)
    }

    /// List all design variables. Returns JSON.
    /// Uses asiGetDesignVarList for reliable variable listing.
    pub fn list_vars(&self) -> String {
        r#"let((vars out sep) vars = asiGetDesignVarList(asiGetCurrentSession()) out = "[" sep = "" foreach(v vars out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" car(v) cadr(v))) sep = ",") strcat(out "]"))"#.into()
    }

    /// Get enabled analyses for a test.
    /// maeGetEnabledAnalysis(?session t_session)
    pub fn get_analyses(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetEnabledAnalysis(?session "{session}")"#)
    }

    /// Run simulation asynchronously. Returns immediately.
    /// maeRunSimulation([?session ?runMode ?callback ?run ?waitUntilDone ?returnRunId])
    pub fn run_simulation(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeRunSimulation(?session "{session}")"#)
    }

    /// Get test outputs (measurement expressions) as JSON.
    /// Returns list of {name, type, signalName, expr} for each output.
    /// maeGetTestOutputs(t_testName [?session t_session])
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(r#"let((outs out sep) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\",\"signalName\":\"%s\",\"expr\":\"%s\"}}" o~>name o~>outputType o~>signalName o~>expr)) sep = ",") strcat(out "]"))"#)
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
    /// maeSaveSetup([?session])
    pub fn save_setup(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeSaveSetup(?session "{session}")"#)
    }

    // =========================================================================
    // Result Reading Functions
    // =========================================================================

    /// Open a history run for programmatic result access.
    /// maeOpenResults(?history t_historyRunId ?session t_session)
    pub fn open_results(&self, history: &str) -> String {
        let history = escape_skill_string(history);
        format!(r#"maeOpenResults(?history "{history}")"#)
    }

    /// Close the currently open results.
    pub fn close_results(&self) -> String {
        r#"maeCloseResults()"#.into()
    }

    /// List all test names that have results in the current history.
    /// maeGetResultTests()
    pub fn get_result_tests(&self) -> String {
        r#"let((tests out sep) tests = maeGetResultTests() out = "[" sep = "" foreach(t tests out = strcat(out sep sprintf(nil "\"%s\"" t)) sep = ",") strcat(out "]"))"#.into()
    }

    /// List all output names available for a given test in the current history.
    /// maeGetResultOutputs(?testName t_testName)
    pub fn get_result_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(r#"let((outs out sep) outs = maeGetResultOutputs(?testName "{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "\"%s\"" o)) sep = ",") strcat(out "]"))"#)
    }

    /// Get the value of a specific output for a specific test and corner.
    /// maeGetOutputValue(t_outputName t_testName [?cornerName t_cornerName])
    /// Returns the numeric value as a string, or "nil" if not available.
    pub fn get_output_value(&self, name: &str, test_name: &str, corner: Option<&str>) -> String {
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);
        match corner {
            Some(c) => {
                let c = escape_skill_string(c);
                format!(r#"maeGetOutputValue("{name}" "{test_name}" ?cornerName "{c}")"#)
            }
            None => {
                format!(r#"maeGetOutputValue("{name}" "{test_name}")"#)
            }
        }
    }

    /// Get the spec pass/fail status for an output.
    /// maeGetSpecStatus(t_outputName t_testName)
    /// Returns: "pass", "fail", or "nil" (no spec defined).
    pub fn get_spec_status(&self, name: &str, test_name: &str) -> String {
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);
        format!(r#"maeGetSpecStatus("{name}" "{test_name}")"#)
    }

    /// Get simulation messages (errors/warnings) from last run.
    /// maeGetSimulationMessages([?session])
    pub fn get_sim_messages(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetSimulationMessages(?session "{session}")"#)
    }

    /// List available history runs for the current Maestro session.
    /// Returns JSON array of history names.
    pub fn get_history_list(&self) -> String {
        r#"let((base histories out sep) base = getDirFiles(strcat(asiGetResultsDir(asiGetCurrentSession()) "/..")) histories = remove("maestro" remove("exprOutputs.log" base)) out = "[" sep = "" foreach(h histories when(h && !index(h ".") out = strcat(out sep sprintf(nil "\"%s\"" h)) sep = ",")) strcat(out "]"))"#.into()
    }

    /// Get the Maestro session ID for the current (most recently opened) session.
    /// Useful when session is opened via GUI rather than maeOpenSetup.
    pub fn get_current_session(&self) -> String {
        r#"let((sess out) sess = asiGetCurrentSession() out = if(sess then sess~>name else "nil"))"#.into()
    }

    /// Export Maestro results to CSV.
    /// maeExportOutputView(?session ?fileName ?view)
    pub fn export_results(&self, session: &str, file_path: &str) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        format!(
            r#"maeExportOutputView(?session "{session}" ?fileName "{file_path}" ?view "Detail")"#
        )
    }
}

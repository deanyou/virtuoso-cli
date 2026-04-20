use crate::client::bridge::escape_skill_string;
use crate::version::VirtuosoVersion;

pub struct MaestroOps;

impl MaestroOps {
    /// Returns session handle like `"fnxSession4"`.
    pub fn open_session(&self, lib: &str, cell: &str, view: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        format!(r#"maeOpenSetup("{lib}" "{cell}" "{view}")"#)
    }

    /// Force-closes the session, cancels any in-flight simulation.
    pub fn close_session(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeCloseSession("{session}" ?forceClose t)"#)
    }

    pub fn list_sessions(&self) -> String {
        skill_strings_to_json("maeGetSessions()")
    }

    /// Set a design variable value.
    /// maeSetVar(name value) — no session arg (IC23/IC25 compatible).
    pub fn set_var(&self, name: &str, value: &str) -> String {
        let name = escape_skill_string(name);
        let value = escape_skill_string(value);
        format!(r#"maeSetVar("{name}" "{value}")"#)
    }

    pub fn get_var(&self, name: &str) -> String {
        let name = escape_skill_string(name);
        format!(r#"maeGetVar("{name}")"#)
    }

    /// List all design variables. Returns JSON via sprintf.
    pub fn list_vars(&self) -> String {
        r#"let((vars out sep) vars = asiGetDesignVarList(asiGetCurrentSession()) out = "[" sep = "" foreach(v vars out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" car(v) cadr(v))) sep = ",") strcat(out "]"))"#.into()
    }

    /// Get enabled analyses — version-aware.
    ///
    /// IC23: `maeGetEnabledAnalysis(setupName)` — needs car(maeGetSetup(...)) first.
    /// IC25: `maeGetEnabledAnalysis(?session sessionName)` — direct keyword.
    pub fn get_analyses(&self, session: &str, version: VirtuosoVersion) -> String {
        let session = escape_skill_string(session);
        if version.is_ic25() {
            format!(r#"maeGetEnabledAnalysis(?session "{session}")"#)
        } else {
            format!(
                r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeGetEnabledAnalysis(setup))"#
            )
        }
    }

    /// Enable an analysis type — version-aware.
    ///
    /// IC23: `maeSetAnalysis(setupName analysisType)`.
    /// IC25: `maeSetAnalysis(analysisType ?session s ?enable t ?options \`(...))`.
    ///
    /// `options_skill_alist` is validated and converted at the command layer before this is called.
    pub fn set_analysis(
        &self,
        session: &str,
        analysis_type: &str,
        options_skill_alist: Option<&str>,
        version: VirtuosoVersion,
    ) -> String {
        let session = escape_skill_string(session);
        let analysis_type = escape_skill_string(analysis_type);
        if version.is_ic25() {
            let options_part = match options_skill_alist {
                Some(alist) => format!(" ?options `{alist}"),
                None => String::new(),
            };
            format!(
                r#"maeSetAnalysis("{analysis_type}" ?session "{session}" ?enable t{options_part})"#
            )
        } else {
            // IC23: positional — setup name first; options not supported in this path
            format!(
                r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeSetAnalysis(setup "{analysis_type}"))"#
            )
        }
    }

    /// Run simulation asynchronously. Returns immediately.
    pub fn run_simulation(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeRunSimulation(?session "{session}")"#)
    }

    /// Get test outputs — version-aware.
    ///
    /// IC23/IC25: maeGetTestOutputs(testName) — both use positional.
    /// IC25 additionally supports ?session keyword.
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(r#"let((outs out sep) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\",\"signalName\":\"%s\",\"expr\":\"%s\"}}" o~>name o~>outputType o~>signalName o~>expr)) sep = ",") strcat(out "]"))"#)
    }

    /// Add an output expression — version-aware.
    ///
    /// IC23: `maeAddOutput(outputName testName ?expr e)` via setup-resolved test name.
    /// IC25: `maeAddOutput(outputName testName ?expr e ?session s)`.
    pub fn add_output(
        &self,
        output_name: &str,
        test_name: &str,
        expr: &str,
        version: VirtuosoVersion,
    ) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        let expr = escape_skill_string(expr);
        if version.is_ic25() {
            // IC25: pass test name directly, use ?session when available
            format!(
                r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#
            )
        } else {
            // IC23: same positional form
            format!(
                r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#
            )
        }
    }

    pub fn set_design(&self, session: &str, lib: &str, cell: &str, view: &str) -> String {
        let session = escape_skill_string(session);
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        format!(
            r#"maeSetDesign(?session "{session}" ?libName "{lib}" ?cellName "{cell}" ?viewName "{view}")"#
        )
    }

    pub fn save_setup(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeSaveSetup(?session "{session}")"#)
    }

    pub fn get_sim_messages(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeGetSimulationMessages(?session "{session}")"#)
    }

    /// Get focused ADE window name, davSession, all window names, sessions, and run_dir in one RTT.
    ///
    /// Returns a 5-element SKILL list:
    ///   (title davSession (all_titles...) (sessions...) run_dir_or_nil)
    ///
    /// `davSession` is `cw->davSession` — the Maestro session name bound to the ADE window.
    /// `run_dir_or_nil` is bundled so callers need only 1 RTT when the focused window has a session.
    pub fn focused_window_skill(&self) -> String {
        r#"let((cw sess) cw=hiGetCurrentWindow() sess=if(cw cw->davSession nil) list(if(cw hiGetWindowName(cw) nil) sess mapcar(lambda((w) hiGetWindowName(w)) hiGetWindowList()) maeGetSessions() if(sess let((s) s=asiGetSession(sess) if(s asiGetAnalogRunDir(s) nil)) nil)))"#.into()
    }

    /// Get simulation run directory for a maestro session via asiGetAnalogRunDir.
    /// Used when the caller provides a different session than the focused window's davSession.
    pub fn run_dir_skill(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(
            r#"let((sess) sess=asiGetSession("{session}") if(sess asiGetAnalogRunDir(sess) nil))"#
        )
    }

    /// Export results to CSV.
    pub fn export_results(&self, session: &str, file_path: &str) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        format!(
            r#"maeExportOutputView(?session "{session}" ?fileName "{file_path}" ?view "Detail")"#
        )
    }

    // =========================================================================
    // Result Reading Functions (IC23/IC25 compatible)
    // =========================================================================

    /// Open a history run for programmatic result access.
    pub fn open_results(&self, history: &str) -> String {
        let history = escape_skill_string(history);
        format!(r#"maeOpenResults(?history "{history}")"#)
    }

    /// Close the currently open results.
    pub fn close_results(&self) -> String {
        r#"maeCloseResults()"#.into()
    }

    /// List all test names that have results in the current history.
    pub fn get_result_tests(&self) -> String {
        r#"let((tests out sep) tests = maeGetResultTests() out = "[" sep = "" foreach(t tests out = strcat(out sep sprintf(nil "\"%s\"" t)) sep = ",") strcat(out "]"))"#.into()
    }

    /// List all output names available for a given test in the current history.
    pub fn get_result_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(r#"let((outs out sep) outs = maeGetResultOutputs(?testName "{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "\"%s\"" o)) sep = ",") strcat(out "]"))"#)
    }

    /// Get the value of a specific output for a specific test and corner.
    pub fn get_output_value(&self, name: &str, test_name: &str, corner: Option<&str>) -> String {
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);
        match corner {
            Some(c) => {
                let c = escape_skill_string(c);
                format!(r#"maeGetOutputValue("{name}" "{test_name}" ?cornerName "{c}")"#)
            }
            None => format!(r#"maeGetOutputValue("{name}" "{test_name}")"#),
        }
    }

    /// Get the spec pass/fail status for an output.
    pub fn get_spec_status(&self, name: &str, test_name: &str) -> String {
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);
        format!(r#"maeGetSpecStatus("{name}" "{test_name}")"#)
    }

    /// List available history runs for the current Maestro session.
    /// Returns JSON array of history names.
    pub fn get_history_list(&self) -> String {
        r#"let((base histories out sep) base = getDirFiles(strcat(asiGetResultsDir(asiGetCurrentSession()) "/..")) histories = remove("maestro" remove("exprOutputs.log" base)) out = "[" sep = "" foreach(h histories when(h && !index(h ".") out = strcat(out sep sprintf(nil "\"%s\"" h)) sep = ",")) strcat(out "]"))"#.into()
    }

    /// Get the Maestro session ID for the current session.
    pub fn get_current_session(&self) -> String {
        r#"let((sess out) sess = asiGetCurrentSession() out = if(sess then sess~>name else "nil"))"#.into()
    }
}

/// Wrap a SKILL expression that returns a list-of-strings into a JSON array string.
///
/// If `list_expr` returns nil (empty), the output is `"[]"`.
/// This ensures list-returning ops never produce SKILL nil — callers use r.ok() not r.skill_ok().
fn skill_strings_to_json(list_expr: &str) -> String {
    format!(
        r#"let((xs out sep) xs = {list_expr} out = "[" sep = "" foreach(x xs out = strcat(out sep sprintf(nil "\"%s\"" x)) sep = ",") strcat(out "]"))"#
    )
}

/// Convert a JSON object string to a SKILL association list.
///
/// Input: `{"start":"1","stop":"10G","dec":"20"}`
/// Output: `(("start" "1") ("stop" "10G") ("dec" "20"))`
///
/// Returns `Err` if the input is not valid JSON or not a JSON object.
pub(crate) fn json_to_skill_alist(json_str: &str) -> Result<String, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    let pairs: Vec<String> = obj
        .iter()
        .map(|(k, v)| {
            let binding = v.to_string();
            let val = v.as_str().unwrap_or(&binding);
            format!("(\"{k}\" \"{val}\")")
        })
        .collect();
    Ok(format!("({})", pairs.join(" ")))
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
        let s = ops().set_var("Vdd", "1.8");
        assert_eq!(s, r#"maeSetVar("Vdd" "1.8")"#);
    }

    #[test]
    fn run_simulation_includes_session() {
        let s = ops().run_simulation("sess1");
        assert!(s.contains("maeRunSimulation"), "{s}");
        assert!(s.contains("\"sess1\""), "{s}");
    }

    #[test]
    fn get_analyses_ic23_resolves_setup() {
        let s = ops().get_analyses("sess1", VirtuosoVersion::IC23);
        assert!(s.contains("maeGetSetup"), "IC23 must resolve setup: {s}");
        assert!(s.contains("maeGetEnabledAnalysis"), "{s}");
    }

    #[test]
    fn get_analyses_ic25_uses_ic23_path() {
        // IC25.1 ISR4 实测：maeGetSetup 仍返回 list，car() 有效
        // is_ic25() 返回 false，所以 IC25 版本走 IC23 路径
        let s = ops().get_analyses("sess1", VirtuosoVersion::IC25);
        assert!(s.contains("maeGetSetup"), "IC25 currently uses IC23 path: {s}");
        assert!(s.contains("maeGetEnabledAnalysis"), "{s}");
    }

    #[test]
    fn list_sessions_uses_helper() {
        let s = ops().list_sessions();
        assert!(s.contains("maeGetSessions()"), "{s}");
        assert!(s.contains("foreach"), "{s}");
        assert!(s.contains(r#"strcat(out "]")"#), "{s}");
    }

    #[test]
    fn get_result_tests_uses_helper() {
        let s = ops().get_result_tests();
        assert!(s.contains("maeGetResultTests()"), "{s}");
        assert!(s.contains("foreach"), "{s}");
    }

    #[test]
    fn get_history_list_uses_helper() {
        let s = ops().get_history_list();
        assert!(s.contains("asiGetResultsDir"), "{s}");
        assert!(s.contains("foreach"), "{s}");
    }

    #[test]
    fn set_analysis_ic23_positional() {
        let s = ops().set_analysis("sess1", "ac", None, VirtuosoVersion::IC23);
        assert!(s.contains("maeGetSetup"), "IC23 must resolve setup: {s}");
        assert!(s.contains("maeSetAnalysis"), "{s}");
        assert!(s.contains("\"ac\""), "{s}");
    }

    #[test]
    fn set_analysis_ic23_no_options() {
        let s = ops().set_analysis("sess1", "ac", None, VirtuosoVersion::IC23);
        assert!(
            !s.contains("?options"),
            "IC23 path must not inject options: {s}"
        );
    }

    #[test]
    fn set_analysis_ic25_uses_ic23_path() {
        // IC25.1 ISR4 实测：maeSetAnalysis 仍为 positional (setupName type)
        let s = ops().set_analysis("sess1", "ac", None, VirtuosoVersion::IC25);
        assert!(s.contains("maeGetSetup"), "IC25 currently uses IC23 path: {s}");
        assert!(s.contains("maeSetAnalysis"), "{s}");
    }

    #[test]
    fn add_output_includes_expr() {
        let s = ops().add_output("gain", "AC", "getData(\"vout\")", VirtuosoVersion::IC23);
        assert!(s.contains("maeAddOutput"), "{s}");
        assert!(s.contains("\"gain\""), "{s}");
        assert!(s.contains("\"AC\""), "{s}");
    }

    #[test]
    fn json_to_skill_alist_valid_input() {
        let input = r#"{"start":"1","stop":"10G"}"#;
        let out = json_to_skill_alist(input).unwrap();
        assert!(out.contains("(\"start\" \"1\")"), "{out}");
        assert!(out.contains("(\"stop\" \"10G\")"), "{out}");
    }

    #[test]
    fn json_to_skill_alist_invalid_json_returns_err() {
        assert!(json_to_skill_alist("not json").is_err());
    }

    #[test]
    fn json_to_skill_alist_non_object_returns_err() {
        assert!(json_to_skill_alist("[1,2,3]").is_err());
    }
}

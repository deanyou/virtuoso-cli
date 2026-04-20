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

    pub fn set_var(&self, name: &str, value: &str) -> String {
        let name = escape_skill_string(name);
        let value = escape_skill_string(value);
        format!(r#"maeSetVar("{name}" "{value}")"#)
    }

    pub fn get_var(&self, name: &str) -> String {
        let name = escape_skill_string(name);
        format!(r#"maeGetVar("{name}")"#)
    }

    pub fn list_vars(&self) -> String {
        r#"let((vars out sep) vars = asiGetDesignVarList(asiGetCurrentSession()) out = "[" sep = "" foreach(v vars out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" car(v) cadr(v))) sep = ",") strcat(out "]"))"#.into()
    }

    /// Get enabled analyses. Always returns a JSON string array; empty → `[]` (not an error).
    pub fn get_analyses(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        skill_strings_to_json(&format!(
            r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeGetEnabledAnalysis(setup))"#
        ))
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

    /// IC23.1: `maeGetTestOutputs` returns list-of-lists; elements accessed via car/cadr/caddr.
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(r#"let((outs out sep) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"test\":\"%s\",\"expr\":\"%s\"}}" car(o) cadr(o) if(caddr(o) then caddr(o) else ""))) sep = ",") strcat(out "]"))"#)
    }

    pub fn add_output(&self, output_name: &str, test_name: &str, expr: &str) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        let expr = escape_skill_string(expr);
        format!(r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#)
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

    /// Get focused ADE window name, all window names, and active sessions.
    /// Returns a SKILL list: (focused_window_name (all_names...) (sessions...))
    pub fn focused_window_skill(&self) -> String {
        r#"let((cw) cw=hiGetCurrentWindow() list(if(cw hiGetWindowName(cw) nil) mapcar(lambda((w) hiGetWindowName(w)) hiGetWindowList()) maeGetSessions()))"#.into()
    }

    /// Get simulation run directory for a maestro session via asiGetAnalogRunDir.
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

    pub fn open_results(&self, history: &str) -> String {
        let history = escape_skill_string(history);
        format!(r#"maeOpenResults(?history "{history}")"#)
    }

    pub fn close_results(&self) -> String {
        r#"maeCloseResults()"#.into()
    }

    /// List all test names that have results in the current history.
    pub fn get_result_tests(&self) -> String {
        skill_strings_to_json("maeGetResultTests()")
    }

    pub fn get_result_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        skill_strings_to_json(&format!(
            r#"maeGetResultOutputs(?testName "{test_name}")"#
        ))
    }

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

    pub fn get_spec_status(&self, name: &str, test_name: &str) -> String {
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);
        format!(r#"maeGetSpecStatus("{name}" "{test_name}")"#)
    }

    /// List available history runs for a Maestro session.
    /// Uses maeGetAllExplorerHistoryNames(sessionName) — IC23.1 documented API.
    /// Pass the Maestro session name from maeGetSessions(), not the Ocean session.
    pub fn get_history_list(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        skill_strings_to_json(&format!(r#"maeGetAllExplorerHistoryNames("{session}")"#))
    }

    pub fn get_current_session(&self) -> String {
        r#"let((sess) sess = asiGetCurrentSession() if(sess then sess~>name else nil))"#.into()
    }
}

/// Wrap a SKILL expression that returns a list-of-strings into a JSON array string.
///
/// If `list_expr` returns nil (empty), the output is `"[]"`.
/// This ensures list-returning ops never produce SKILL nil — callers use r.ok() not r.skill_ok().
fn skill_strings_to_json(list_expr: &str) -> String {
    format!(r#"let((xs out sep) xs = {list_expr} out = "[" sep = "" foreach(x xs out = strcat(out sep sprintf(nil "\"%s\"" x)) sep = ",") strcat(out "]"))"#)
}

/// Convert a JSON object string to a SKILL association list.
///
/// Input:  `{"start":"1","stop":"10G","dec":"20"}`
/// Output: `(("start" "1") ("stop" "10G") ("dec" "20"))`
///
/// Returns `Err` if the input is not valid JSON or not a JSON object.
pub(crate) fn json_to_skill_alist(json_str: &str) -> Result<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("invalid JSON: {e}"))?;
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
    fn list_sessions_uses_helper() {
        let s = ops().list_sessions();
        assert!(s.contains("maeGetSessions()"), "{s}");
        assert!(s.contains("foreach"), "{s}");
        assert!(s.contains(r#"strcat(out "]")"#), "{s}");
    }

    #[test]
    fn get_analyses_resolves_setup() {
        let s = ops().get_analyses("sess1");
        assert!(s.contains("maeGetSetup"), "must resolve setup: {s}");
        assert!(s.contains("maeGetEnabledAnalysis"), "{s}");
        assert!(s.contains("foreach"), "must produce JSON array: {s}");
    }

    #[test]
    fn get_result_tests_uses_helper() {
        let s = ops().get_result_tests();
        assert!(s.contains("maeGetResultTests()"), "{s}");
        assert!(s.contains("foreach"), "{s}");
    }

    #[test]
    fn get_history_list_uses_helper() {
        let s = ops().get_history_list("fnxSession0");
        assert!(s.contains("maeGetAllExplorerHistoryNames"), "{s}");
        assert!(s.contains("fnxSession0"), "{s}");
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
        assert!(!s.contains("?options"), "IC23 path must not inject options: {s}");
    }

    #[test]
    fn add_output_includes_expr() {
        let s = ops().add_output("gain", "AC", "getData(\"vout\")");
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

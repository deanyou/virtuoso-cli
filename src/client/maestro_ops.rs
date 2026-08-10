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

    /// Get enabled analyses — IC23/IC25 均用 positional (setupName)。
    ///
    /// 实测（IC25.1 ISR7）：`maeGetEnabledAnalysis(?session ...)` 报错，
    /// 必须先 `car(maeGetSetup(?session ...))` 取 setup 名，再 positional 传入。
    pub fn get_analyses(&self, session: &str, _version: VirtuosoVersion) -> String {
        let session = escape_skill_string(session);
        format!(
            r#"let((setup) setup = car(maeGetSetup(?session "{session}")) maeGetEnabledAnalysis(setup))"#
        )
    }

    /// Enable an analysis type — version-aware.
    ///
    /// IC23: `maeSetAnalysis(setupName analysisType)` — positional.
    /// IC25: `maeSetAnalysis(setupName analysisType ?session s ?enable t ?options (list ...))`.
    ///
    /// 实测 IC25（2026-08-06, IC25.1 ISR7）：
    /// - `?options (list (list "stop" "1e10"))` 成功写入 netlist
    /// - `?options` 中 `dec` 被静默丢弃，需通过 netlist sed 补全
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
            // IC25: maeSetAnalysis(sessionName type ?session s ?enable t ?options opts)
            //
            // IMPORTANT: use session name string directly as setup identifier — NOT car(maeGetSetup(...)).
            // maeGetSetup(?session ...) returns nil for fresh Maestro sessions that have no persistent
            // test yet.  maeSetAnalysis(sessionName ...) accepts the session name as the setup name
            // argument and creates the test implicitly.
            let (opts_binding, opts_arg) = match options_skill_alist {
                Some(alist) => {
                    let pairs: Vec<String> = parse_skill_pairs(alist);
                    let quoted: String = pairs
                        .iter()
                        .map(|p| skill_pair_to_quoted(p))
                        .collect::<Vec<_>>()
                        .join(" ");
                    (format!("opts = (list {})", quoted), " ?options opts".to_string())
                }
                None => (String::new(), String::new()),
            };
            format!(
                r#"let((opts) {opts_binding} maeSetAnalysis("{session}" "{analysis_type}" ?session "{session}" ?enable t{opts_arg}))"#
            )
        } else {
            // IC23: positional — setup name first; options not supported
            format!(
                r#"maeSetAnalysis("{session}" "{analysis_type}")"#
            )
        }
    }


    /// Run simulation asynchronously. Returns immediately.
    pub fn run_simulation(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeRunSimulation(?session "{session}")"#)
    }

    /// Run simulation with dec injection, all in one SKILL expression.
    ///
    /// For IC25 ISR4: maeSetAnalysis ?options doesn't write dec to the netlist.
    ///
    /// The entire SKILL expression (procedure definition + call) goes through
    /// `execute_skill_admin` which passes `skip_whitelist=true`, bypassing both
    /// `check_blocking_skill` and the whitelist so `system("sed")` is permitted.
    /// The procedure runs in Virtuoso's SKILL context where `system()` is available.
    ///
    /// Key findings (IC25 ISR7):
    /// - maeSetAnalysis: first positional arg must be the SETUP NAME (from maeGetSetup),
    ///   NOT the session name. Using session name → returns nil, doesn't enable analysis.
    /// - maeGetAnalogRunDir is NOT available in bridge daemon SKILL context → use
    ///   derived path via getWorkingDir() + maeGetSetup().
    /// - Netlist lives at:
    ///   getWorkingDir()/simulation/<cellview>/maestro/results/maestro/.tmpADEDir_user1/
    ///   <setupName>/<cellview>_schematic_spectre/netlist/input.scs
    pub fn run_with_dec(
        &self,
        session: &str,
        analysis_type: &str,
        options_skill_alist: Option<&str>,
        dec: u32,
    ) -> String {
        let session_esc = escape_skill_string(session);
        let at_esc = escape_skill_string(analysis_type);
        let dec_u32 = dec;
        let opts_inner = match options_skill_alist {
            Some(alist) => {
                let pairs: Vec<String> = parse_skill_pairs(alist);
                let quoted: String = pairs
                    .iter()
                    .map(|p| skill_pair_to_quoted(p))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("quote((\"dec\" \"0\")) {}", quoted)
            }
            None => r#"quote(("dec" "0"))"#.to_string(),
        };
        // The sed primary pattern matches " stop=" (space before stop) in the netlist.
        // If dec=0 was in the netlist it would match first, otherwise fallback matches.
        // After injection: " dec=N stop=" preserves spacing before stop=.
        format!(
            r#"progn(
procedure(vcliDecInject(s a o d)
let((setupName cellView netDir netPath cmd ok ds)
; Derive netlist path from getWorkingDir + maeGetSetup
; IC25 Maestro writes netlist to:
;   <wd>/simulation/<cellview>/maestro/results/maestro/.tmpADEDir_user1/<setupName>/<cellview>_schematic_spectre/netlist/input.scs
setupName=car(maeGetSetup(?session s))
cellView=substring(setupName 1 sub1(strlen(setupName)))
netDir=sprintf(nil "%s/simulation/%s/maestro/results/maestro/.tmpADEDir_user1/%s/%s_schematic_spectre/netlist" getWorkingDir() cellView setupName cellView)
netPath=strcat(netDir "/input.scs")
; Build dec="N" string for sed
ds=sprintf(nil "dec=%d" d)
; Set analysis with dec=0 (placeholder, sed will replace it)
maeSetAnalysis(setupName a ?session s ?enable t ?options o)
; Save → generates netlist
maeSaveSetup(?session s)
; Primary sed: replace " stop=" → " dec=N stop=" on the ac line
; Uses /0/.../0/ address to match only the first occurrence
cmd=sprintf(nil "sed -i '0,/ stop=/s/ stop=/ %s stop=/' %s %s" ds netPath netPath)
ok=not(system(cmd))
when(ok system(sprintf(nil "grep '^ac ' %s" netPath)))
maeRunSimulation(?session s)))
vcliDecInject("{}" "{}" (list {}) {}))"#,
            session_esc,
            at_esc,
            opts_inner,
            dec_u32
        )
    }

    /// Get test outputs — version-aware.
    ///
    /// IC23/IC25: maeGetTestOutputs(testName) — both use positional.
    /// IC25 additionally supports ?session keyword.
    #[allow(dead_code)]
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        format!(
            r#"let((outs out sep) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\",\"signalName\":\"%s\",\"expr\":\"%s\"}}" o~>name o~>outputType o~>signalName o~>expr)) sep = ",") strcat(out "]"))"#
        )
    }

    pub fn add_output(&self, output_name: &str, test_name: &str, expr: &str) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        let expr = escape_skill_string(expr);
        format!(r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#)
    }

    #[allow(dead_code)]
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

    /// Create a netlist for a specific corner.
    /// maeCreateNetlistForCorner(testName cornerName outputDir ?session s)
    pub fn create_netlist_for_corner(
        &self,
        test_name: &str,
        corner: &str,
        output_dir: &str,
        session: &str,
    ) -> String {
        let test_name = escape_skill_string(test_name);
        let corner = escape_skill_string(corner);
        let output_dir = escape_skill_string(output_dir);
        let session = escape_skill_string(session);
        format!(
            r#"maeCreateNetlistForCorner("{test_name}" "{corner}" "{output_dir}" ?session "{session}")"#
        )
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

    /// Export results to CSV via maeExportOutputView.
    pub fn export_results(
        &self,
        session: &str,
        file_path: &str,
        test_name: Option<&str>,
        history: Option<&str>,
    ) -> String {
        let session = escape_skill_string(session);
        let file_path = escape_skill_string(file_path);
        let test_name_part = match test_name {
            Some(t) => format!(r#" ?testName "{}""#, escape_skill_string(t)),
            None => String::new(),
        };
        let history_part = match history {
            Some(h) => format!(r#" ?historyName "{}""#, escape_skill_string(h)),
            None => String::new(),
        };
        format!(
            r#"maeExportOutputView(?session "{session}"{test_name_part}{history_part} ?view "Detail" ?fileName "{file_path}")"#
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
        format!(
            r#"let((outs out sep) outs = maeGetResultOutputs(?testName "{test_name}") out = "[" sep = "" foreach(o outs out = strcat(out sep sprintf(nil "\"%s\"" o)) sep = ",") strcat(out "]"))"#
        )
    }

    /// Get the value of a specific output for a specific test and corner.
    ///
    /// Note: This method does NOT call maeOpenResults first. You must call
    /// open_results(history) before using this method to ensure the results
    /// are accessible. Alternatively, use get_output_value_with_open() which
    /// combines both operations.
    ///
    /// Similar to virtuoso-bridge-lite's fix for issue #81: maeGetOutputValue
    /// should work directly without gating on maeExportOutputView return value.
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

    /// Get output value with results opened first.
    ///
    /// This is a convenience method that combines open_results and get_output_value.
    /// Use this when you need to read output values from a specific history run.
    ///
    /// Returns a SKILL expression that:
    /// 1. Opens the history results (ignores return value - virtuoso-bridge-lite #81 fix)
    /// 2. Gets the output value
    pub fn get_output_value_with_open(
        &self,
        history: &str,
        name: &str,
        test_name: &str,
        corner: Option<&str>,
    ) -> String {
        let history = escape_skill_string(history);
        let name = escape_skill_string(name);
        let test_name = escape_skill_string(test_name);

        // Build the get_output_value call
        let get_value = match corner {
            Some(c) => {
                let c = escape_skill_string(c);
                format!(r#"maeGetOutputValue("{name}" "{test_name}" ?cornerName "{c}")"#)
            }
            None => format!(r#"maeGetOutputValue("{name}" "{test_name}")"#),
        };

        // Combine: open results (ignore return), then get value
        // Note: We don't gate on maeOpenResults return value (virtuoso-bridge-lite #81 fix)
        format!(r#"(progn (maeOpenResults ?history "{history}") {get_value})"#)
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
    #[allow(dead_code)]
    pub fn get_current_session(&self) -> String {
        r#"let((sess out) sess = asiGetCurrentSession() out = if(sess then sess~>name else "nil"))"#
            .into()
    }

    // =========================================================================
    // Auto-Detection Helpers (similar to virtuoso-bridge-lite)
    // =========================================================================

    /// Get the current Maestro session info as a structured SKILL call.
    /// Returns: (session_name, lib, cell, view) or nil.
    ///
    /// Usage:
    ///   let skill = ops.maestro_session_info();
    ///   let result = client.execute_skill(&skill)?;
    pub fn maestro_session_info(&self) -> String {
        r#"let((sess info) sess = asiGetCurrentSession() info = if(sess list(sess~>name if(sess~>adeSession then sess~>adeSession~>libName else nil) if(sess~>adeSession then sess~>adeSession~>cellName else nil) if(sess~>adeSession then sess~>adeSession~>viewName else nil)) else nil))"#.into()
    }

    /// Check if a cell exists in a library.
    /// Returns "exists" if found, nil otherwise.
    pub fn cell_exists(&self, lib: &str, cell: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        format!(r#"when(ddGetObj("{lib}" "{cell}") "exists")"#)
    }

    /// List all libraries containing cells of a specific view type.
    /// Useful for auto-detecting which PDK libraries are available.
    pub fn libs_with_view(&self, view: &str) -> String {
        let view = escape_skill_string(view);
        format!(
            r#"let((libs out sep) libs = ddGetLibList() out = "[" sep = "" foreach(l libs when(member("{view}" l~>cells~>viewName) out = strcat(out sep sprintf(nil "\"%s\"" l~>name)) sep = ",")) strcat(out "]"))"#
        )
    }

    /// Get the simulation results directory for the current session.
    /// Returns nil if no session is active.
    pub fn results_dir(&self) -> String {
        r#"let((sess dir) sess = asiGetCurrentSession() dir = if(sess then asiGetResultsDir(sess) else nil) if(dir dir "nil"))"#.into()
    }

    /// Get all available corner/corner-set names from the Maestro setup.
    pub fn get_corners(&self) -> String {
        r#"let((corners out sep) corners = maeGetCorners() out = "[" sep = "" foreach(c corners out = strcat(out sep sprintf(nil "\"%s\"" c)) sep = ",") strcat(out "]"))"#.into()
    }

    /// Detect PVT corner from simulation results directory or cell name.
    /// Returns the corner string (e.g., "tt", "ss", "ff") if detectable.
    pub fn detect_corner_from_path(&self, path: &str) -> String {
        let path = escape_skill_string(path);
        format!(
            r#"let((name corner) name = "{path}" corner = cond(
                (rexMatchp("tt" name) "tt")
                (rexMatchp("ss" name) "ss")
                (rexMatchp("ff" name) "ff")
                (rexMatchp("snfp" name) "snfp")
                (rexMatchp("fnfp" name) "fnfp")
                (rexMatchp("fs" name) "fs")
                (rexMatchp("sf" name) "sf")
                (t nil)
            ) if(corner corner "nil"))"#
        )
    }

    /// Get simulation status for a session (running, completed, failed).
    pub fn get_sim_status(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(
            r#"let((sess status) sess = asiGetSession("{session}") status = if(sess sess~>status else "nil"))"#
        )
    }
}

/// Parse a SKILL alist string into individual pair strings like `["(list \"k1\" \"v1\")", "(list \"k2\" \"v2\")"]`.
///
/// Handles both formats:
///   - With `list` keyword:  `(list (list "k1" "v1") (list "k2" "v2"))`
///   - Without `list` keyword (from `json_to_skill_alist`): `(("k1" "v1") ("k2" "v2"))`
fn parse_skill_pairs(alist: &str) -> Vec<String> {
    let alist = alist.trim();
    if alist.len() < 2 {
        return vec![alist.to_string()];
    }
    // Format A: "(list (list ...) (list ...))"
    if alist.starts_with("(list (") {
        let inner = &alist[1..alist.len() - 1]; // strip first '(' and last ')'
        return parse_inner_pairs(inner);
    }
    // Format B: "((\"k1\" \"v1\") (\"k2\" \"v2\"))" — bare pairs, no outer list keyword
    if alist.starts_with("((") {
        let inner = &alist[1..alist.len() - 1]; // strip outer '(' and ')'
        return parse_inner_pairs(inner);
    }
    vec![alist.to_string()]
}

/// Parse the inner content between the outer parentheses of an alist,
/// extracting each top-level parenthetical group as a separate pair.
fn parse_inner_pairs(inner: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => {
                depth += 1;
                if depth == 1 {
                    start = i;
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    results.push(inner[start..=i].trim().to_string());
                }
            }
            _ => {}
        }
        if depth < 0 {
            break;
        }
    }
    if results.is_empty() {
        vec![inner.trim().to_string()]
    } else {
        results
    }
}

/// Convert a parsed pair to a SKILL quoted list: `quote(("key" "val"))`.
///
/// maeSetAnalysis ?options expects a list of such quoted pairs.
fn skill_pair_to_quoted(pair: &str) -> String {
    let trimmed = pair.trim();
    if trimmed.starts_with("(list ") {
        // Has list keyword: strip "(list " and trailing ")"
        let without_prefix = &trimmed["(list ".len()..];
        let stripped = without_prefix.strip_suffix(')').unwrap_or(without_prefix).trim();
        format!("quote(({stripped}))")
    } else {
        // Bare pair: `("key" "val")`
        format!("quote({trimmed})")
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
    fn get_analyses_ic25_uses_setup_name() {
        // 实测 IC25.1 ISR7：maeGetEnabledAnalysis(?session ...) 报错
        // IC23/IC25 均需 car(maeGetSetup()) 取 setup 名，positional 传入
        let s = ops().get_analyses("sess1", VirtuosoVersion::IC25);
        assert!(
            s.contains("maeGetSetup"),
            "Both IC23 and IC25 need maeGetSetup: {s}"
        );
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
    fn export_results_minimal() {
        let s = ops().export_results("sess1", "/tmp/out.csv", None, None);
        assert!(s.contains("maeExportOutputView"), "{s}");
        assert!(s.contains(r#"?session "sess1""#), "{s}");
        assert!(s.contains(r#"?fileName "/tmp/out.csv""#), "{s}");
        assert!(s.contains(r#"?view "Detail""#), "{s}");
        assert!(!s.contains("?testName"), "should be absent when None: {s}");
        assert!(
            !s.contains("?historyName"),
            "should be absent when None: {s}"
        );
    }

    #[test]
    fn export_results_with_all_params() {
        let s = ops().export_results("sess1", "/tmp/out.csv", Some("AC"), Some("ExplorerRun.0"));
        assert!(s.contains(r#"?testName "AC""#), "{s}");
        assert!(s.contains(r#"?historyName "ExplorerRun.0""#), "{s}");
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
    fn set_analysis_ic25_includes_keywords() {
        // IC25 uses ?session and ?enable t keywords (unlike IC23 positional-only)
        let s = ops().set_analysis("sess1", "ac", None, VirtuosoVersion::IC25);
        assert!(s.contains("?session"), "IC25 must include ?session keyword: {s}");
        assert!(s.contains("?enable t"), "IC25 must include ?enable t: {s}");
        assert!(s.contains("maeGetSetup"), "IC25 needs setup name: {s}");
        assert!(!s.contains("?options"), "IC25 without options must not inject ?options: {s}");
    }

    #[test]
    fn set_analysis_ic25_with_options() {
        // IC25: maeSetAnalysis with ?options using quote to protect inner pairs.
        // json_to_skill_alist returns "((\"stop\" \"1e10\") (\"start\" \"1\"))" — bare pairs.
        // skill_pair_to_quoted wraps each as quote(("stop" "1e10")).
        // Generated: let((setup opts) setup=car(...) opts=(list quote(...)) maeSetAnalysis(...))
        let s = ops().set_analysis(
            "sess1", "ac", Some(r#"(("stop" "1e10") ("start" "1"))"#), VirtuosoVersion::IC25,
        );
        assert!(s.contains(r#"quote(("stop" "1e10"))"#), "IC25 options must use quote: {s}");
        assert!(s.contains("?options opts"), "Must pass options via ?options keyword: {s}");
        assert!(s.contains("let((setup opts)"), "Must use flat let with two bindings: {s}");
        assert!(s.contains("opts = (list"), "opts must be bound via = form: {s}");
        assert!(s.contains("maeSetAnalysis(setup"), "Must call maeSetAnalysis directly: {s}");
        assert!(!s.contains("apply("), "No apply() needed for direct maeSetAnalysis: {s}");
    }

    #[test]
    fn add_output_includes_expr() {
        let s = ops().add_output("gain", "AC", "getData(\"vout\")");
        assert!(s.contains("maeAddOutput"), "{s}");
        assert!(s.contains("\"gain\""), "{s}");
        assert!(s.contains("\"AC\""), "{s}");
    }

    #[test]
    fn create_netlist_for_corner_format() {
        let s = ops().create_netlist_for_corner("AC", "tt", "/tmp/out", "fnxSession4");
        assert_eq!(
            s,
            r#"maeCreateNetlistForCorner("AC" "tt" "/tmp/out" ?session "fnxSession4")"#
        );
    }

    #[test]
    fn create_netlist_for_corner_escapes_quote_in_test() {
        // Quotes inside the test name must be SKILL-escaped (`"` → `\"`).
        let s = ops().create_netlist_for_corner(r#"te"st"#, "tt", "/tmp/out", "fnxSession4");
        assert!(s.contains(r#""te\"st""#), "{s}");
        // Confirm command is still single-line and well-formed.
        assert!(s.starts_with("maeCreateNetlistForCorner("), "{s}");
        assert!(s.ends_with(")"), "{s}");
    }

    #[test]
    fn create_netlist_for_corner_escapes_backslash_in_corner() {
        // Backslash inside the corner name must be doubled (`\` → `\\`).
        let s = ops().create_netlist_for_corner("AC", r#"a\b"#, "/tmp/out", "fnxSession4");
        assert!(s.contains(r#""a\\b""#), "{s}");
    }

    #[test]
    fn create_netlist_for_corner_escapes_quote_in_output_dir() {
        // Quote and space inside the output dir must be SKILL-escaped.
        let s = ops().create_netlist_for_corner("AC", "tt", r#"/tmp/out "x""#, "fnxSession4");
        assert!(s.contains(r#""/tmp/out \"x\"""#), "{s}");
    }

    #[test]
    fn create_netlist_for_corner_escapes_quote_in_session() {
        let s = ops().create_netlist_for_corner("AC", "tt", "/tmp/out", r#"fnx"4"#);
        assert!(s.contains(r#"?session "fnx\"4""#), "{s}");
    }

    #[test]
    fn create_netlist_for_corner_escapes_all_four_args_independently() {
        // All four SKILL string parameters use the same escape function,
        // so each one must round-trip its special characters independently.
        let s = ops().create_netlist_for_corner(
            r#"te"st"#,
            r#"co\rner"#,
            r#"/tmp/out "x""#,
            r#"fnx"4"#,
        );
        assert!(s.contains(r#""te\"st""#), "{s}");
        assert!(s.contains(r#""co\\rner""#), "{s}");
        assert!(s.contains(r#""/tmp/out \"x\"""#), "{s}");
        assert!(s.contains(r#""fnx\"4""#), "{s}");
        // Exactly one occurrence of the opening maeCreateNetlistForCorner( and one closing ).
        assert_eq!(s.matches("maeCreateNetlistForCorner(").count(), 1);
        assert!(s.trim_end().ends_with(')'));
        assert_eq!(s.matches("?session").count(), 1);
    }

    #[test]
    fn create_netlist_for_corner_includes_session_keyword() {
        let s = ops().create_netlist_for_corner("AC", "tt", "/tmp/out", "fnxSession0");
        // Position of the ?session keyword must come last in the builder.
        let session_pos = s.find("?session").expect("?session keyword");
        let paren_end = s.rfind(')').expect("closing paren");
        assert!(
            session_pos < paren_end,
            "session keyword before closing paren: {s}"
        );
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

    #[test]
    fn get_output_value_without_corner() {
        let s = ops().get_output_value("gain", "AC", None);
        assert!(s.contains("maeGetOutputValue"), "{s}");
        assert!(s.contains("\"gain\""), "{s}");
        assert!(s.contains("\"AC\""), "{s}");
        assert!(
            !s.contains("?cornerName"),
            "should not have cornerName when None: {s}"
        );
    }

    #[test]
    fn get_output_value_with_corner() {
        let s = ops().get_output_value("gain", "AC", Some("tt"));
        assert!(s.contains("maeGetOutputValue"), "{s}");
        assert!(s.contains("?cornerName"), "should have cornerName: {s}");
        assert!(s.contains("\"tt\""), "{s}");
    }

    #[test]
    fn get_spec_status() {
        let s = ops().get_spec_status("gain", "AC");
        assert!(s.contains("maeGetSpecStatus"), "{s}");
        assert!(s.contains("\"gain\""), "{s}");
        assert!(s.contains("\"AC\""), "{s}");
    }

    #[test]
    fn get_current_session() {
        let s = ops().get_current_session();
        assert!(s.contains("asiGetCurrentSession"), "{s}");
        assert!(s.contains("sess~>name"), "{s}");
    }

    #[test]
    fn get_result_outputs() {
        let s = ops().get_result_outputs("AC");
        assert!(s.contains("maeGetResultOutputs"), "{s}");
        assert!(s.contains("\"AC\""), "{s}");
        assert!(s.contains("foreach"), "{s}");
    }

    #[test]
    fn set_design() {
        let s = ops().set_design("sess1", "myLib", "myCell", "schematic");
        assert!(s.contains("maeSetDesign"), "{s}");
        assert!(s.contains("?session"), "{s}");
        assert!(s.contains("?libName"), "{s}");
        assert!(s.contains("?cellName"), "{s}");
        assert!(s.contains("?viewName"), "{s}");
    }

    #[test]
    fn save_setup() {
        let s = ops().save_setup("sess1");
        assert!(s.contains("maeSaveSetup"), "{s}");
        assert!(s.contains("?session"), "{s}");
    }

    #[test]
    fn cell_exists() {
        let s = ops().cell_exists("myLib", "myCell");
        assert!(s.contains("ddGetObj"), "{s}");
        assert!(s.contains("\"myLib\""), "{s}");
        assert!(s.contains("\"myCell\""), "{s}");
        assert!(s.contains("when"), "{s}");
    }

    #[test]
    fn results_dir() {
        let s = ops().results_dir();
        assert!(s.contains("asiGetResultsDir"), "{s}");
    }

    #[test]
    fn detect_corner_from_path() {
        let s = ops().detect_corner_from_path("/path/to/tt_netlist");
        assert!(s.contains("rexMatchp"), "{s}");
        assert!(s.contains("\"tt\""), "{s}");
    }

    #[test]
    fn get_sim_status() {
        let s = ops().get_sim_status("sess1");
        assert!(s.contains("asiGetSession"), "{s}");
        assert!(s.contains("~>status"), "{s}");
    }
}

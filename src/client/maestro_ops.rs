use crate::client::bridge::escape_skill_string;
use crate::version::VirtuosoVersion;

/// Parameters for `MaestroOps::export_waveform`.
///
/// Bundles the seven caller-supplied strings/integers so the
/// constructor signature stays inside clippy's argument-count
/// budget (see `LayoutOps::xstream_out` for the same pattern).
pub struct ExportWaveformRequest<'a> {
    pub session: &'a str,
    pub expression: &'a str,
    pub analysis: &'a str,
    /// One of "scientific", "engineering", "automatic".
    pub number_notation: &'a str,
    /// `1..=16`.
    pub precision: u8,
    /// `>= 4`.
    pub width: u8,
    pub output_path: &'a str,
}

pub struct MaestroOps;

/// SKILL that binds `tn` to the test an Assembler call must act on.
///
/// Expects `tests` and `tn` to be declared in the enclosing `let`.
///
/// `maeGetSetup(?session s)` returns the session's **test names** — that is its
/// documented default (`?typeName "tests"`, IC23.1 ADE SKILL Reference), and
/// `maeSetAnalysis`/`maeGetEnabledAnalysis` take a test name as their first
/// positional argument. Taking `car()` of that list is a guess that happens to
/// be right while a session holds one test. So:
///
/// - `test` given → use it, but check membership first, because
///   `maeSetAnalysis` on an unknown test just returns `nil` with no hint of
///   which name it did not recognise.
/// - `test` omitted and the session holds exactly one → use that one.
/// - `test` omitted and the session holds several → **error naming them**,
///   rather than silently configuring whichever test happens to be first.
fn test_binding(session: &str, test: Option<&str>, caller: &str) -> String {
    let session = escape_skill_string(session);
    let caller = escape_skill_string(caller);
    let resolve = match test {
        Some(t) => {
            let t = escape_skill_string(t);
            format!(
                r#"tn = "{t}" unless(member(tn tests) error("{caller}: session \"{session}\" has no test named \"{t}\" — it has %L" tests))"#
            )
        }
        None => format!(
            r#"tn = cond((null(tests) error("{caller}: session \"{session}\" has no tests")) (cdr(tests) error("{caller}: session \"{session}\" has %d tests %L — pass \"test\" to say which one" length(tests) tests)) (t car(tests)))"#
        ),
    };
    format!(r#"tests = maeGetSetup(?session "{session}") {resolve}"#)
}

impl MaestroOps {
    /// Opens a maestro setup, returning a session handle like `"fnxSession4"`.
    ///
    /// `mode` is the difference between a session a human can work around and
    /// one that locks them out, so it is passed explicitly rather than left to
    /// `maeOpenSetup`'s own default:
    ///
    /// - `"r"` — read mode. **Takes no edit lock**: no `maestro.sdb.cdslck`
    ///   appears next to the setup, so the cellview stays openable for edit in
    ///   the GUI. The setup is still fully readable — `maeGetSetup` and the
    ///   rest of the query API work normally.
    /// - `"a"` — append mode, and `maeOpenSetup`'s default. Editable, and
    ///   **takes the edit lock**. A SKILL-opened session has no window (Cadence
    ///   documents this as the "non-GUI mode" and ships no API to attach one),
    ///   so a human then finds the cell unopenable with nothing anywhere to
    ///   click — EXPLORER-1642. `maeCloseSession` is the only way out.
    ///
    /// So callers open `"r"` and use [`Self::set_session_mode`] to hold `"a"`
    /// across the writes alone. Measured on IC23.1 2026-09-09 by watching the
    /// lock file appear and disappear.
    ///
    /// One asymmetry to pass on to the user: `maeOpenSetup` *creates* the
    /// cellview when it does not exist, and per Cadence "you cannot create a
    /// new view in read mode". Opening a setup that does not exist yet
    /// therefore needs `"a"`, and returns nil in `"r"`.
    pub fn open_session(&self, lib: &str, cell: &str, view: &str, mode: &str) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let mode = escape_skill_string(mode);
        format!(r#"maeOpenSetup("{lib}" "{cell}" "{view}" ?mode "{mode}")"#)
    }

    /// Switches an already-open session between editable and read-only.
    ///
    /// This is what makes a read-only default cost nothing: `maeMakeEditable`
    /// takes the edit lock and `maeMakeReadonly` hands it back, both in place
    /// with no reopen, so the window during which a human is locked out shrinks
    /// to the writes themselves. Verified in both directions against the lock
    /// file on IC23.1 2026-09-09.
    pub fn set_session_mode(&self, session: &str, editable: bool) -> String {
        let session = escape_skill_string(session);
        if editable {
            format!(r#"maeMakeEditable(?session "{session}")"#)
        } else {
            format!(r#"maeMakeReadonly(?session "{session}")"#)
        }
    }

    /// Force-closes the session, cancels any in-flight simulation.
    ///
    /// `maeCloseSession` takes the session as the `?session` keyword, not
    /// positionally — IC23.1 rejects the positional form with
    /// "extra arguments or keyword missing".
    pub fn close_session(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(r#"maeCloseSession(?session "{session}" ?forceClose t)"#)
    }

    pub fn list_sessions(&self) -> String {
        skill_strings_to_json("maeGetSessions()")
    }

    /// List the test names in a Maestro session.
    ///
    /// `maeGetSetup(?session s)` called *without* `?typeName` returns the
    /// test-name list, e.g. `("tb_cmp_SA")`. Every output-related API
    /// (`get_outputs`, `add_output`, `get_spec_status`, `export`,
    /// `create_corner_netlist`) is keyed by that name, so without this the
    /// caller has to guess it.
    pub fn list_tests(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        skill_strings_to_json(&format!(r#"maeGetSetup(?session "{session}")"#))
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

    /// List all Assembler-global design variables. Returns JSON via sprintf.
    ///
    /// Scope note: `set_var`/`get_var` write and read the *Assembler-global*
    /// scope (`maeSetVar`/`maeGetVar`), which is the scope that reaches the
    /// netlist. This method used to read `asiGetDesignVarList`, the *per-test /
    /// Explorer* scope — a different table that does not mirror the global one,
    /// so variables written over RPC always read back as absent.
    ///
    /// IC23.1 has no `mae*` enumerator (`maeGetVarList` / `maeGetVars` /
    /// `maeGetAllVars` / `maeGetVarNames` / `maeListVars` /
    /// `maeGetDesignVarList` / `maeGetVarValue` are all unbound). The names come
    /// from `maeGetSetup(?session s ?typeName "variables")` — note the spelling:
    /// `"vars"` and `"var"` both return nil. Values then come from
    /// `maeGetVar(name)` one at a time.
    pub fn list_vars(&self) -> String {
        r#"let((s names out sep raw val) s = car(maeGetSessions()) names = maeGetSetup(?session s ?typeName "variables") out = "[" sep = "" foreach(n names raw = maeGetVar(n) val = if(stringp(raw) then raw else if(raw then sprintf(nil "%L" raw) else "")) val = buildString(parseString(val "\"") "\\\"") out = strcat(out sep sprintf(nil "{\"name\":\"%s\",\"value\":\"%s\"}" n val)) sep = ",") strcat(out "]"))"#.into()
    }

    /// Delete an Assembler-global design variable.
    /// Mirrors `maeSetVar(name value)` — positional, no session argument.
    pub fn delete_var(&self, name: &str) -> String {
        let name = escape_skill_string(name);
        format!(r#"maeDeleteVar("{name}")"#)
    }

    /// Delete an output from a test's setup.
    /// Mirrors `maeAddOutput(name test ?expr ...)` — positional name + test.
    pub fn delete_output(&self, output_name: &str, test_name: &str) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        format!(r#"maeDeleteOutput("{output_name}" "{test_name}")"#)
    }

    /// Disable/remove an analysis from the current test.
    ///
    /// There is no `maeDeleteAnalysis` on IC23.1 (probed unbound, as is
    /// `maeRemoveAnalysis`); only the ADE-level `asiDeleteAnalysis` exists, so
    /// this one call is session-scoped rather than Assembler-scoped.
    ///
    /// Two IC23.1 quirks force the shape of this call:
    /// 1. The analysis name must be a **symbol**, not a string — passing `"ac"`
    ///    fails in `asiiDeleteObject` with "argument #1 should be a symbol".
    /// 2. Even on success, `asiDeleteAnalysis` throws a trailing
    ///    `*Error* eraseObject: no applicable method for the class list()`.
    ///    The analysis really is gone, so a plain error check would report a
    ///    successful delete as a failure. Hence: swallow the throw with
    ///    `errset`, then decide the outcome by *re-reading the enabled list*.
    ///
    /// Returns the string `"t"` when the analysis is absent afterwards.
    pub fn delete_analysis(&self, analysis: &str) -> String {
        let analysis = escape_skill_string(analysis);
        format!(
            r#"let((r lst) errset(asiDeleteAnalysis(asiGetCurrentSession() stringToSymbol("{analysis}"))) r = errset(maeGetEnabledAnalysis(car(maeGetSetup()))) lst = if(r car(r) nil) if(exists(a lst equal(a "{analysis}")) "nil" "t"))"#
        )
    }

    /// Get enabled analyses — IC23/IC25 均用 positional (testName)。
    ///
    /// 实测（IC25.1 ISR7）：`maeGetEnabledAnalysis(?session ...)` 报错，
    /// 必须 positional 传入测试名。
    pub fn get_analyses(
        &self,
        session: &str,
        test: Option<&str>,
        _version: VirtuosoVersion,
    ) -> String {
        let bind = test_binding(session, test, "maestro.get_analyses");
        format!(r#"let((tests tn) {bind} maeGetEnabledAnalysis(tn))"#)
    }

    /// Enable an analysis type — version-aware.
    ///
    /// `maeSetAnalysis`'s first positional argument is **`t_testName`**
    /// (IC23.1 ADE SKILL Reference), and `maeGetSetup(?session s)` returns the
    /// session's **test names**. The previous version passed
    /// `car(maeGetSetup(...))` and called it a "setup name": correct only while
    /// a session holds exactly one test, and silently configuring the *first*
    /// test the moment it holds two. Measured 2026-09-09 — `fnxSession14` grew
    /// a second test (`amp_buf_tb`) and every subsequent `set_analysis`
    /// would still have landed on `amp_tb`, returning `ok` each time. So the
    /// test is now either named explicitly or resolved from a session that has
    /// exactly one; ambiguity is an error, not a guess. See [`test_binding`].
    ///
    /// IC23: `maeSetAnalysis(testName analysisType ?enable t ?options ...)`.
    /// IC25: same, plus `?session`.
    ///
    /// 实测 IC25（2026-08-06, IC25.1 ISR7）：
    /// - `?options (list (list "stop" "1e10"))` 成功写入 netlist
    /// - `?options` 中 `dec` 被静默丢弃，需通过 netlist sed 补全
    pub fn set_analysis(
        &self,
        session: &str,
        analysis_type: &str,
        options_skill_alist: Option<&str>,
        test: Option<&str>,
        version: VirtuosoVersion,
    ) -> String {
        let bind = test_binding(session, test, "maestro.set_analysis");
        let session = escape_skill_string(session);
        let analysis_type = escape_skill_string(analysis_type);
        let (opts_binding, opts_arg) = match options_skill_alist {
            Some(alist) => {
                let quoted: String = parse_skill_pairs(alist)
                    .iter()
                    .map(|p| skill_pair_to_quoted(p))
                    .collect::<Vec<_>>()
                    .join(" ");
                (
                    format!("opts = (list {quoted})"),
                    " ?options opts".to_string(),
                )
            }
            None => (String::new(), String::new()),
        };
        // `?session` is only accepted on IC25; on IC23 the call is scoped by
        // the current session, which `test_binding` has already validated
        // against.
        let session_arg = if version.is_ic25() {
            format!(r#" ?session "{session}""#)
        } else {
            String::new()
        };
        format!(
            r#"let((tests tn opts) {bind} {opts_binding} maeSetAnalysis(tn "{analysis_type}"{session_arg} ?enable t{opts_arg}))"#
        )
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
procedure(vcliShellQuote(value)
; Return a POSIX single-quoted shell word, escaping embedded apostrophes.
; Apostrophe in input becomes '"'"' (close sq, add dq-sq-dq, reopen sq).
let((out i c len)
out = "'"
len = strlen(value)
i = 0
while(i < len
c = substring(value i 1)
if(c == "'"
out = strcat(out "'\"'\"'")
out = strcat(out c))
i = add1(i))
strcat(out "'")))
procedure(vcliDecInject(s a o d)
let((setupName cellView netDir netPath netPathQuoted cmd ok ds)
; Derive netlist path from getWorkingDir + maeGetSetup
; IC25 Maestro writes netlist to:
;   <wd>/simulation/<cellview>/maestro/results/maestro/.tmpADEDir_user1/<setupName>/<cellview>_schematic_spectre/netlist/input.scs
setupName=car(maeGetSetup(?session s))
cellView=substring(setupName 1 sub1(strlen(setupName)))
netDir=sprintf(nil "%s/simulation/%s/maestro/results/maestro/.tmpADEDir_user1/%s/%s_schematic_spectre/netlist" getWorkingDir() cellView setupName cellView)
netPath=strcat(netDir "/input.scs")
netPathQuoted=vcliShellQuote(netPath)
; Build dec="N" string for sed
ds=sprintf(nil "dec=%d" d)
; Set analysis with dec=0 (placeholder, sed will replace it)
maeSetAnalysis(setupName a ?session s ?enable t ?options o)
; Save → generates netlist
maeSaveSetup(?session s)
; Primary sed: replace " stop=" → " dec=N stop=" on the ac line
; Uses /0/.../0/ address to match only the first occurrence
cmd=sprintf(nil "sed -i '0,/ stop=/s/ stop=/ %s stop=/' %s %s" ds netPathQuoted netPathQuoted)
ok=not(system(cmd))
when(ok system(sprintf(nil "grep '^ac ' %s" netPathQuoted)))
maeRunSimulation(?session s)))
vcliDecInject("{}" "{}" (list {}) {}))"#,
            session_esc, at_esc, opts_inner, dec_u32
        )
    }

    /// Get test outputs — version-aware.
    ///
    /// IC23/IC25: maeGetTestOutputs(testName) — both use positional.
    /// IC25 additionally supports ?session keyword.
    #[allow(dead_code)]
    /// List a test's outputs as JSON.
    ///
    /// Three things the naive `sprintf(... o~>name o~>outputType ...)` got
    /// wrong on IC23.1:
    /// - outputs created by `maeAddOutput(... ?expr)` have a nil `outputType`,
    ///   and one nil aborts the whole `sprintf`
    ///   ("format spec. incompatible with data, argument #2 is nil");
    /// - the expression lives on `~>expression`, not `~>expr`, and it is a
    ///   SKILL form rather than a string, so `%s` cannot print it;
    /// - expressions contain `"` (`VDC("/vout")`), which has to be escaped or
    ///   the emitted JSON does not parse.
    pub fn get_outputs(&self, test_name: &str) -> String {
        let test_name = escape_skill_string(test_name);
        // buildString(parseString(x "\"") "\\\"") — stateless `"` → `\"`.
        let esc = |var: &str| format!(r#"buildString(parseString({var} "\"") "\\\"")"#);
        let str_field = |field: &str| {
            format!(r#"if(stringp(o~>{field}) then o~>{field} else "")"#)
        };
        let (name, otype, signal) = (
            str_field("name"),
            str_field("outputType"),
            str_field("signalName"),
        );
        // `expr` first (IC25 string form), then `expression` (IC23 SKILL form,
        // printed with %L since it is a list, not a string).
        let expr = r#"if(stringp(o~>expr) then o~>expr else if(o~>expression then sprintf(nil "%L" o~>expression) else ""))"#;
        let (e_name, e_type, e_signal, e_expr) =
            (esc("nm"), esc("ty"), esc("sg"), esc("ex"));
        format!(
            r#"let((outs out sep nm ty sg ex) outs = maeGetTestOutputs("{test_name}") out = "[" sep = "" foreach(o outs nm = {name} ty = {otype} sg = {signal} ex = {expr} out = strcat(out sep sprintf(nil "{{\"name\":\"%s\",\"type\":\"%s\",\"signalName\":\"%s\",\"expr\":\"%s\"}}" {e_name} {e_type} {e_signal} {e_expr})) sep = ",") strcat(out "]"))"#
        )
    }

    pub fn add_output(&self, output_name: &str, test_name: &str, expr: &str) -> String {
        let output_name = escape_skill_string(output_name);
        let test_name = escape_skill_string(test_name);
        let expr = escape_skill_string(expr);
        format!(r#"maeAddOutput("{output_name}" "{test_name}" ?expr "{expr}")"#)
    }

    /// Points a test (or every test in the session) at a design cellview.
    ///
    /// Uses `maeSetDesignForTest`, whose IC23.1 signature is
    /// `(t_lib t_cell t_view [?test t] [?session s])` — the library/cell/view
    /// are **positional**, and omitting `?test` sets the design for all tests.
    ///
    /// The previous form called `maeSetDesign` with `?libName`/`?cellName`/
    /// `?viewName` keywords and no test name at all. IC23.1 has no such
    /// keywords, and the real `maeSetDesign` takes the test name as its first
    /// positional argument, so that call could only ever return nil — which is
    /// why it sat behind `#[allow(dead_code)]` instead of being wired up.
    /// Signature read from
    /// `doc/maeSKILLref/maestroSKILL_re_maeSetDesignForTest.html` 2026-09-09.
    pub fn set_design(
        &self,
        session: &str,
        lib: &str,
        cell: &str,
        view: &str,
        test: Option<&str>,
    ) -> String {
        let session = escape_skill_string(session);
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let test_arg = match test {
            Some(t) => format!(" ?test \"{}\"", escape_skill_string(t)),
            None => String::new(),
        };
        format!(
            r#"maeSetDesignForTest("{lib}" "{cell}" "{view}"{test_arg} ?session "{session}")"#
        )
    }

    /// Creates a test in an open Maestro session.
    ///
    /// `maeCreateTest(t_testName [?sourceTest t] [?lib t] [?cell t] [?view t]
    /// [?simulator t] [?session t]) => t / nil`. Giving lib/cell/view sets the
    /// test's design in the same call, so no separate `set_design` is needed
    /// for a fresh test.
    ///
    /// The session must be editable — a session opened in the default
    /// `?mode "r"` accepts the in-memory change but `maeSaveSetup` will refuse
    /// to persist it (see `save_setup`).
    pub fn create_test(
        &self,
        session: &str,
        test: &str,
        lib: &str,
        cell: &str,
        view: &str,
        simulator: &str,
    ) -> String {
        let session = escape_skill_string(session);
        let test = escape_skill_string(test);
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let simulator = escape_skill_string(simulator);
        format!(
            r#"maeCreateTest("{test}" ?lib "{lib}" ?cell "{cell}" ?view "{view}" ?simulator "{simulator}" ?session "{session}")"#
        )
    }

    /// Saves the setup to disk — refusing up front if the session cannot write.
    ///
    /// `maeSaveSetup` on a read-only session returns non-nil and writes
    /// **nothing**. Measured on IC23.1 2026-09-09: with `fnxSession12` open in
    /// `?mode "r"`, `maeSetVar` accepted `RO_PROBE=1` (in-memory writes are not
    /// blocked either), `maeSaveSetup` returned success, and `maestro.sdb` kept
    /// its three-hour-old mtime with `<vars></vars>` still empty. So the return
    /// value proves nothing and the caller walks away believing the edit
    /// persisted — the same failure shape as `dbClose` reporting success while
    /// keeping its lock.
    ///
    /// `axlIsSessionReadOnly` is the oracle the return value isn't, so it is
    /// checked first and the save is refused with an error that names the fix.
    /// It is probed rather than assumed: on a Virtuoso that lacks it the save
    /// proceeds unguarded, which is no worse than before.
    pub fn save_setup(&self, session: &str) -> String {
        let session = escape_skill_string(session);
        format!(
            "let((s) s = \"{session}\" \
             when(getd('axlIsSessionReadOnly) && axlIsSessionReadOnly(s) \
               error(\"maestro.save: session %s is read-only. maeSaveSetup would report \
             success and write nothing. Switch it with maestro.set_session_mode mode=a, \
             save, then switch back to mode=r.\" s)) \
             maeSaveSetup(?session s))"
        )
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

    /// Export a waveform (or expression) to a text file via `ocnPrint`.
    ///
    /// # ⚠️ DEAD BUILDER — the SKILL it emits cannot run on IC23.1
    ///
    /// `axlGetWaveform` and `axlWaveformToList` **do not exist** on IC23.1:
    /// absent from all 41 SKILL Finder databases, and `getd` returns nil for
    /// both. Only the innermost `ocnPrint` is real. Executing this string dies
    /// on its first form.
    ///
    /// It is harmless today only because **no RPC method reaches it** —
    /// `maestro.export` routes to [`MaestroOps::export_results`]
    /// (`maeExportOutputView`, documented and bound). The sole callers are this
    /// module's own tests.
    ///
    /// Note what those tests assert: `contains("ocnPrint")`,
    /// `contains("scientific")`, `contains("/tmp/wave.txt")` — every one of
    /// them checks the one function in the string that actually exists, and
    /// none checks the two that don't. Six green tests over an unrunnable
    /// expression. A string builder's unit tests can only prove what the string
    /// looks like, never that it runs.
    ///
    /// **Do not wire this to an RPC method as-is.** The documented route
    /// (`IC231__maeSKILLref`) is `drIsWaveform` / `drGetWaveformXVec` /
    /// `drGetWaveformYVec`, or `maeOpenResults` + `ocnPrint`. Confirm with
    /// `skill.info` and `getd` before rewriting — see
    /// `docs/skill-fn-audit-2026-09-10.md` §2.7.
    ///
    /// `expression` is a Cadence expression like `vout` or
    /// `VT("/net015")` — passed verbatim to `ocnPrint` (no escape
    /// because it's not a SKILL string literal; it's a syntax tree).
    /// `analysis` selects the analysis context (`"tran"`, `"ac"`,
    /// `"dc"`, ...). `number_notation` must be one of `"scientific"`,
    /// `"engineering"`, `"automatic"`. `precision` is `1..=16`.
    /// `width` is `>= 4`.
    ///
    /// Panics on invalid arguments — these are programming errors,
    /// not user errors, since the constructor is meant to be called
    /// from a clap layer that already validated the inputs.
    pub fn export_waveform(&self, req: &ExportWaveformRequest<'_>) -> String {
        // Validate enum + ranges up front — see AGENTS.md "no validation
        // at system boundaries, trust types internally" rule: these
        // bounds are part of the type contract for the new command.
        if !(1..=16).contains(&req.precision) {
            panic!(
                "export_waveform: precision must be in 1..=16, got {}",
                req.precision
            );
        }
        if req.width < 4 {
            panic!(
                "export_waveform: width must be >= 4 (enough room for sign + 1 digit + decimal point), got {}",
                req.width
            );
        }
        match req.number_notation {
            "scientific" | "engineering" | "automatic" => {}
            other => panic!(
                "export_waveform: numberNotation must be one of [scientific, engineering, automatic], got {other:?}"
            ),
        }

        let session = escape_skill_string(req.session);
        let analysis = escape_skill_string(req.analysis);
        let expression_escaped = escape_skill_string(req.expression);
        let output_path_escaped = escape_skill_string(req.output_path);
        let number_notation_escaped = escape_skill_string(req.number_notation);
        let precision = req.precision;
        let width = req.width;

        // SKILL string literals for the format parameters; the
        // expression is itself a SKILL expression tree, escaped so
        // it can be embedded in a quote() form below.
        format!(
            r#"let((v) v = axlWaveformToList(axlGetWaveform(?session "{session}" ?expression "{expression_escaped}" ?analysis "{analysis}")) ocnPrint(?expr v ?numberNotation "{number_notation_escaped}" ?precision {precision} ?width {width} ?outputFile "{output_path_escaped}"))"#
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
        // The loop variable must not be `t`: `t` is SKILL's protected true
        // constant, so `foreach(t ...)` aborts with
        // "*Error* setq/set: Variable is protected and cannot be assigned to".
        r#"let((tests out sep) tests = maeGetResultTests() out = "[" sep = "" foreach(tst tests out = strcat(out sep sprintf(nil "\"%s\"" tst)) sep = ",") strcat(out "]"))"#.into()
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
        // History names are neither fixed nor guessable: they increment with
        // every run and the user can rename them in the GUI, so callers must
        // be able to enumerate them.
        //
        // The old directory heuristic got two things wrong on IC23.1:
        // - `asiGetResultsDir(...)/..` lands on `<history>/psf`, two levels
        //   below where histories live, so it listed *test* names;
        // - the `!index(h ".")` filter drops every name containing a dot,
        //   which excludes Cadence's own default names (`Interactive.0`,
        //   `Interactive.1`, ...). Together these made a correct answer
        //   impossible.
        //
        // Try the Maestro API first (wrapped in `errset` so an unbound symbol
        // on a given IC release degrades instead of erroring), then fall back
        // to scanning the histories root — the directory named `maestro` that
        // `asiGetResultsDir` sits under, found by climbing rather than by a
        // hard-coded level count.
        // Probed on IC23.1: every `mae*` history accessor is unbound
        // (maeGetHistoryNames / maeGetHistoryList / maeGetHistories /
        // maeGetResultHistories / maeGetResultsDir / maeGetHistory), and
        // `axlGetHistoryNames` is unbound too — only `axlGetMainSetupDB`
        // exists. So the history list is derived from the filesystem, where
        // each history is a directory directly under `<...>/results/maestro`.
        //
        // `asiGetResultsDir` returns a path *inside* one history
        // (`<root>/<history>/psf/<test>`), so walk its components forward and
        // stop at the first prefix ending in `/results/maestro` — that is the
        // root, without hard-coding how many levels deep the results dir sits.
        //
        // Two bugs this replaces: the old code used `<resultsDir>/..`, which
        // lands on `<history>/psf` and therefore listed *test* names; and it
        // filtered with `!index(h ".")`, which rejects Cadence's own default
        // history names (`Interactive.0`, `Interactive.1`, ...). History names
        // change with every run and can be renamed in the GUI, so this has to
        // enumerate rather than assume.
        r#"let((rd root found hs out sep) rd = car(errset(asiGetResultsDir(asiGetCurrentSession()))) root = nil found = nil when(stringp(rd) root = "" foreach(p parseString(rd "/") unless(found root = strcat(root "/" p) when(rexMatchp("/results/maestro$" root) found = t)))) hs = nil when(found && isDir(root) hs = setof(h getDirFiles(root) h != "." && h != ".." && !rexMatchp("^[.]" h) && isDir(strcat(root "/" h)))) out = "[" sep = "" foreach(h hs out = strcat(out sep sprintf(nil "\"%s\"" h)) sep = ",") strcat(out "]"))"#.into()
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
    if let Some(without_prefix) = trimmed.strip_prefix("(list ") {
        // Has list keyword: strip "(list " and trailing ")"
        let stripped = without_prefix
            .strip_suffix(')')
            .unwrap_or(without_prefix)
            .trim();
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
        let s = ops().open_session("myLib", "myCell", "adexl", "r");
        assert_eq!(s, r#"maeOpenSetup("myLib" "myCell" "adexl" ?mode "r")"#);
    }

    #[test]
    fn open_session_escapes_quotes() {
        let s = ops().open_session(r#"lib"x"#, "cell", "adexl", "r");
        assert!(s.contains(r#"lib\"x"#), "{s}");
    }

    #[test]
    fn open_session_passes_append_mode_through() {
        // "a" is the mode that takes the edit lock; it must reach maeOpenSetup
        // verbatim, because a caller asking for it is asking to write.
        let s = ops().open_session("myLib", "myCell", "maestro", "a");
        assert_eq!(s, r#"maeOpenSetup("myLib" "myCell" "maestro" ?mode "a")"#);
    }

    #[test]
    fn set_session_mode_picks_the_matching_function() {
        // The lock follows these two exactly (measured against the .cdslck
        // file on IC23.1): editable takes it, read-only gives it back.
        assert_eq!(
            ops().set_session_mode("fnxSession7", true),
            r#"maeMakeEditable(?session "fnxSession7")"#
        );
        assert_eq!(
            ops().set_session_mode("fnxSession7", false),
            r#"maeMakeReadonly(?session "fnxSession7")"#
        );
    }

    #[test]
    fn set_var_format() {
        let s = ops().set_var("Vdd", "1.8");
        assert_eq!(s, r#"maeSetVar("Vdd" "1.8")"#);
    }

    #[test]
    fn list_vars_reads_assembler_global_scope() {
        let s = ops().list_vars();
        // Must read the same scope `set_var` writes (`maeSetVar`/`maeGetVar`),
        // not the per-test/Explorer scope — the two do not mirror.
        assert!(s.contains("maeGetVar("), "{s}");
        assert!(!s.contains("asiGetDesignVarList"), "{s}");
        // IC23.1 has no `mae*` var enumerator; names come from maeGetSetup,
        // and only the spelling "variables" works ("vars"/"var" return nil).
        assert!(s.contains(r#"?typeName "variables""#), "{s}");
    }

    #[test]
    fn delete_var_format() {
        let s = ops().delete_var("Vdd");
        assert_eq!(s, r#"maeDeleteVar("Vdd")"#);
    }

    #[test]
    fn delete_output_format() {
        let s = ops().delete_output("Gain", "test1");
        assert_eq!(s, r#"maeDeleteOutput("Gain" "test1")"#);
    }

    #[test]
    fn delete_analysis_uses_symbol_and_verifies() {
        let s = ops().delete_analysis("ac");
        // The name must reach asiDeleteAnalysis as a symbol, not a string.
        assert!(s.contains(r#"stringToSymbol("ac")"#), "{s}");
        // asiDeleteAnalysis throws even on success, so the throw is swallowed
        // and the outcome decided by re-reading the enabled list.
        assert!(s.contains("errset(asiDeleteAnalysis"), "{s}");
        assert!(s.contains("maeGetEnabledAnalysis"), "{s}");
    }

    #[test]
    fn run_simulation_includes_session() {
        let s = ops().run_simulation("sess1");
        assert!(s.contains("maeRunSimulation"), "{s}");
        assert!(s.contains("\"sess1\""), "{s}");
    }

    #[test]
    fn get_analyses_ic23_resolves_test() {
        let s = ops().get_analyses("sess1", None, VirtuosoVersion::IC23);
        assert!(s.contains("maeGetSetup"), "IC23 must resolve the test: {s}");
        assert!(s.contains("maeGetEnabledAnalysis(tn)"), "{s}");
    }

    #[test]
    fn get_analyses_ic25_uses_test_name() {
        // 实测 IC25.1 ISR7：maeGetEnabledAnalysis(?session ...) 报错
        // IC23/IC25 均需 positional 传测试名
        let s = ops().get_analyses("sess1", None, VirtuosoVersion::IC25);
        assert!(
            s.contains("maeGetSetup"),
            "Both IC23 and IC25 need maeGetSetup: {s}"
        );
        assert!(s.contains("maeGetEnabledAnalysis(tn)"), "{s}");
    }

    #[test]
    fn get_analyses_uses_the_named_test_verbatim() {
        let s = ops().get_analyses("sess1", Some("TRAN"), VirtuosoVersion::IC23);
        assert!(s.contains(r#"tn = "TRAN""#), "{s}");
        assert!(s.contains("member(tn tests)"), "must validate the name: {s}");
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
        // `t` is SKILL's protected true constant — binding it aborts the loop.
        assert!(!s.contains("foreach(t "), "{s}");
    }

    #[test]
    fn get_history_list_uses_helper() {
        let s = ops().get_history_list();
        assert!(s.contains("asiGetResultsDir"), "{s}");
        assert!(s.contains("foreach"), "{s}");
        // Default history names contain a dot (`Interactive.0`), so a
        // dot-rejecting filter can never return them.
        assert!(!s.contains(r#"!index(h ".")"#), "{s}");
    }

    // === export_waveform tests (RED phase) ===
    //
    // Cadence SKILL ocnPrint with numberNotation/precision/width:
    //   (ocnPrint ?expr <expr> ?numberNotation <notation> ?precision <n>
    //             ?width <n> ?outputFile <path>)
    // notation ∈ { "scientific" | "engineering" | "automatic" }

    #[test]
    fn export_waveform_emits_ocn_print_with_formatting() {
        let s = ops().export_waveform(&ExportWaveformRequest {
            session: "sess1",
            expression: "vout",
            analysis: "ac",
            number_notation: "scientific",
            precision: 12,
            width: 18,
            output_path: "/tmp/wave.txt",
        });
        assert!(s.contains("ocnPrint"), "must call ocnPrint: {s}");
        assert!(s.contains("vout"), "expression must appear: {s}");
        assert!(s.contains("scientific"), "notation must appear: {s}");
        assert!(s.contains("12"), "precision must appear: {s}");
        assert!(s.contains("18"), "width must appear: {s}");
        assert!(s.contains("/tmp/wave.txt"), "output path must appear: {s}");
    }

    #[test]
    fn export_waveform_escapes_double_quotes_in_session_and_path() {
        // escape_skill_string escapes `\`, `"`, and control chars only —
        // spaces, slashes, and apostrophes are literal in SKILL string
        // literals. This test pins behavior for the most security-
        // relevant character: the double quote, which would otherwise
        // close the string literal early and enable injection.
        let s = ops().export_waveform(&ExportWaveformRequest {
            session: "sess\"x",
            expression: "/tmp/with spaces/wave.txt",
            analysis: "tran",
            number_notation: "automatic",
            precision: 8,
            width: 10,
            output_path: "/tmp/dest.txt",
        });
        // session and output path go through escape_skill_string;
        // double-quotes are backslash-escaped.
        assert!(
            s.contains(r#"sess\"x"#),
            "session double-quote must be escaped: {s}"
        );
        assert!(s.contains("/tmp/with spaces/wave.txt"));
        assert!(s.contains("/tmp/dest.txt"));
    }

    #[test]
    #[should_panic(expected = "precision must be in 1..=16")]
    fn export_waveform_rejects_precision_zero() {
        ops().export_waveform(&ExportWaveformRequest {
            session: "sess",
            expression: "vout",
            analysis: "ac",
            number_notation: "scientific",
            precision: 0,
            width: 18,
            output_path: "/tmp/x",
        });
    }

    #[test]
    #[should_panic(expected = "precision must be in 1..=16")]
    fn export_waveform_rejects_precision_above_16() {
        ops().export_waveform(&ExportWaveformRequest {
            session: "sess",
            expression: "vout",
            analysis: "ac",
            number_notation: "scientific",
            precision: 17,
            width: 18,
            output_path: "/tmp/x",
        });
    }

    #[test]
    #[should_panic(expected = "width must be >= 4")]
    fn export_waveform_rejects_width_below_four() {
        ops().export_waveform(&ExportWaveformRequest {
            session: "sess",
            expression: "vout",
            analysis: "ac",
            number_notation: "scientific",
            precision: 12,
            width: 3,
            output_path: "/tmp/x",
        });
    }

    #[test]
    #[should_panic(expected = "numberNotation must be one of")]
    fn export_waveform_rejects_unknown_notation() {
        ops().export_waveform(&ExportWaveformRequest {
            session: "sess",
            expression: "vout",
            analysis: "ac",
            number_notation: "hex",
            precision: 12,
            width: 18,
            output_path: "/tmp/x",
        });
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
        let s = ops().set_analysis("sess1", "ac", None, None, VirtuosoVersion::IC23);
        assert!(s.contains("maeGetSetup"), "IC23 must resolve the test: {s}");
        assert!(s.contains(r#"maeSetAnalysis(tn "ac""#), "{s}");
    }

    #[test]
    fn set_analysis_ic23_no_options() {
        let s = ops().set_analysis("sess1", "ac", None, None, VirtuosoVersion::IC23);
        assert!(
            !s.contains("?options"),
            "IC23 path must not inject options: {s}"
        );
    }

    /// The whole point of the test argument: with two tests in a session,
    /// `maeSetAnalysis` must be told which one rather than taking the first.
    #[test]
    fn set_analysis_targets_the_named_test() {
        let s = ops().set_analysis("sess1", "tran", None, Some("buf_tb"), VirtuosoVersion::IC23);
        assert!(s.contains(r#"tn = "buf_tb""#), "{s}");
        assert!(
            !s.contains("car(tests)"),
            "an explicit test must not fall back to the first one: {s}"
        );
    }

    /// Omitting the test on a multi-test session is an error, not a guess.
    #[test]
    fn set_analysis_without_a_test_refuses_an_ambiguous_session() {
        let s = ops().set_analysis("sess1", "tran", None, None, VirtuosoVersion::IC23);
        assert!(s.contains("cdr(tests)"), "must detect a second test: {s}");
        assert!(
            s.contains("pass \\\"test\\\" to say which one"),
            "the error must name the way out: {s}"
        );
        assert!(s.contains("car(tests)"), "one test is still resolved: {s}");
    }

    #[test]
    fn set_analysis_ic25_includes_keywords() {
        // IC25 uses ?session and ?enable t keywords (unlike IC23 positional-only)
        let s = ops().set_analysis("sess1", "ac", None, None, VirtuosoVersion::IC25);
        assert!(
            s.contains("?session"),
            "IC25 must include ?session keyword: {s}"
        );
        assert!(s.contains("?enable t"), "IC25 must include ?enable t: {s}");
        assert!(s.contains("maeGetSetup"), "IC25 needs the test name: {s}");
        assert!(
            !s.contains("?options"),
            "IC25 without options must not inject ?options: {s}"
        );
    }

    #[test]
    fn set_analysis_ic25_with_options() {
        // IC25: maeSetAnalysis with ?options using quote to protect inner pairs.
        // json_to_skill_alist returns "((\"stop\" \"1e10\") (\"start\" \"1\"))" — bare pairs.
        // skill_pair_to_quoted wraps each as quote(("stop" "1e10")).
        // Generated: let((setup opts) setup=car(...) opts=(list quote(...)) maeSetAnalysis(...))
        let s = ops().set_analysis(
            "sess1",
            "ac",
            Some(r#"(("stop" "1e10") ("start" "1"))"#),
            None,
            VirtuosoVersion::IC25,
        );
        assert!(
            s.contains(r#"quote(("stop" "1e10"))"#),
            "IC25 options must use quote: {s}"
        );
        assert!(
            s.contains("?options opts"),
            "Must pass options via ?options keyword: {s}"
        );
        assert!(
            s.contains("let((tests tn opts)"),
            "Must use a flat let holding the resolved test and the options: {s}"
        );
        assert!(
            s.contains("opts = (list"),
            "opts must be bound via = form: {s}"
        );
        assert!(
            s.contains("maeSetAnalysis(tn"),
            "maeSetAnalysis takes the test name first: {s}"
        );
        assert!(
            !s.contains("apply("),
            "No apply() needed for direct maeSetAnalysis: {s}"
        );
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
        // maeSetDesignForTest takes lib/cell/view positionally. The old form
        // used ?libName/?cellName/?viewName, which IC23.1 does not accept.
        let s = ops().set_design("sess1", "myLib", "myCell", "schematic", None);
        assert_eq!(
            s,
            r#"maeSetDesignForTest("myLib" "myCell" "schematic" ?session "sess1")"#
        );
        assert!(!s.contains("?libName"), "keyword form is wrong on IC23.1: {s}");
    }

    #[test]
    fn set_design_scopes_to_one_test_when_asked() {
        // Without ?test the design is set for every test in the session, which
        // is the right default but the wrong thing when a session has several.
        let s = ops().set_design("sess1", "myLib", "myCell", "schematic", Some("AC"));
        assert_eq!(
            s,
            r#"maeSetDesignForTest("myLib" "myCell" "schematic" ?test "AC" ?session "sess1")"#
        );
    }

    #[test]
    fn create_test_passes_the_design_in_the_same_call() {
        let s = ops().create_test("sess1", "tb1", "myLib", "myCell", "schematic", "spectre");
        assert_eq!(
            s,
            r#"maeCreateTest("tb1" ?lib "myLib" ?cell "myCell" ?view "schematic" ?simulator "spectre" ?session "sess1")"#
        );
    }

    #[test]
    fn create_test_escapes_names() {
        let s = ops().create_test("s", r#"t"x"#, "l", "c", "schematic", "spectre");
        assert!(s.contains(r#"t\"x"#), "{s}");
    }

    #[test]
    fn save_setup() {
        let s = ops().save_setup("sess1");
        assert!(s.contains("maeSaveSetup"), "{s}");
        assert!(s.contains("?session"), "{s}");
    }

    #[test]
    fn save_setup_refuses_a_read_only_session() {
        // maeSaveSetup returns success on a read-only session and writes
        // nothing, so the guard has to come before the call, not after it.
        let s = ops().save_setup("sess1");
        assert!(s.contains("axlIsSessionReadOnly"), "{s}");
        assert!(s.contains("error("), "{s}");
        assert!(
            s.find("axlIsSessionReadOnly") < s.find("maeSaveSetup(?session"),
            "the read-only check must run before the save: {s}"
        );
        // Probed, not assumed — a Virtuoso without the predicate still saves.
        assert!(s.contains("getd('axlIsSessionReadOnly)"), "{s}");
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

    #[test]
    fn run_with_dec_quotes_netpath_for_shell() {
        // The generated SKILL must protect netPath for POSIX shell-word use in sed/grep.
        // netPath is derived at runtime from getWorkingDir() — we cannot assume it is safe.
        // Required: a local sh_quote procedure defined before use, a quoted binding
        // (netPathQuoted), and that quoted binding used in the sprintf argument lists for
        // sed and grep.  The sed command template keeps %s placeholders; SKILL's sprintf
        // fills them with the shell-quoted strings from the argument list.
        // Without this, a path containing spaces or single-quotes causes shell injection
        // or argument-splitting errors at runtime inside Virtuoso's system() call.
        let s = ops().run_with_dec("sess1", "ac", None, 11);

        // 1. A local quoting helper must be defined inside the procedure.
        assert!(
            s.contains("procedure(vcliShellQuote"),
            "SKILL must define a local vcliShellQuote procedure: {s}"
        );

        // 2. netPath must be shell-quoted before use in system() commands.
        assert!(
            s.contains("netPathQuoted"),
            "SKILL must assign a quoted netPath (netPathQuoted): {s}"
        );

        // 3. vcliShellQuote must handle embedded single-quotes via the standard
        //    single-quote / double-quote / single-quote sequence.
        assert!(
            s.contains("'\"'\"'"),
            "vcliShellQuote must escape embedded apostrophes via '\\'\"'\"'' pattern: {s}"
        );

        // 4. The sed sprintf argument list must pass netPathQuoted (not raw netPath)
        //    so SKILL fills the %s placeholders with the shell-safe string.
        //    Old form: sprintf(nil "sed ... %s %s" ds netPath netPath)  ← raw netPath
        //    Fixed form: sprintf(nil "sed ... %s %s" ds netPathQuoted netPathQuoted)
        assert!(
            s.contains("ds netPathQuoted netPathQuoted"),
            "sed sprintf args must use netPathQuoted, not bare netPath: {s}"
        );

        // 5. Negative: the old raw netPath sed form must be absent.
        assert!(
            !s.contains("ds netPath netPath"),
            "raw 'ds netPath netPath' must not appear — use netPathQuoted: {s}"
        );

        // 6. The grep sprintf argument list must also use netPathQuoted.
        //    Old form ends with: sprintf(nil "grep '^ac ' %s" netPath)
        assert!(
            s.contains("\"grep '^ac ' %s\" netPathQuoted")
                || (s.contains("\"grep '^ac ' %s\"") && s.contains("netPathQuoted")),
            "grep sprintf args must use netPathQuoted, not bare netPath: {s}"
        );

        // 7. Negative: bare netPath after grep pattern must not appear.
        assert!(
            !s.contains("\"grep '^ac ' %s\" netPath)"),
            "bare netPath must not follow grep pattern — use netPathQuoted: {s}"
        );
    }
}

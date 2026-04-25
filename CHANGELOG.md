# Changelog

All notable changes to this project will be documented in this file.

## [0.3.11] - 2026-04-26

### Fixed
- **`maestro session-info`** — when the focused window is not an ADE window (e.g. waveform viewer or file browser), auto-selects if exactly one Maestro session exists; previously all fields were null in this case
- **`VirtuosoResult::ok_or_exec()`** — error message now includes the daemon error text for NAK responses; previously showed an empty message when SKILL threw an exception (as opposed to returning nil)

## [0.3.10] - 2026-04-26

### Fixed
- **`maestro get-analyses`** — `analyses` field is now a JSON array `["ac","dc"]` instead of a raw SKILL sexp string `"(\"ac\" \"dc\")"`; parsed with `parse_sexp` at the command layer
- **`maestro sim-messages`** — `messages` field now strips surrounding SKILL quotes; was returning `"\"\""` for empty messages instead of `""`

## [0.3.9] - 2026-04-26

### Refactored
- **`VirtuosoResult::ok_or_exec(context)`** — collapses the 16 repetitive `if !r.skill_ok() { return Err(...) }` blocks in `maestro.rs` into a single chained method call; error message format unchanged
- **`VirtuosoResult::output_unquoted()`** — replaces 7 inline `trim_matches('"')` sites
- **`SexpVal::as_str()`** — simplifies the `get_current_design()` closure in `bridge.rs`
- **`error.rs` nil suggestion** — narrowed `contains("nil")` to `ends_with(": nil")` to avoid false hints on unrelated error messages containing "nil" as a substring

## [0.3.8] - 2026-04-25

### Changed
- **All maestro commands** — SKILL failures now return `Err(VirtuosoError::Execution)` (exit 1) instead of `Ok({status:"error"})` (exit 0); LLM tool callers can now rely on exit code alone
- **`get_current_design()`** — replaced `split_whitespace` with `parse_sexp`; cellview names containing spaces no longer cause parse failures
- **`get_analyses()`** — added missing `status` field for consistency with all other commands
- **Success responses** — removed raw SKILL `output` fields from `close`, `set_var`, `add_output`, `open_results`, `run`; success paths only contain structured data
- **`run()`** — error path now returns `Err()` (exit 1); success still returns `{"status":"launched"}` to indicate async dispatch

### Added
- **`error::suggestion()`** — hints for `Execution` errors containing `nil`/`unbound` and for `NotFound` errors

### Removed
- **`DaemonNotReady`** error variant — never instantiated; removed from all match arms
- **`print_table` / `print_section`** in `output.rs` — never called
- **`skill_str()` helper** in `maestro.rs` — inlined into call sites after error-path removal

## [0.3.7] - 2026-04-25

### Added
- **`src/client/skill_sexp.rs`** — SKILL s-expression parser (`SexpVal` enum + `parse_sexp` + `sexp_to_str_list`); replaces the `sprintf`-JSON approach in `execute_skill_fetch` that silently corrupted field values containing `"` or `\n`
- **`SSHRunner::is_cm_failure()`** — detects ControlMaster failure patterns (`mux_client_request_session`, `could not create named pipe`, `ControlPath`, etc.)
- **`VB_SSH_CONFIG`** env var — path to a custom SSH config file, passed as `-F` to all SSH invocations
- **`VB_DISABLE_CONTROL_MASTER`** env var — pre-emptively disable CM (useful on WSL2/Windows where socket paths contain non-ASCII chars)
- Debug logging for `.env` load path and session directory on startup

### Changed
- **`build_fetch_skill`** — now emits `mapcar(lambda((o) list(o~>f1 ...)) expr)` (native SKILL list-of-lists) instead of `sprintf`-JSON; parsed with the new sexp parser
- **`SSHRunner`** — added `use_control_master: Cell<bool>`; `run_command` and `test_connection` automatically retry without CM on failure, persisting the disabled state
- **`try_ssh_tunnel`** — respects `use_control_master` flag and forwards `ssh_config_path`

## [0.3.6] - 2026-04-25

### Added
- **`execute_skill_fetch()`** — batch-fetch multiple `~>slot` fields from a SKILL list in a single bridge RTT; returns `Vec<HashMap<String, String>>`

### Fixed
- **`#[allow(dead_code)]`** — suppress clippy warnings on `get_outputs` and `get_current_session` APIs reserved for future use

### Changed
- **`add_output` version branch** — removed dead IC25 branch and redundant `raw` field; IC23/IC25 dispatch unified

## [0.3.5] - 2026-04-24

### Fixed
- **SSH `sh -c` argument passing** — `upload()` and `upload_text()` now pass `"sh -c 'command'"` as a single SSH argument, fixing `&&`-chained commands that were silently broken
- **`maestro history-list`** — no longer requires `--session` arg; uses `asiGetResultsDir` to discover runs from the current session
- **`get_current_session`** — returns `"nil"` string instead of SKILL nil on no-session, avoiding `skill_ok()` false negative

### Added
- **SSH port in RAMIC Bridge banner** — `ramic_bridge.il` displays `SSH: <port>` in the ready banner for quick tunnel setup
- **`tunnel-connect` skill** — Quick Connect section: extract Session/Port/SSH directly from the banner
- **`maestro get-analyses`** — version-aware IC23/IC25 dispatch via `VirtuosoVersion`
- **`maestro add-output`** — takes `VirtuosoVersion` parameter for future IC25 divergence
- **`maestro get-outputs`** — uses struct accessors (`~>name`, `~>outputType`, `~>signalName`, `~>expr`) matching IC23.1/IC25.1 actual return type

### Changed
- **`maestro get-result-tests`** — inline JSON serialization replacing `skill_strings_to_json` helper (avoids double-wrapping)
- **`maestro get-result-outputs`** — same inline serialization fix

## [0.3.4] - 2026-04-24

### Fixed
- **`vcli tunnel start` SSH upload bug** — `upload()` and `upload_text()` were passing "sh", "-c", and command as three separate arguments to SSH, which concatenated them without quotes, breaking commands with `&&`. Now passes `"sh -c 'command'"` as a single argument.

### Added
- **SSH port in RAMIC Bridge banner** — `ramic_bridge.il` now displays the SSH port number in the Ready banner, making it easier to extract connection parameters at a glance.
- **`tunnel-connect` skill updated** — documents how to connect from the banner, extracting Session, Port, and SSH values directly.

## [0.3.0] - 2026-04-19

### Added
- **`vcli maestro session-info`** — inspect the focused ADE Assembler/Explorer window; returns `lib`, `cell`, `view`, `editable`, `unsaved_changes`, and `run_dir` as structured JSON
- **Callback File IPC** — replaces `ipcWriteProcess` with a temp-file pair protocol (`/tmp/.ramic_cb_{port+1}` + `.done` marker); fixes IC23.1/RHEL8 platform bug where `ipcWriteProcess` data handler stops firing after the first call
- **`spectre-netlist-template` skill** — 9 circuit-type templates (OTA, diff-OTA, LDO, comparator, bandgap reference, current mirror, active filter, VCO, LNA) with verified vsource/isource/analysis syntax from IC231 documentation
- **`inject_stimulus.py` script** — standalone Python helper (no deps) that auto-detects circuit type from `subckt` port names and writes a complete Spectre testbench wrapper with stimulus + analysis statements

### Fixed
- **Callback file `cb_port` arithmetic** — daemon now derives `cb_port = actual_port + 1` from `listener.local_addr()` instead of `argv[2]`; previously the OS-assigned port was never propagated so all callback files were written to `/tmp/.ramic_cb_1`

### Changed
- **Release workflow** — new `.github/workflows/release.yml` builds Linux x86_64 release binaries and publishes to crates.io on `v*` tags

## [0.2.0] - 2026-04-18

### Changed
- **`vcli optim` removed** — migrated to `circuit-optimizer` skill script (`scripts/run_bandgap_sweep.py`); deleted 650 lines of Rust and the `serde_yaml` dependency
- **Zombie job fix** — `jobs.rs::refresh()` no longer marks a spectre process as alive based on PID alone; validates against the simulation log file to detect completed runs whose OS process has already exited

## [0.1.5] - 2026-04-15

### Added
- **`Orient` enum** for schematic instance orientation — type-safe replacement for `String`, derives `clap::ValueEnum` + `serde::Deserialize` so both CLI (`--orient`) and JSON spec (`build --spec`) reject invalid values at the boundary. Accepts exactly the 8 Cadence orientations: R0, R90, R180, R270, MX, MY, MXR90, MYR90
- **`maestro add-output` now resolves setup name from session internally** — previously passed session ID as SKILL output name and user name as setup name, causing `maeAddOutput` to always return nil

### Fixed
- **`sim::job_list` no longer uses `unwrap_or_default()`** — propagates serialization errors via `VirtuosoError::Execution` per project convention
- **`parse_skill_json` returns `Result<Value>`** instead of silently falling back to `{"raw": output}` — surfaces SKILL output corruption instead of masking it; all call sites updated to propagate the error
- **`cv_guard` injection in `schematic_ops.rs`** — every schematic operation now validates the `RB_SCH_CV` global SKILL variable is bound before use, surfacing clear errors instead of cryptic SKILL failures

### Refactored
- **`main.rs` dispatcher extraction** — 239-line central match reduced to 12 lines by extracting 9 `dispatch_*` functions (one per command group)
- **`measure` expression validation** — new `validate_measure_expr` blocks destructive SKILL calls (`system`, `ipcBeginProcess`, `deleteFile`, `load`, `evalstring`, …) before execution

## [0.1.4] - 2026-04-15

### Added
- **`vcli window` subcommand group** — `list`, `dismiss-dialog`, `screenshot`
  - `list`: enumerate all open Virtuoso windows with derived mode labels (`ade-editing`, `ade-reading`, `schematic`, `layout`, `other`); handles SKILL octal escapes (`\256` = ®) that break standard JSON parsers
  - `dismiss-dialog [--action ok|cancel] [--dry-run]`: programmatically cancel or confirm a blocking GUI dialog
  - `screenshot --path FILE [--window PATTERN]`: capture via X11 ImageMagick `import -window root` (IC23.1 fallback — `hiGetWindowScreenDump` is IC25+ only)
- **`vcli maestro set-analysis`** — enable an analysis type (ac/dc/tran/noise/…) on a setup by session name; resolves setup internally via `maeGetSetup`

### Fixed
- **`maestro add-output`** — parameter order was completely wrong: session ID was passed as SKILL output name and user-supplied name as setup name, causing `maeAddOutput` to always return nil; now resolves setup from session automatically
- **`maestro get-analyses`** — `maeGetEnabledAnalysis` takes a positional setup name (not `?session` keyword) in IC23.1; setup name is now resolved via `maeGetSetup` internally
- **`--session` global arg no longer clobbers `VB_SESSION`** — bridge session ID and Maestro session name can coexist without conflict

## [0.1.3] - 2026-04-15

### Fixed
- **format tracing::debug line in bridge.rs** — fix log formatting issue
- **maestro: align SKILL function signatures with IC25.1 official documentation** — fixes Maestro operations compatibility

### Added
- **New skills** — `circuit-optimizer`, `sim-plot`, `schematic-gen`, `spectre-netlist-gotchas` — see [.claude/skills/](.claude/skills/)
- **Maestro skill** and Virtuoso reference documentation

### Dependencies
- Updated various dependencies for stability

## [0.1.2] - 2026-04-13

### Added
- **Interactive TUI Dashboard** — `vtui` binary with Sessions/Jobs/Config tabs
- **Remote Session Auto-Discovery** — `vcli tunnel start` syncs remote sessions
- **Remote Async Spectre Simulation** — `vcli sim run-async` works via SSH nohup
- **SSH Configuration** — `VB_SSH_PORT`, `VB_SSH_KEY` support
- **IC23.1+ Maestro Explorer Support** — `vcli maestro` commands

## [0.1.1] - Previous release

See [release history](https://github.com/deanyou/virtuoso-cli/releases)

# Changelog

All notable changes to this project will be documented in this file.

## [0.4.0-alpha.10] - 2026-06-13

### Added
- **Centralized `runtime_paths` module** (`src/runtime_paths.rs`, ported from
  virtuoso-bridge-lite `6b9309d`). Honors XDG Base Directory spec
  (`XDG_CACHE_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`) with `VB_*`
  env-var overrides (`VB_CACHE_DIR`, `VB_LOG_DIR`, `VB_OUTPUT_DIR`,
  `VB_TMP_DIR`, `VB_STATE_DIR`, `VB_CONFIG_DIR`) and a `VB_HOME` umbrella.
  Backward-compatible default: `~/.cache/virtuoso_bridge/...` on Linux.
  Eleven call sites migrated: `command_log`, `auth`, `config`, `history`,
  `models`, `transaction/snapshot`, `transport/ssh`, `transport/tunnel`,
  `plugins/registry`, `skill_finder`, `spectre/jobs`, `commands/process`.
- **X11 helper structured errors** (`src/transport/x11.rs`). New
  `extract_helper_errors()` surfaces three independent failure signal
  sources — structured JSON errors on stdout, non-zero returncode with
  stderr context, and non-empty stderr alone — deduped via `BTreeSet`.
  Wired into `dismiss()`, `dismiss_window()`, `list_dialogs()`, and
  `list_windows()`; the list paths now return `VirtuosoError::Execution`
  when the helper dies, so "no windows" can no longer be confused with
  "helper crashed".
- **Digital import verification recipes** (`.claude/skills/digital-import/SKILL.md`,
  ported from virtuoso-bridge-lite `1ae2156`). Step 1 (strmin) now
  includes a `length(cv~>shapes) + length(cv~>instances) > 0` check
  that catches the silent-stub failure mode where strmin prints
  "Translation completed" and exits 0 but leaves an empty layout cell.
  Step 2 (ihdl) adds a `ddGetObj(lib mod view)` check for `schematic`
  + `symbol` views to catch ihdl's partial-import mode.

### Tests
- 13 unit tests in `runtime_paths` (env precedence, blank values,
  profile variants, XDG fallback).
- 6 unit tests in `transport::x11` (extract_helper_errors: JSON
  errors, non-zero returncode, dedup, distinct-message preservation).
- 2 integration tests in `daemon_user_guard.rs` (`VB_CACHE_DIR` /
  `VB_LOG_DIR` end-to-end override via the public `runtime_paths` API).
- Fixed pre-existing test isolation race: `save_to_session_file_*`
  tests now hold `ENV_LOCK` to serialize `XDG_CACHE_HOME` mutations.

## [0.4.0-alpha.9] - 2026-06-03

### Fixed
- **Multi-profile CIW setup-file collision** (ported from upstream
  virtuoso-bridge-lite PR #86). Previously, two profiles on the
  same remote host would both write to `/tmp/virtuoso_bridge/ramic_bridge.il`,
  so the second profile's `tunnel start` would silently overwrite
  the first profile's CIW setup file. The first profile's CIW `load()`
  would then start the wrong daemon. After the fix:
  - profile A's setup dir: `/tmp/virtuoso_bridge_analog/`
  - profile B's setup dir: `/tmp/virtuoso_bridge_digital/`
  - `tunnel stop` / `cleanup_remote` is now profile-scoped and never
    wipes other profiles' dirs
- **Tunnel cleanup wiping per-client scratch**: the previous
  `rm -rf /tmp/virtuoso_bridge` in `cleanup_remote` was also
  deleting all per-client `client_id` subdirs (those created by
  the per-client scratch scoping feature). Now scoped to the
  active profile's dir.

### Added
- **`vcli profile bind` / `vcli profile clear` extended with three
  scopes** (was `--venv` only):
  - `--venv`  → write `$VIRTUAL_ENV/.vcli-profile` (Python venv binding)
  - `--user`  → write `~/.vcli/.env` `VB_PROFILE=...` line (user default)
  - `--local` → write `./.vcli-profile` (current working dir)
  Re-binding replaces the existing `VB_PROFILE=` line (no duplicate
  append). Empty / whitespace-only / newline-injected names are
  rejected. Other lines in `~/.vcli/.env` are preserved by `clear`.
- **Public helpers** in `virtuoso_cli::transport::tunnel` for
  external consumers:
  - `profiled_bridge_leaf(Option<&str>) -> String`
  - `profiled_env_key(&str, Option<&str>) -> String`
  - `setup_dir_for_profile(Option<&str>) -> String`

### Security
- Profile name sanitization in setup-dir leaf: any char outside
  `[A-Za-z0-9._-]` is replaced with `_`, length capped at 64.
  Path-traversal attempts (`../etc/passwd`, etc.) are neutralized.
  All-underscore results fall back to `virtuoso_bridge_profile` to
  avoid shadowing the no-profile leaf.

### Tests
- +9 unit tests in `src/transport/tunnel.rs` (sanitization, length
  cap, fallback, two-profile isolation)
- +4 unit tests in `src/profile.rs` (bind/clear round-trip, line
  preservation, empty/newline rejection) + `Mutex` for parallel
  test safety on `~/.vcli/.env`
- +7 integration tests in `tests/tunnel_profile.rs` (helper
  invariants, CLI plumbing end-to-end, error handling)

Total: 1133 tests pass, 0 clippy warnings.

## [0.4.0-alpha.8] - 2026-06-03

### Added
- **aarch64 cross-compiled `virtuoso-daemon`** — ships at
  `resources/daemons/virtuoso-daemon-aarch64` (398 KB, ELF AArch64).
  Lets `vcli tunnel start` deploy to ARM64 hosts (AWS Graviton, Apple
  Silicon Linux VMs, Ampere Altra) without needing a Rust toolchain
  on the remote side. Architecture is auto-detected at deploy time
  and the matching binary is uploaded.
  - `docs/aarch64-cross-compile.md` documents the manual Ubuntu
    22.04 aarch64 sysroot setup under `/usr/aarch64-linux-gnu/sys-root/`
    (Rocky 8 has no aarch64 glibc in its dnf repos) and the
    `cannot find -lgcc_s` linker trap that required the explicit
    `-L` flag in the gcc wrapper.
  - GitHub Actions release workflow now builds the aarch64 daemon
    in CI via cross-compile (Ubuntu runner + gcc-aarch64-linux-gnu +
    rustup target), so the binary in the GitHub release is rebuilt
    fresh on every tag.

### Changed
- **README.md** — bilingual sync to v0.4.0-alpha.7+. Both English
  and Chinese sections now document:
  - `vcli session show` reports `daemon_version` + `version_skew`
  - Stale-daemon recovery on `ramic_bridge.il` `load`
  - Native cross-arch tunnel deploy (x86_64 / aarch64)
  - Per-client scratch scoping via `VB_CLIENT_ID`
  - Skill Finder 5 search modes (`vcli skill find --mode {fuzzy,prefix,suffix,exact,regex}`)
  - Admin capability gate (`VCLI_CAPABILITY=admin`) for `skill broadcast` and raw SKILL exec
  - Ready banner version: 0.3.18 → 0.4.0-alpha.7
  - Command Reference: `skill broadcast` (admin), `skill find`, `skill info`
  - Configuration table: `VB_CLIENT_ID`, `VCLI_CAPABILITY`
- **Release workflow** (`release.yml`) — added `build-linux-aarch64`
  job that cross-compiles `virtuoso-daemon` for aarch64 and uploads
  the binary to the GitHub release.

## [0.4.0-alpha.7] - 2026-06-02

### Fixed
- **`vcli skill find` end-to-end** — the .fnd parser in
  `src/skill_finder/parser.rs` expected a fictional 3-line-per-entry
  format that does not exist in Cadence's SKILL Finder database.
  The real format is a SKILL list literal:
  `("name" "syntax" "description")` per entry, with newlines
  preserved inside the strings. The parser was rewritten as a small
  state machine that correctly handles:
    - 3-string entries (the common case, ~9808 of 9812 entries)
    - 4-string entries with empty placeholder (4 entries in
      `maeSKILLref.fnd`)
    - Multi-line syntax and description strings
    - Embedded `(` and `)` inside description text
      (e.g. `deselected.)` at the end of a sentence)
    - `;`-prefixed comments and blank lines between entries
  The old parser returned 0 results for every search because
  the first line of each entry was `("name"` (with the open paren)
  which it then stored as the function name. `vcli skill find` now
  returns real results for all 5 search modes (fuzzy, prefix,
  suffix, exact, regex) and `--include-desc` matches against the
  description field as designed.

### Tests
- 7 new parser unit tests, including verbatim samples from
  `abstract.fnd` and `skdfref.fnd` plus a regression test that
  reads `/opt/cadence/IC231/doc/finder/SKILL/SKILL/abstract.fnd`
  directly and asserts > 50 well-formed entries (skips if the
  fixture is absent).
- Total: 1090 tests pass (was 1058).

## [0.4.0-alpha.6] - 2026-06-02

### Added
- **CLI/daemon version unification** — `vcli session show` now reports
  the daemon's version (from the SKILL global `RBDVersion`, populated by
  `RBIpcErrHandler` parsing the `VERSION:x.x.x` line the Rust daemon prints
  to stderr on startup) and warns when it does not match the vcli binary
  version (`CARGO_PKG_VERSION`). Catches the common "I upgraded vcli but
  forgot to reload `ramic_bridge.il`" footgun where the SKILL wrapper
  falls out of sync with the binary.
- **`SessionInfo.daemon_version`** — optional field with the same
  backward-compat guarantees as `daemon_user` (older `ramic_bridge.il`
  never writes it; legacy session files still parse via
  `#[serde(default, skip_serializing_if = "Option::is_none")]`).
- **`VirtuosoClient::get_daemon_version()`** — narrow internal API
  (fixed-literal SKILL payload, no capability required). Returns
  `Ok(None)` for the empty/`"?"` placeholder so old daemons that never
  emitted `VERSION:` are not falsely flagged.
- **`check_version_skew()` helper in `commands::session`** — pure
  function with 4 unit tests (match / mismatch / empty / `?`).
- **`ramic_bridge.il` `; RB_VERSION: 0.4.0-alpha.6` stamp** — human-
  readable top-of-file marker. Two new regression tests
  (`ramic_bridge_has_rb_version_stamp`,
  `ramic_bridge_version_stamp_matches_cargo_toml`) refuse to merge a
  drift between the .il stamp and `Cargo.toml`.

### Changed
- `vcli session show` JSON now includes `daemon_version` and
  `cli_version` in the `session` block, plus a new
  `warnings.version_skew` field.

## [0.4.0-alpha.5] - 2026-06-01

### Added
- **Cross-user daemon guard** — `vcli session show` queries the daemon's Unix
  `$USER` via `getShellEnvVar` and warns when it does not match the configured
  `VB_REMOTE_USER[<profile>]`. Suppressed by `VB_ALLOW_CROSS_USER_DAEMON=1`.
  Catches SSH-tunnel-to-wrong-user misconfigurations that previously failed
  silently with confusing SKILL output.
- **Stale-daemon recovery hint** — `vcli session show` now prints a recovery
  procedure (RBStop / RBStopAll / re-load) when the daemon port is bound but
  the daemon is not responding to SKILL. Mirrors the new hint added to
  `ramic_bridge.il` `RBStart()` "already running" branch.
- **`VirtuosoClient::get_daemon_user()`** and **`daemon_alive()`** — narrow
  internal API used by the session show probes. Both use fixed-literal SKILL
  payloads so they require no SKILL capability.
- **Per-client remote scratch scoping** — `load_il` now uses
  `/tmp/virtuoso_bridge/{client_id}/{filename}` instead of the unscoped
  `/tmp/virtuoso_bridge/{filename}`. Resolution order:
  `VB_CLIENT_ID` > `VB_PROFILE` > `gethostname()`. Prevents name collisions
  when multiple local machines share one remote Unix account.
- **`SessionInfo.daemon_user`** — optional field, populated lazily by
  `session show` and persisted to the per-session JSON file via the new
  `save_to_session_file()` method. Backward-compatible with legacy session
  files (older `ramic_bridge.il` versions never write this key).
- **`--include-desc` flag for `vcli skill find`** — matches against the
  description field, not just the function name. Prefix/suffix/exact
  modes retain name-shape filtering; fuzzy and regex also check descriptions.
- **SKILL Finder CLI** (`vcli skill find` / `vcli skill info`) with **remote
  cache via SSH** — fetches `.fnd` corpus from the remote EDA host and caches
  locally for offline search.
- **New RPC methods** registered: `skill.eval`, `maestro.snapshot`,
  `schematic.polish_label`, `spectre.max_workers`.
- **Maestro CSV parser** with integration tests and helpers.
- **Spectre pasc callback handler** (`08_set_simulator_mode.il`) plus
  `VTTYPE` parameter update examples.
- **Spectre sweep parser** enhancements and comprehensive test coverage.
- **`VB_SPECTRE_BIN` env var** support for absolute spectre binary path.
- **Pensieve frontmatter** added to 39 knowledge/decision/maxim files.

### Fixed
- **`ipcIsProcessRunning()` no-arg call returns nil** — all three call sites
  (`bridge::ping`, `rpc::dispatcher::"ping"`, `VirtuosoClient::daemon_alive`)
  replaced with the no-op SKILL probe `plus(1 1)`. The no-arg form of
  `ipcIsProcessRunning()` was returning nil on every live daemon, causing
  `vcli rpc call util.ping` to spuriously fail. **Live verified**:
  `util.ping` now returns `{"status":"ok"}` on a clearly-alive daemon.
- **`ensure_remote_dir` was a no-op in local mode** — `vcli skill load` in
  local mode (no SSH tunnel) was returning "io error: No such file or
  directory" for the per-client scratch dir. `ensure_remote_dir` now calls
  `std::fs::create_dir_all` in local mode (mkdir -p via SSH in remote mode).
- **`save_to_session_file` mkdir-p** — newly added method now creates the
  parent directory if it doesn't exist, so cold callers (no prior SKILL
  bridge init) work without manual setup.
- **SKILL syntax errors in `pasc_callbacks.ils`** corrected.
- **Addressed upstream `virtuoso-bridge-lite` issues #92 and #81**:
  profile-aware `Skill` config init; ensure `RBStart()` is idempotent.
- **`maestro.snapshot` 08_set_simulator_mode** made portable + verifiable.
- **Clippy and fmt warnings** addressed across dispatcher and tests.

### Security
- **`ensure_remote_dir` now uses `shell_quote`** for defense-in-depth on the
  SSH-side `mkdir -p` invocation. Current callers only pass sanitized
  client_id (alnum + `-_.`), so no actual shell-injection surface, but
  future callers passing user-controlled paths are protected.
- **`shell_quote` promoted to `pub(crate)`** for cross-module reuse.

### Tests
- 1058+ passing tests, 0 failures (post-fix).
- New integration tests: `tests/daemon_user_guard.rs` (18 tests),
  `tests/skill_finder_include_desc.rs` (11 tests).
- Regression tests pinning the `plus(1 1)` probe at all three call sites
  and the `ipcIsProcessRunning()` ban.

## [0.3.18] - 2026-05-01

### Added
- **Session history** — vcli now records two history streams per session:
  - **SKILL layer** (`~/.cache/virtuoso_bridge/history/<session_id>.jsonl`): every `execute_skill()` call, timestamp + code + ok flag + output (first 512 chars); written only when a session is resolved (not on raw VB_PORT fallback)
  - **CLI layer** (`~/.cache/virtuoso_bridge/history/cmd.jsonl`): every vcli invocation, timestamp + args + exit code + session ID; written for all commands including failures
- **`vcli session history <id>`** — show SKILL and CLI history for a session; supports `--skill` (SKILL only), `--cmd` (CLI only), `--limit N` (default 50)
- `VirtuosoClient::session_id` field — the resolved bridge session ID is now available on the client struct for tooling and introspection

## [0.3.17] - 2026-05-01

### Fixed
- **Stale session auto-cleanup in `from_env()`** — `VirtuosoClient::from_env()` now filters dead sessions (port not open) before raising "multiple sessions active"; previously a crashed Virtuoso's leftover session file would permanently block all commands until manually deleted

### Added
- **`vcli session current`** — dry-run of session auto-discovery: shows which session would be selected (or "ambiguous" if multiple live sessions exist)
- **`vcli session cleanup`** — removes session files for daemons that are no longer running; returns JSON with count and list of removed IDs
- **Stale/live session cleanup tests** — `stale_session_filtered_in_cleanup` and `live_session_not_removed_by_cleanup` cover the cleanup() boundary

## [0.3.16] - 2026-05-01

### Fixed
- **Multi-Virtuoso session collision** — `ramic_bridge.il` now uses the OS-assigned port as the session ID suffix (`hostname-user-<port>`) instead of a per-process sequence counter (`hostname-user-1`); when two Virtuoso instances ran concurrently, both generated `hostname-user-1` and the second `RBWriteSession` silently overwrote the first session file, making the original session invisible to `vcli`
- Removed the now-dead `RBSessionSeq` global (was only used as a placeholder immediately overwritten by the port-based ID)

### Added
- **Session coexistence tests** — `session_info_tests` covers: two port-based sessions coexist without collision, session ID suffix equals port field, port survives JSON round-trip, all sessions visible for `--session` disambiguation

## [0.3.15] - 2026-05-01

### Fixed
- **`ramic_bridge.il` multi-statement SKILL truncation** — `evalstring(data)` replaced with `evalstring(strcat("(progn " data ")"))` so that SKILL payloads containing multiple expressions (e.g. `let(...)` blocks) execute fully; previously only the first top-level form was evaluated and the rest were silently discarded

## [0.3.14] - 2026-04-30

### Added
- **`virtuoso-daemon --version`** — prints the semver and exits; previously the only way to check the daemon version was `strings(1)` on the binary

### Changed
- **vtui daemon stats caching** — `DaemonStats` is now refreshed on the 500ms tick and cached in `App::daemon_stats`; previously loaded from disk on every render frame, causing unnecessary I/O
- **`DaemonStats::path(port)`** — centralizes the stats file path; `render_detail` and `write_stats` both derive the path from this single source

## [0.3.13] - 2026-04-29

### Added
- **SSH login shell hardening** — `run_command_inner()` now invokes `sh -l -s` instead of `sh -s`; sources `/etc/profile` and `~/.profile` on EDA hosts where the login shell is csh/tcsh, ensuring PATH is populated correctly
- **Daemon runtime metrics** — `virtuoso-daemon` tracks total calls, error count, and uptime using `AtomicU64` + `OnceLock<Instant>`; writes `{"calls":N,"errors":N,"uptime_secs":N}` to `/tmp/.ramic_stats_{port}` after each request
- **`DaemonStats::load(port)`** — reads the stats file from a running daemon; returns `None` if the daemon has never written stats (e.g. pre-0.3.13 or not yet started)
- **vtui Sessions detail pane** — shows Calls / Errors / Uptime rows when a daemon stats file is available for the selected session's port

## [0.3.12] - 2026-04-27

### Changed
- **`.env` upward discovery** — `Config::from_env()` now walks from the current directory up to the filesystem root looking for a `.env` file; previously only the working directory was checked, causing config to silently disappear when `cd`-ing into a project subdirectory

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

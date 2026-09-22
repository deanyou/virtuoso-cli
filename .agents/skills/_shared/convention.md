# vcli ↔ virtuoso naming convention

This document is the **single source of truth** for which CLI name the project's
SKILL corpus uses, why, and how it evolves. It is paired with the wrapper at
`./vcli.sh` in this same directory.

## TL;DR

| Name        | What it is                                                | Who calls it                                  |
| ----------- | --------------------------------------------------------- | --------------------------------------------- |
| `vcli`      | The actual Rust binary — `[[bin]] name = "vcli"` in Cargo | The binary itself                             |
| `virtuoso`  | The conventional name used inside SKILL.md examples       | 16 of the 27 SKILL.md files (document corpus) |
| `_shared/vcli.sh` | The adapter that bridges both names                | Installed on `PATH` as either name            |

If a user runs `vcli skill exec '1+1'`, the binary is invoked directly. If a
user runs `virtuoso skill exec '1+1'` and the wrapper symlink is on `PATH`, the
wrapper forwards to the binary. Both produce identical exit codes and stdout.

## Why the corpus uses `virtuoso`

This is a deliberate upstream choice, not a bug:

1. **Semantic clarity.** Cadence's own EDA tool is also called `virtuoso`.
   Calling the SKILL-over-TCP bridge `virtuoso` would collide with the GUI
   executable and confuse logs. Calling the bridge `vcli` keeps the two distinct.
2. **Documentation drift.** The README documents `virtuoso tunnel start` etc.
   The SKILL.md corpus inherits the README's wording. The Cargo `[[bin]]`
   stayed `vcli` to preserve the README's collision-avoidance rationale.
3. **Adapter-friendly.** Splitting "documented name" (UI-facing) from
   "binary name" (exec-facing) lets us fix wrapper-level mismatches without
   rewriting 16 SKILL.md files every time.

## How the adapter works

`_shared/vcli.sh`:

- Detects which name it was called by (`$0`).
- Locates the real `vcli` binary in this order:
  1. `$VCLI_BIN` env var (override)
  2. `command -v vcli` (PATH)
  3. `~/.cargo/bin/vcli` (cargo install default)
  4. `/opt/cargo/bin/vcli` (system-wide install)
- Forwards argv unchanged — does NOT inject `--format json`; that is the
  caller's call. The skill's `allowed-tools` and `Bash(...)` glob already
  controls what the agent may invoke.
- Logs stderr to `$VB_LOG_DIR/harness-YYYYMMDD.log` so forensic traces survive
  the harness tool's stdout-buffer TTL.
- Surfaces the real exit code; `set -euo pipefail` is set so upstream errors
  propagate.

## Install (one-time, per agent host)

```bash
# from repository root
SHARED="$(pwd)/.claude/skills/_shared/vcli.sh"
mkdir -p ~/.local/bin
ln -sf "$SHARED" ~/.local/bin/vcli
ln -sf "$SHARED" ~/.local/bin/virtuoso
export PATH="$HOME/.local/bin:$PATH"

# verify
virtuoso doctor
vcli doctor            # both should produce identical output
virtuoso --version     # = vcli --version
```

After install, **do not edit a SKILL.md just to switch `virtuoso` → `vcli`**.
The wrapper handles it.

## When the name changes

If the binary is ever renamed again (e.g. upstream picks `vc` or merges
`virtuoso-daemon` into a single file), the migration surface is:

| File                                          | Change                                                  |
| --------------------------------------------- | ------------------------------------------------------- |
| `.claude/skills/_shared/vcli.sh`              | Edit `VCLI_BIN` resolution + bump the docstring        |
| `.claude/skills/_shared/convention.md`        | Update the table + the install snippet                  |
| 0 SKILL.md files                              | Unchanged                                               |
| 0 `allowed-tools` frontmatter                 | Unchanged                                               |

That's the single-point contract. The 16 SKILL.md files are *clients* of the
wrapper, not owners of the name.

## SKILL.md hygiene rules (for skill authors)

1. Use `virtuoso <subcmd> ...` in your examples. This matches the README and
   every other skill. Do **not** switch to `vcli` "for consistency" — it
   breaks grep-based bulk refactors and confuses new readers.
2. `allowed-tools` for shell-calling skills should be `Bash(virtuoso *)`.
   Optionally also list `Bash(vcli *)` if the skill directly references both
   (see `ocean-netlist-regen` and `skill-exec` for precedent).
3. Never embed user-controlled strings in a `vcli skill exec '...'` argument.
   Always pass them as `vcli skill exec -- "1+1 $user_value"` so that
   `bridge::escape_skill_string()` (project invariant, src/client/bridge.rs)
   handles the escape. The wrapper does **not** validate SKILL payloads.
4. Errors must propagate. Do not `|| true`. The harness `bash` tool reads
   `[exit code: N]` markers — the wrapper guarantees the binary's exit code is
   the wrapper's exit code.

## Diagnostics

Run `virtuoso doctor` (or `vcli doctor`) for a one-shot summary of what the
wrapper sees: the binary path it resolved, the version, the log directory,
and the current `VB_*` environment overrides.

## Diagnostic playbook — general rules

The wrapper is a thin `exec` shim. The rules below abstract over the
specific hosts, libraries, session IDs, and cell names that triggered
them. Each rule names the **observable pattern**, the **invariant in
the system**, and a **universal recovery** that does not depend on the
particulars of any one debugging session.

### Rule: vcli's disk-cache writes are silent-on-failure

**Pattern** — `vcli session history`, `cmd.jsonl`, `~/.cache/virtuoso_bridge/...`
files appear empty or stale even though the CLI printed success.

**Why** — vcli opens cache files with
`fs::OpenOptions::new().create(true).append(true).open(path)` and discards
the resulting `io::Error` (`src/history.rs:36–44, :60–73`). There is no
stderr noise and no exit-code change when the cache cannot be written.

**Common causes**

- DSH sandbox `workspace-write` blocks writes outside the session workspace.
- Pre-existing cache directory with wrong ownership or `chmod a-w`.
- Read-only mount after a previous crash.

**Probe**

```bash
echo probe > "$VB_CACHE_DIR/history/__probe.txt" 2>&1 || echo BLOCKED
```

If `BLOCKED`: the fix is *not* in vcli.

**Recoveries**

| # | Action                                                                | Trade-off                                                              |
| - | --------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| A | `VB_CACHE_DIR=$WORKSPACE/.cache/vcli && export VB_CACHE_DIR`         | Pure env override; remember to set on every shell.                     |
| B | Symlink the vcli cache root into the workspace                        | Affects all vcli invocations on the host.                              |
| C | Promote sandbox to `danger-full-access` (DSH only)                    | Heavy-weight; triggers per-session approval.                           |
| D | Treat stdout and exit code as ground truth; ignore the cache          | Loss of forensic history; acceptable for short debug sessions.         |

### Rule: ramic_bridge.il is loaded once per CIW window

**Pattern** — two or more sessions registered under the same user,
even though you only opened "your" CIW.

**Why** — `ramic_bridge.il` auto-spawns a bridge daemon every time CIW
`load`s the script. Two CIWs means two daemons means two registry entries.
The registry file list and the actual `list_alive()` set can differ when
one daemon dies.

**Recovery**

- Disambiguate: `VB_SESSION=<id>` or `--session <id>`.
- Reap orphans: `vcli session cleanup --format json` (drops registry
  files for ports that no longer have a listener).
- Identify which one is yours: `vcli session show <id>` plus
  `host`/`created` together pin the offender.

### Rule: the RPC layer is split into two pools by the Admin gate

**Pattern** — `vcli rpc call` either succeeds without admin or rejects
the call with `missing required capability`, depending on the method.

**Why** — the RPC namespace is not a uniform bypass of the whitelist.
The binary places every method in one of two pools:

| Pool                              | Examples                                                              | Admin required? |
| --------------------------------- | --------------------------------------------------------------------- | --------------- |
| **Typed / whitelisted RPC**       | `cell.*`, `library.*`, `util.*`, `maestro.*` (reads), `window.*`, `schematic.*`, `symbol.*`, `tx.*`, `file.*`, `sim.*` | **No** |
| **Raw SKILL RPC**                 | `skill.exec`, `skill.eval` (any future `skill.<verb>` resolving to `evalstring`) | **Yes** |

**Recovery**

- Read-only / discovery agents: use any first-pool RPC. They do not
  require Admin.
- Arbitrary SKILL expressions: there is no escape from
  `VCLI_CAPABILITY=admin`, whether you go through `vcli skill exec`
  or `rpc call --method skill.exec`. Pick the form that's easier to
  grep later:
  - `vcli skill exec '...'`
  - `rpc call --method skill.exec --params '{"code":"..."}'`
  - `rpc call --method skill.eval --params '{"code":"..."}'`
    (multi-statement).

Neither posture is "the bug". First pool is the intended production
route for agents; second is a deliberate escape hatch demanding operator
consent.

### Rule: RPC methods are snake_case with a mandatory subdomain prefix

**Pattern** — `unknown <subdomain> method 'X'` even though `X` is the
verb you want.

**Why** — RPC names are not arbitrary. They follow `<subdomain>.<verb>`
in `snake_case`: `maestro.get_analyses`, `schematic.open_cell_view`,
`library.list`. The subdomain prefix is mandatory — bare verbs are rejected.

**Recovery**

- One-shot dump: `vcli rpc schema --format json`. There is no
  `--method <name>` filter — only the full dump.
- Pin the spelling across binary upgrades; the schema is stable until
  a release notes file announces otherwise.

### Rule: Maestro writes are split between UI bookkeeping and the simulator

**Pattern** — a `vcli maestro set-var` (or `rpc maestro.set_var`) call
returns success but a later `grep <var> input.scs` shows the old value.

**Why** — there are two design-variable namespaces under the ADE UI:

| Namespace                | Owner API                  | Affects netlist? |
| ------------------------ | -------------------------- | ---------------- |
| Maestro internal varList | `maeSetVar`                | No               |
| Ocean / Spectre input    | `asiSetDesignVarList`      | **Yes**          |

`maestro.set_var` resolves to the first row. The variable the
simulator consumes lives in the second row, owned by `asiSetDesignVarList`
plus a subsequent `maeSaveSetup` to flush.

**Recovery**

Use the asi path explicitly via `skill.eval` (Admin):

```
let((sess vl)
  sess=asiGetCurrentSession()
  vl=asiGetDesignVarList(sess)
  vl=cons(list("<var>" "<new>") remove(assoc("<var>" vl) vl))
  asiSetDesignVarList(sess vl)
  maeSaveSetup(?session sess~>name))
```

`~>name` extracts the session-name string from the handle (see next
rule). `maeSaveSetup` without `~>name` fails on IC25 (see Rule "IC25
SKILL keyword type-template").

**Ground-truth check** — after the above, the variable appears in the
generated `input.scs` as `parameters ... <var>=<new> ...`. Read that
file directly (on the host where the run completed) to confirm.

### Rule: IC25 SKILL keyword arguments are strictly string-quoted

**Pattern** — SKILL calls of the form
`<fname>(?keyword <var-bound-handle>) ...` fail with
`*Error* <fname>: argument for keyword ?<name> should be a string
(type template = "...")`.

**Why** — IC23 SKILL accepted handles transparently. IC25 added a
type-template guard on `?session` and other keyword args that
requires a literal-or-extracted string. The exact `type template =`
error code names how many placeholders are mismatched (a useful
self-check but not stable across versions).

**Recovery** — extract the string explicitly:

```
<fname>(?keyword <handle>~>name)
```

Where `~>name` is the conventional accessor that returns the
underlying name string. Any SKILL API that has a session-handle form
plus a name accessor is subject to this on IC25.

### Rule: `mae*` reads with implicit session resolution fail in ADE Editing mode

**Pattern** — calls like `get_result_tests`, `get_history_list`, or
anything that internally `setq`s an `asiSession` return either an
empty list or `*Error* setq/set: Variable is protected and cannot be
assigned to`.

**Why** — ADE Editing mode marks `asiSession` as read-only. Functions
that try to install an implicit session binding at the start of a read
are blocked. This is a Cadence-side policy, not a vcli policy.

**Recovery** — bypass the wrapper and call the underlying API
directly via `skill.eval` (Admin), or pass the session explicitly:

- `maeOpenResults(?history "ExplorerRun.<idx>")` — works with `?history`
  because it does not need `asiSession` resolution.
- Direct filesystem walk of the results-tree directory:
  `.../maestro/results/maestro/ExplorerRun.<n>/1/<cell>_sim/psf/`.
- `maeGetAllExplorerHistoryNames(<session>)` — accepts a session
  string explicitly and returns the explorer-attached history list
  (which excludes bypass-mode runs).

### Rule: bypass-mode `maestro.run` writes results to disk but not the Explorer history list

**Pattern** — `maestro.run` returns `{"status":"ok"}`, but
`get_history_list` does not show the new run; the run's data is in
`<results>/maestro/ExplorerRun.<idx>/...`.

**Why** — `maeRunSimulation` invoked without going through the
Explorer dropdown writes the results tree on disk directly. The
explorer-attached history list (`maeGetAllExplorerHistoryNames`) is a
GUI-side view of runs the user launched from Explorer; bypass-launched
runs are not registered there.

**Recovery**

1. Open results by direct name (bypasses Explorer):
   `rpc call --method maestro.open_results --params '{"history":"ExplorerRun.<idx>"}'`
2. Inspect the `<cell>_sim/psf/` dir on disk to confirm PSF data
   (`psf`, `wavedb`, `dsWaveforms`, `mappingFile.*`, `exprOutputs.log.*`).

### Rule: IC25 PSF is compressed binary by default; raw waveforms are not greppable

**Pattern** — you inspect `<results>/.../psf/tran.tran.tran` or `ac.ac`
on disk and see `<BINPSF creation time>`, a list of `<ENV_VAR_0..N>`
index markers, and a final group block listing signal names
(`VSS`, `VDD`, `VIN+`, `VOUT`, `Iref_in`, `V0:p`, `V2:p`, ...). The
file is `type = data` per `file(1)`. There are no `<time> <value>`
rows to grep.

**Why** — IC25's default PSF format writes binary blocks (signaled by
`*cdnshcompressiontype 26 1` in the `.sig` file). The header text
gibberish at the top is ASCII because the index is human-readable,
but the data section is compressed. spectre would have to be invoked
with `-raw ./psf` and ASCII format headers to produce grep-friendly
output, and the Maestro-driven runs don't pass that flag.

**Recoveries**

- Read via Ocean / `srrWave` API:
  ```
  VCLI_CAPABILITY=admin vcli rpc call --method skill.eval --params '{
    "code": "let((d) d=getData(\"/VOUT\" ?result \"tran-tran\") printf(\"type=%L\\n\" type(d)))"
  }'
  ```
  Confirms the data is loaded and identifies it as `srrWave`.
  Numerical access requires the srrWave API rather than
  `value(getData(...))`.
- Re-run spectre manually with ASCII PSF if the wrapper can find a
  `modelFiles`-resolved netlist copy on the vcli host:
  `spectre -format psfascii -raw ./psf <netlist>` produces text files
  in `psf/` per the `spectre-netlist-gotchas` skill.
- Accept the loss; treat PSF signal presence as ground truth and rely
  on downstream Ocean/Virtuoso plotting instead of harness-side numerics.
- Write a typed `rpc call --method maestro.get_output_value` to read
  scalar values, but note that in `ADE Editing` mode it returns `nil`
  because the underlying `maeGetResult*` family tries to install a
  default `asiSession` binding that Editing mode rejects. Use
  `run-async`/`sim measure` instead, or run Ocean from a separate
  session.

### Rule: only sessions opened by this process can be closed programmatically

**Pattern** — `maestro.close_session` or `maeCloseSession` returns
`*Error* maeCloseSession: failed to close session 'X' because it has
been opened from the Virtuoso user interface. maeCloseSession can be
used to close only those sessions that were opened using maeOpenSetup
in the SKILL code`.

**Why** — Cadence deliberately prevents programmatic SKILL from
closing a session the user opened through the GUI. This is a
safety / consistency guarantee for the active ADE Explorer GUI state.

**Recoveries**

- Do not automate closing GUI-launched sessions. Ask the user to
  close them from the Explorer window, or call `maeOpenSetup` from
  your own code if you also want to be able to close them.
- For sessions your code opened (`maeOpenSetup` / `rpc call
  --method maestro.open_session`), normal `maeCloseSession` works.
  This applies to `spectre0`-style simulator-side sessions opened
  by `maestro.run`, where `maeCloseSession ?session <name>` does
  succeed even though the ADE-side `fnxSession<n>` may not.

### Rule: IC25 `?session` keyword rejection with `~>name` is not enough

**Pattern** — `maeSaveSetup(?session sess~>name)` still errors with
`argument for keyword ?session should be a string (type template = ...)`
even though `sess~>name` returns a string. `*Error*` includes the
session name in its argument list, indicating the slot was populated
with a string but the *template* check still failed.

**Why** — IC25's type-template check on `?session` of `maeSaveSetup`
is stricter than IC23, and accepts only a literal `?session "<name>"`
form. `~>name` from a variable-bound handle is not equivalent; the
template guard sees the value chain through `let((sess) ...)` and
does not unwind it to a literal.

**Recoveries**

- Pass the literal name string explicitly:
  `maeSaveSetup(?session "<session-name-from-window.list>")`.
- If you need this from a variable, fetch the name into a literal
  in your SKILL before calling: `(let((s) s=asiGetCurrentSession()
  maeSaveSetup(?session s~>name)))` also fails; use the literal-name
  form unconditionally.
- This is one of the few `mae*` APIs where the only safe value is
  the literal session name read from `window.list` output. Keep that
  pipeline handy.


### Rule: `maestro.create_corner_netlist` requires `VB_REMOTE_HOST` plus an ssh-capable target

**Pattern A** — config error: `VB_REMOTE_HOST is required for vcli maestro export-netlist`.

**Why A** — the RPC downloads a netlist from the Virtuoso server to
the vcli client via ssh scp. The binary defaults `VB_REMOTE_HOST` to
localhost, which is wrong in any multi-host setup.

**Recovery A** — set `VB_REMOTE_HOST`, `VB_REMOTE_PORT`, `VB_SSH_USER`
in `.env` or the active shell. The `output_dir` parameter must be a
non-existent path — the RPC uses `atomic_publish_no_replace` and
refuses to overwrite an existing directory.

**Pattern B** — ssh error: `mkdir remote dir failed: ... /home/<user>/.ssh/known_hosts` or
unix listener `Permission denied`.

**Why B** — two stacked locks:

1. The remote sshd may be configured with `ForceCommand internal-sftp`
   plus `ChrootDirectory <root>`. Probe with
   `ssh user@host 'echo ok'`; if the response is "allows sftp connections
   only", shell exec is denied. Only sftp operations against a
   chroot are allowed, and the chroot typically excludes the user's
   home and the project tree. vcli's `create_corner_netlist` requires
   remote shell exec; it cannot work on an sftp-only host.
2. DSH sandbox mode `workspace-write` blocks vcli's local
   ControlMaster socket at `~/.cache/virtuoso_bridge/ssh/<host>-<port>-<user>.<rand>`
   and appends to `~/.ssh/known_hosts`. Both paths sit outside the
   workspace allow set.

**Recoveries B** — pick one:

| Path                                              | Trade-off                                          |
| ------------------------------------------------- | -------------------------------------------------- |
| Use a remote host whose sshd permits shell exec   | Requires operator access to sshd_config            |
| Run `spectre` directly against a co-located netlist | Bypasses vcli ssh entirely; see `spectre-netlist-gotchas` |
| Promote DSH session to `danger-full-access`       | Per-session user approval; uses workspace-allowed alternatives |
| Verify netlist content by direct file read on the vcli host | Requires that vcli and the compute host share a file view, or that vcli runs on the compute host. |

### Rule: name a Cadence-version before you cite a SKILL signature

**Pattern** — a SKILL function that worked on one Cadence version
rejects the same arguments on another. The `type template = "..."`
error codes change between releases.

**Why** — Cadence SKILL is version-typed at the keyword-arg level.
The same function name can have stricter type checks between IC23,
IC23.1, IC24, IC25, etc. The `?options` alist format on `maeSetAnalysis`
and the `?session` keyword on `maeSaveSetup` are two known surfaces
where IC23-only code breaks on IC25.

**Recovery** — never write a SKILL fragment without naming the
intended Cadence version. The skill `ocean-netlist-regen` carries
the per-version deltas as of the last validation pass.

## Maintenance contract — keep the playbook honest

The rules above are **observations pinned against binary `1.0.0` and
Cadence IC25** as of the last validation pass. They are not frozen.
Three forces will erode them:

1. **vcli binary upgrades.** A new binary version may add, rename, or
   remove RPC methods, change error strings, or shift behavior of
   `maestro.set_var`, `maeGetSetup`, etc. Re-dump `vcli rpc schema
   --format json` after every binary upgrade and re-read the rules
   whose patterns include version-specific strings or method lists.
2. **Cadence version upgrades.** IC26+ will change SKILL signatures,
   type-template guard wording, and PSF format. Treat any new
   *Cadence-version* (IC24, IC26, ...) as a new working assumption.
3. **Host / wrapper / sandbox policy changes.** `vcli.sh` resolution
   order, DSH sandbox default mode (`workspace-write` vs
   `danger-full-access`), and `<root>` sshd ChrootDirectory
   configuration are all host/runtime-side and can shift without
   notice.

### How to evolve a rule

You never *delete* a rule; you **supersede** it. The lifecycle is:

1. Observe a new pattern in a live session that contradicts (or
   refines) an existing rule.
2. Verify it across at least one more session, host, or PDK before
   editing convention.md — single-session observations are special
   cases, not rules.
3. Edit convention.md in place. The rule that previously held may
   stay (it was true at a point in time) but mark the change:
   - Add the new rule below the old one.
   - If the old rule is *wrong* (not just incomplete), update its
     `**Recovery**` block to reflect the new fix and note the
     correction in your commit message.
4. If the change touches the wrapper (`vcli.sh`) or directory layout
   (new files in `_shared/`), the rest of the surface is
   version-controlled alongside it — there is no separate "playbook
   release" process.

### When to *not* edit convention.md

- Single-encounter traces. If the rule reads like a log entry
  ("on session X, port Y, library Z, this happened"), it is not a
  Rule yet. Wait for the second occurrence before promoting.
- vcli binary bug reports. They belong in the upstream issue
  tracker, not in the agent's diagnostic playbook. The playbook
  records behaviour, not bugs.
- Library/PDK-specific patterns. Move those to
  `.claude/skills/<pdk-name>/SKILL.md` so they ship with the
  library's documentation. convention.md is host-and-bridge
  focused.

## What this conversation produced

Closing summary of the current `main` HEAD state (post-validation):

- 14 playbook rules covering: cache-write failures, multi-session
  auto-discovery, the RPC Admin gate, RPC naming conventions,
  Maestro's UI/simulator variable split, IC25 SKILL keyword
  type-templates, ADE Editing mode read restrictions,
  bypass-launched runs, PSF binary format, programmatic session
  close restrictions, IC25 `?session` rejection of `~>name`,
  `create_corner_netlist` ssh requirements, and Cadence-version
  style pinning.
- A bash wrapper (`_shared/vcli.sh`) that resolves the `virtuoso`
  vs `vcli` naming split without touching any SKILL.md.
- A README.md and convention.md structure that keeps the
  rules/rename-contract/diagnostic-playbook in the same directory
  for one-stop retrieval.

Anyone returning to this repo on a fresh agent host should run
`virtuoso doctor` first, then read the playbook top-down before
issuing any SKILL or RPC call. If they hit a new pattern, the
lifecycle above applies.

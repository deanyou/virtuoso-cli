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

## Diagnostic playbook — when things look broken but aren't

The wrapper is a thin `exec` shim. When the harness-side observable behaviour
looks wrong, walk through this checklist before touching code. Each entry
records a real failure mode that was misdiagnosed at least once during
integration.

### "session history" is empty, but my commands did execute

**What you see**

```bash
$ virtuoso --session $ID session history $ID --cmd --format json
{"cmd": [], "cmd_count": 0, ...}
```

You also notice `~/.cache/virtuoso_bridge/history/$ID.jsonl` does not exist
and `cmd.jsonl` mtime hasn't moved since yesterday.

**What this actually means**

The binary did run, but **its `history::append_cmd` write was silently dropped**
because the host shell process could not open files under
`~/.cache/virtuoso_bridge/history/` for writing. vcli's writers use
`fs::OpenOptions::new().create(true).append(true).open(path)` and
**discard the error** (`src/history.rs:36–44, :60–73`). No stderr noise,
no exit code change — the command appears to "succeed" without persisting.

This is commonly a **host filesystem permission** issue, not a vcli bug:

- **DSH sandbox = `workspace-write`** (the default). Writing anywhere outside
  the session workspace — including `~/.cache/`, `/tmp/`, `/var/log/` —
  returns `EACCES`. The wrapper resolves `VB_LOG_DIR` to `~/.cache/...` which
  is **outside the sandbox-allowed set**.
- **Multi-user hosts** where another user `chown -R`'d the cache dir before
  you.
- **Read-only mounts**: some EDA farms mount the home cache as read-only when
  a previous job crashed mid-write.

**Fast test**

```bash
echo "probe" > ~/.cache/virtuoso_bridge/history/__probe.txt && echo OK
# expected:  OK
# actual:    bash: ...: Permission denied
```

If you see `Permission denied`, the fix is *not* in vcli.

**Fixes (pick one)**

| # | Action                                                                | Trade-off                                                              |
| - | --------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| A | Set `VB_CACHE_DIR=$WORKSPACE/.cache/vcli` and re-run                  | Pure env override; history now lives inside the sandbox workspace.    |
| B | Symlink `~/.cache/virtuoso_bridge → $WORKSPACE/.cache/virtuoso_bridge` | Changes the global cache path for *all* vcli invocations on the host. |
| C | Promote the sandbox to `danger-full-access` (DSH-only)                 | Triggers user approval per session; heavy-weight.                      |
| D | Accept the loss                                                       | Use `doctor` and live `vcli` stdout as the source of truth in-session. |

If you go with A, remember `VB_CACHE_DIR` must be set on **every** invocation
because `doctor` does not read it implicitly — the next agent that runs a
wrapper command without the override will revert to the unsandboxable default.

### Multi-session appears even though I only opened one

**What you see**

```bash
$ virtuoso skill exec '1+1'
error: multiple Virtuoso sessions active: dean-A, dean-B
```

**What this actually means**

`ramic_bridge.il` auto-spawns a daemon **every time CIW `load`s the script**.
If another human (or another agent) loaded it in a separate CIW window, you
now have two sessions registered. `list_alive()` correctly de-dups by TCP
port reachability, but `session list` only filters by registry, not by port.

**Fixes**

- Disambiguate: `VB_SESSION=<specific-id>` or `--session <id>`.
- Reap the orphan: `virtuoso session cleanup --format json` (removes registry
  files for ports that no longer have a listener).
- Identify which one is yours: `virtuoso session show <id>` — `host` and
  `created` together usually pin the offender.

### Raw SKILL exec is rejected with "use 'vcli rpc call' instead"

**What you see**

```bash
$ virtuoso skill exec '1+1'
error: raw SKILL exec is not permitted: use 'vcli rpc call' instead
```

**What this actually means**

In binary `1.0.0`, the `whitelist` policy in `src/client/bridge.rs:83–84`
blocks `evalstring` for any caller that does not hold the **Admin**
capability. Admin is toggled via `VCLI_CAPABILITY=admin`.

**Subtlety (2026-08-14 validation)**: the RPC layer is not a uniform bypass.
The RPC namespace is split into two pools:

| Pool                              | Examples                                                              | Admin required? |
| --------------------------------- | --------------------------------------------------------------------- | --------------- |
| **Typed / whitelisted RPC**       | `cell.info`, `library.list`, `util.ping`, `util.version`, `maestro.*` (read methods), `window.list`, `schematic.*` (reads), `symbol.*`, `tx.*`, `file.*`, `sim.check_license` | **No** |
| **Raw SKILL RPC**                 | `skill.exec`, `skill.eval` (and any future `skill.<verb>` that resolves to evalstring) | **Yes** — same gate as `vcli skill exec`; error reads `"method 'skill.exec' not permitted: missing required capability"` |

So the agent route depends on what you're trying to do:

- **Discovery / read-only work**: call any RPC method from the first pool.
  These run under the typed whitelist and do not require Admin. Example:
  `virtuoso rpc call --method cell.info --params '{}'`.
- **Anything that needs an arbitrary SKILL expression** (custom tooling,
  one-off diagnostics, debugging) — there is no escaping `VCLI_CAPABILITY=admin`,
  whether you go through `vcli skill exec` or `rpc call --method skill.exec`.
  Pick the path that's easier to grep later:
  - `VCLI_CAPABILITY=admin virtuoso skill exec '...'`
  - `VCLI_CAPABILITY=admin virtuoso rpc call --method skill.exec --params '{"code":"..."}'`
  - `VCLI_CAPABILITY=admin virtuoso rpc call --method skill.eval --params '{"code":"..."}'`
    (this last one supports multi-statement SKILL)

Neither posture is "the bug". The first is the intended read-only
production route; the second is a deliberate escape hatch that demands
explicit operator consent. Pick by role, not by accident.

If the agent ever needs to call `skill.exec` repeatedly across many turns,
export `VCLI_CAPABILITY=admin` once in the shell session rather than
prefixing every invocation — same effect, less noise.

### RPC method name returned `unknown <domain> method`

**What you see**

```bash
$ virtuoso rpc call --method maestro.getAnalyses --params '{}'
unknown maestro method 'getAnalyses'
```

**What this actually means**

RPC method names are **`snake_case`**, not `camelCase`. Run
`virtuoso rpc schema --format json` once and pin the exact spelling; the
schema currently lists 26 `maestro.*` methods, all of the form
`maestro.get_analyses`, `maestro.list_sessions`, etc.

This is a stable API — once a method lands in `rpc schema` it does not get
renamed silently. Re-check the schema on every binary upgrade.

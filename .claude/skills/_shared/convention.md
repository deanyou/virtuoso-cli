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

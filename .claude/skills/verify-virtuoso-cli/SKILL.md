---
name: verify-virtuoso-cli
description: "Behavior verification for vcli, the Rust CLI bridge that drives Cadence Virtuoso over SKILL, X11 and Spectre. Use for /verify-virtuoso-cli, 'prove vcli still works', 'verify the Virtuoso bridge', 'did the change break vcli', or when a change to vcli must be shown to still drive a real Virtuoso instance rather than merely compile."
---

# Verify virtuoso-cli (vcli)

`vcli` is a Rust CLI that controls Cadence Virtuoso from anywhere: a TCP bridge
runs inside Virtuoso (loaded from `resources/ramic_bridge.il`), `vcli` talks to it
over a direct socket or an SSH tunnel, and a second channel drives the Virtuoso GUI
over X11.

This skill proves **behavior**, not compilation. `cargo build` passing says the
binary links. It says nothing about whether `vcli init` writes a config the rest of
the tool will read, whether `vcli rpc call` reaches a live Virtuoso, or whether the
exit codes still mean what CI assumes they mean.

**Read `features/README.md` first.** It is the maintained map of user-facing
features and their recipes. This file is the operating procedure; the map is the
checklist.

## The two tiers, and why the difference matters

Virtuoso is commercial software with a license, an install, and a GUI. A verification
host may or may not have any of that. The skill is therefore split in two, and you
must never let a tier-A result be reported as a tier-B result.

| Tier | What it covers | Needs |
|------|----------------|-------|
| **A — client-side** | build, version, config bootstrap, schema/RPC introspection, session discovery, doctor, exit-code contract, command-history side effect, GUI scenario *replay* | Rust toolchain only |
| **B — bridge** | `vcli rpc call` against a live Virtuoso, `vcli cell` / `vcli sim` / `vcli maestro`, `vcli window action-x11`, GUI scenario `--executor live` | a running Virtuoso with `ramic_bridge.il` loaded in the CIW, a reachable TCP port, and a valid license |

Tier A is fully provable on any machine with Rust. Tier B needs the real target. If
you only have tier A, say so — a tier-B feature you could not reach is
`verified-unreachable` with its prerequisite named, never a pass and never a
quiet omission.

## 1. Launch

`vcli` is a short-lived CLI. There is no client-side server to keep alive: **launch
means build the binary once, then start each drive in its own isolated invocation.**

```bash
export PATH="$HOME/.cargo/bin:$PATH"     # rustup is not on the default PATH here
cd <repo-root>                            # the checkout that contains this skill
cargo build                              # builds vcli and vtui
```

Ready means usable, not merely present:

```bash
./target/debug/vcli --version
```

Ready = the string `vcli <semver>` on stdout and exit code `0`. If the build fails,
**report the exact compiler error and stop** — a skill written against a broken base
teaches wrong steps. Do not edit `src/` to make a verification run go green; a
product defect is a finding, not something to patch in place.

### Isolation

Every run gets its own data directory. `VB_HOME` relocates *all* of vcli's runtime
state at once — config, cache, logs, state, artifacts and tmp
(`src/runtime_paths.rs`):

```bash
export VB_HOME="$(mktemp -d /tmp/vcli-verify-XXXXXX)"
```

Per-directory overrides if you need finer grain: `VB_CONFIG_DIR`, `VB_CACHE_DIR`,
`VB_LOG_DIR`, `VB_STATE_DIR`, `VB_OUTPUT_DIR`, `VB_TMP_DIR`. The evidence-loop
database has its own: `VB_EXPERIENCE_DB`.

Never point `VB_HOME` at a real user's home, and never set it to a fixed path that
two concurrent runs would share.

### The bridge daemon is not yours to start

The daemon `vcli` talks to lives **inside Virtuoso**, not on this host. On a target
that already has it running, use it. On a target that does not, `vcli tunnel start`
opens an SSH tunnel and deploys the daemon — that mutates the remote host, so only
do it when the run owns that host. `vcli tunnel stop` then tears down what
`vcli tunnel start` created; never run it against a tunnel you did not start.

### Secondary surface: the TUI

`vtui` is a ratatui dashboard and needs a PTY. Run it under tmux with its own
session name so cleanup can target it exactly:

```bash
tmux new-session -d -s vcli-verify -x 120 -y 40 "./target/debug/vtui"
tmux capture-pane -t vcli-verify -p          # read the rendered dashboard
tmux kill-session -t vcli-verify             # teardown
```

### Teardown

`rm -rf "$VB_HOME"` and `tmux kill-session -t vcli-verify`. Nothing else was
started. See [Cleanup](#5-cleanup).

## 2. Doctor

One read-only check that answers *"is this instance worth driving?"* Run it before
the first drive, on every fresh session, and again after any failed drive.

```bash
"${CLAUDE_SKILL_DIR}/scripts/doctor.sh"
```

`${CLAUDE_SKILL_DIR}` is this skill's own directory (`.claude/skills/verify-virtuoso-cli`).
The helper prints a JSON verdict and exits `0` when the host is drivable.

The four probes, and what each one buys you:

| Probe | Command | Read-only | Answers |
|-------|---------|-----------|---------|
| identity | `vcli --version` | yes | is this the right build? |
| config | `vcli config check` | yes | does the config resolve? any diagnostics? |
| sessions | `vcli session list` | yes | which bridge sessions exist, and is the tunnel up? |
| bridge | `vcli tunnel diagnose` | yes | is the daemon reachable and is SKILL alive? |

`vcli config check` reports every resolved key with the layer it came from
(`env` / `file` / `target` / `default`) plus a `digest` and a `diagnostics` array.
`vcli tunnel diagnose` is the decisive one for tier B: it reports
`daemon_responsive`, `tcp_reachable`, `skill_eval_ok` and `virtuoso_version`.

**Verdict rules.**

- **Tier A drivable** — identity ok and `config check` exits `0`.
- **Tier B drivable** — additionally `tunnel diagnose` reports `daemon_responsive: true`
  *and* `skill_eval_ok: true`. `tcp_reachable: true` with `daemon_responsive: false`
  means something is listening but it is not the bridge.

**Do not drive a tier-B feature when the doctor says the bridge is down.** The
failure you would capture is your own missing license, not the app's behavior.

One nuance: in remote mode (`VB_REMOTE_HOST` set) `vcli session list` opens an SSH
connection to refresh the cached session list. It never mutates OA state. In local
mode it is a pure local read. `vcli tunnel diagnose` likewise only probes.

Probes write state only under `$VB_HOME`: the append-only command history
(`$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`), and — in remote mode — the
session cache that `vcli session list` refreshes from the remote host. `vcli
--version` writes nothing. The target Virtuoso is not modified.

## 3. Drive

The harness is the `vcli` binary itself, driven from a shell. No PTY is needed for
the JSON path; the TUI is the only surface that needs one.

Four rules, every time:

1. **Pass `--format json` explicitly** — `vcli --format json <command>`. The default
   is `table` in a TTY and `json` in a pipe, so an explicit flag keeps assertions
   stable no matter how the command was invoked.
2. **Always capture the exit code.** vcli has a semantic exit-code contract
   (`src/exit_codes.rs`) and it is part of the product's user-facing behavior:

   | Code | Meaning |
   |------|---------|
   | `0` | success |
   | `1` | general error |
   | `2` | usage error |
   | `3` | not found |
   | `4` | permission denied |
   | `5` | conflict |
   | `10` | dry-run ok |

3. **Drive the user path, not an internal.** Type the command a user would type.
   Do not call into Rust internals, do not use test-only endpoints, do not assert on
   a private function's return value.

4. **Capture both channels.** vcli writes **results to stdout and errors to
   stderr**. A failing command leaves stdout empty and puts the structured error
   JSON on stderr. An assertion that reads stdout only sees an empty string and
   passes vacuously — that is a false green, not a proof.

A drive is one isolated invocation:

```bash
out="$(./target/debug/vcli --format json cell open --lib demo --cell top --dry-run 2>err.txt)"; rc=$?
```

Use the shipped helper to capture this properly, including the side-effect diff:

```bash
"${CLAUDE_SKILL_DIR}/scripts/probe.sh" "$EVIDENCE_DIR" cell-open-dry-run -- \
    cell open --lib demo --cell top --dry-run
```

### Driving the GUI

GUI features go through the repo's scenario harness, not ad-hoc `xdotool`:

```bash
python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py run <scenario.json> \
    --output "$EVIDENCE_DIR/gui" --executor fake
```

| Executor | What it drives | Needs |
|----------|----------------|-------|
| `fake` | a deterministic offline replay of a scenario | nothing |
| `live` | real `vcli` over SSH against a live Virtuoso | tier B |
| `local` | `xdotool` against a local `DISPLAY` | X11 + xdotool |

`fake` proves that the scenario engine routes, executes, verifies and records
correctly. **It proves nothing about Virtuoso.** A `fake` pass is never a tier-B
pass.

## 4. Evidence

### Where it goes

```
<repo>/.verify-artifacts/<RUN_ID>/
├── doctor.json                  # the doctor verdict for this run
├── <feature>/<step>/            # one directory per feature step driven
│   ├── action.txt               # the exact command(s) issued
│   ├── stdout.txt               # raw stdout
│   ├── stdout.json              # stdout parsed as JSON (only when it is JSON)
│   ├── stderr.txt               # stderr
│   ├── exit-code.txt            # the semantic exit code
│   ├── side-effects.txt         # files / rows observed to change
│   └── vbhome-path.txt          # the disposable VB_HOME the probe used
└── manifest.json                # feature id -> artifact paths -> verdict
```

`RUN_ID` is yours to choose, e.g. `$(date +%Y%m%d-%H%M%S)`.

`.verify-artifacts/` is **git-ignored by default** — the repo-root `.gitignore`
lists it. Evidence carries hostnames, ports and ssh key paths, so it must never be
one `git add -A` away from a commit. If a run's evidence has to be archived, copy
it out of the repo explicitly; the ignored directory is not versioned and will not
travel with a branch.

**Evidence must not live under `$VB_HOME`.** Cleanup deletes `$VB_HOME`. Proof that
the cleanup ate is not proof. `.verify-artifacts/` sits in the repo working tree,
outside the disposable data dir, so it survives teardown — and after cleanup you
must confirm it did:

```bash
ls -R .verify-artifacts/$RUN_ID
```

### The proof standard

1. **Exercise the real user path.** The same command a user types, against the same
   target a user would use.
2. **Capture the action and the resulting state.** A command that exits `0` with no
   observable state change is not proof. Record what you did *and* what you observed
   afterwards.
3. **Verify side effects alongside the visible result.** For vcli the canonical
   side effect is the command history — `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`
   gains one JSON record per invocation, carrying `ts`, `cmd` and `exit_code`. If a
   command claims to have changed something, show the file that changed, the row that
   was inserted, or the log line that was appended.
4. **Mock only where a production boundary already isolates the external system.**
   For vcli the external system is Virtuoso itself. The `fake` executor is a
   *replay* of the harness, not a mock of Virtuoso — it must never be presented as
   evidence that Virtuoso responded.

### Dry-run honesty

`--dry-run` on vcli commands returns exit code `10` and a request plan, and it does
**not** contact the bridge. Verified by observation rather than by trusting the
name: `vcli cell open --lib demo --cell top --dry-run` returned its plan JSON with
no connection error, while the same command without `--dry-run` failed with
`{"error":"connection_failed", ...}`.

So a dry run proves **request shaping only** — that the CLI resolves arguments into
the right call. It never proves the bridge works, never proves Virtuoso is
reachable, and never proves the operation took effect. Never report a dry-run as a
bridge pass, and never use a dry-run to satisfy a feature whose proof requires an
observed side effect.

### Recording a failure honestly

If a feature cannot be reached, record it as `verified-unreachable` with the
concrete prerequisite and the route you attempted. Do not weaken the journey to
make it pass, and do not omit the feature from the manifest.

## 5. Cleanup

Tear down what this run created. Nothing else.

| What you started | How to stop it | Never do this |
|------------------|----------------|---------------|
| `$VB_HOME` | `rm -rf "$VB_HOME"` | `rm -rf ~/.vcli` or `~/.cache/virtuoso_bridge` |
| a tmux session | `tmux kill-session -t vcli-verify` | `pkill vtui` (kills other people's dashboard) |
| a tunnel | `vcli tunnel stop` — **only** if this run ran `vcli tunnel start` | stop a tunnel you found running |
| a background process | kill the recorded PID | kill by process name |

Never kill by process name. A verification run that leaves a stray listener on a
shared machine is worse than one that failed.

**Cleanup removes instances and scratch state, never the evidence.** The artifacts in
`.verify-artifacts/<RUN_ID>/` are the deliverable and must survive. After cleanup,
confirm they are still there — a cleanup that eats the proof fails the run.

```bash
rm -rf "$VB_HOME"
tmux kill-session -t vcli-verify 2>/dev/null || true
ls -R .verify-artifacts/$RUN_ID      # must still list every artifact
```

Run cleanup after **every** failed iteration too, so a broken attempt does not
strand a process or a port.

**One repo file is not yours to leave dirty.** A `fake` GUI scenario run writes to
the repo's tracked `.claude/skills/virtuoso-gui-debug/data/skill_db.sqlite3`. After
any GUI-scenario drive, check `git status` and restore that file so the verification
run leaves no tracked file modified.

## 6. Helpers

Both helpers live in this skill's `scripts/` directory, are executable, and take
no arguments you have to reverse-engineer.

### `scripts/doctor.sh`

Read-only health check. Runs the four probes, prints a JSON verdict, exits `0` when
the host is drivable.

```bash
"${CLAUDE_SKILL_DIR}/scripts/doctor.sh"                    # uses the repo it lives in
"${CLAUDE_SKILL_DIR}/scripts/doctor.sh" /path/to/repo      # or an explicit checkout
```

Verdict shape:

```json
{
  "tier_a": "drivable",
  "tier_b": "unreachable",
  "reason": "bridge daemon not reachable on port 65000",
  "probes": { "identity": "ok", "config": "ok", "sessions": "ok", "bridge": "not reachable" }
}
```

`tier_b` is `drivable` only when the bridge is responsive *and* SKILL is alive.
`tier_b: "unreachable"` is a normal, honest result on a host without Virtuoso — it
is not a failure of the tool under test.

### `scripts/probe.sh`

Runs one `vcli` command in an isolated data dir and captures the full proof set:
action, stdout, stderr, exit code, and a before/after diff of the data directory.

```bash
"${CLAUDE_SKILL_DIR}/scripts/probe.sh" "$EVIDENCE_DIR" <feature-slug> -- <vcli args...>
```

For example:

```bash
"${CLAUDE_SKILL_DIR}/scripts/probe.sh" "$EVIDENCE_DIR" config-bootstrap -- init
"${CLAUDE_SKILL_DIR}/scripts/probe.sh" "$EVIDENCE_DIR" cell-open-dry-run -- \
    cell open --lib demo --cell top --dry-run
```

It writes `<EVIDENCE_DIR>/<feature-slug>/{action.txt,stdout.txt,stdout.json,stderr.txt,exit-code.txt,side-effects.txt,vbhome-path.txt}`,
prints the exit code, and exits with vcli's own exit code so it composes in a
pipeline. It never touches the user's real home: it creates a throwaway `VB_HOME`
under `/tmp` unless `VB_HOME` is already exported.

## Maintenance

The feature map rots as the app changes. After `vcli` gains a command, changes an
exit code, or moves a path, re-audit this skill and the map under `features/`:
run the review workflow in `.claude/skills/pstack-vcli/` and then manually
re-verify every assertion in `features/*.md` against the current source under
`src/`.

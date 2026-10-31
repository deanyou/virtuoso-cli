# vcli verification map

This directory is the maintained source for verifying the user-facing behavior of
`vcli`, the Rust CLI bridge that drives Cadence Virtuoso. Read the index before
driving, then use the matching feature file as the recipe.

The operating procedure — how to build, how to health-check, where evidence goes,
how to tear down — lives in `../SKILL.md`. This map is the checklist of *what* to
prove.

## Baseline preconditions

- Build the binary from the repo checkout: `export PATH="$HOME/.cargo/bin:$PATH"`
  then `cargo build`. The binary lands at `target/debug/vcli`.
- Give every run its own data dir: `export VB_HOME="$(mktemp -d /tmp/vcli-verify-XXXXXX)"`.
  It relocates config, cache, logs, state, artifacts and tmp together.
- Run `scripts/doctor.sh` and record the verdict as `doctor.json` in the evidence
  dir. It must report `tier_a: "drivable"` before any tier-A feature is driven.
- Only drive a tier-B feature when the same doctor reports `tier_b: "drivable"`.
- Never drive a Virtuoso instance that this run did not start or was not given
  permission to use. One Virtuoso instance must not be double-driven: the bridge
  is a single TCP port per session and the operations mutate the OA database.

## The two tiers

| Tier | Meaning | Precondition |
|------|---------|--------------|
| **A** | client-side behavior of the CLI itself | Rust toolchain only |
| **B** | behavior that crosses the bridge into a live Virtuoso | a running Virtuoso with `resources/ramic_bridge.il` loaded in the CIW, a reachable TCP port, a valid license |

A tier-A pass is never evidence for a tier-B feature. When a tier-B precondition is
missing, the journey is reported as `verified-unreachable` with the concrete
prerequisite named and the route attempted — not skipped silently, and not weakened
until it passes.

## Driving conventions

- The harness is the `vcli` binary itself, driven from a shell. Pass
  `--format json` explicitly on every invocation.
- Capture the exit code every time. vcli's exit codes are a user-facing contract:
  `0` success, `1` general error, `2` usage error, `3` not found, `4` permission
  denied, `5` conflict, `10` dry-run ok.
- Drive the command a user would type. No internal setters, no test-only endpoints.
- Start every recipe from the baseline state unless its preconditions say otherwise.
- Restore seeded state after a mutation. Do not remove proof artifacts during
  cleanup.

## Proof and skip reporting

- Capture the user action *and* the resulting state, not only the final screen.
- CLI proof includes the command, stdout, stderr and the exit code.
- Mutation proof includes a read-only second view of the stored value.
- The canonical side effect in vcli is the append-only command history at
  `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl` — one JSON record per
  invocation with `ts`, `cmd` and `exit_code`. Show it, do not assert it.
- A `--dry-run` proves request shaping only. It never contacts the bridge and
  never proves a side effect took place. Never use one to satisfy a feature whose
  proof requires an observed side effect.
- The `fake` executor of the GUI scenario harness is a replay of the harness, not
  a mock of Virtuoso. A `fake` pass is never a tier-B pass.
- Record the feature id and the entry point used with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition.
- Do not report a skipped entry point as verified through a different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the
user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with <harness>` starts with `Preconditions:` and uses labeled
   bullets that pair each user action with an exact command and observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable handles,
required state, commands, and observable proof.

## Features

- [Bootstrap the connection config](./config-bootstrap.md) covers `vcli init`,
  the config file it writes, the conflict contract, and `vcli config check`.
  **Tier A.**
- [Introspect the agent-facing contract](./agent-introspection.md) covers
  `vcli schema --all`, `vcli rpc schema`, the MCP stdio server's tool list, and
  the version-drift between them. **Tier A.**
- [Find a target and decide whether to drive it](./session-discovery-and-doctor.md)
  covers `vcli session list`, `vcli tunnel status`, `vcli tunnel diagnose`, and the
  doctor verdict. **Tier A** for discovery and doctor; the bridge leg is **Tier B**.
- [Call the SKILL bridge over RPC](./skill-bridge-rpc.md) covers
  `vcli rpc call`, the request-shaping dry run, the MCP stdio equivalent, and the
  failure contract when the bridge is down. **Tier B** for the success path; the
  request-shaping leg is **Tier A**.
- [Replay a GUI debugging scenario](./gui-scenario-replay.md) covers
  `gui_runner.py` with the `fake`, `local` and `live` executors, the evidence
  files a run emits, and the intervention record -> mine -> adopt loop with its
  SQLite side effect. **Tier A** for the `fake` executor and the evidence loop;
  `live` is **Tier B**, `local` needs X11.

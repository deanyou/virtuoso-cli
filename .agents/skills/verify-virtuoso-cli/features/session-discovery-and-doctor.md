# Find a target and decide whether to drive it

Before touching a design, a user has to find the Virtuoso instance to talk to and
decide whether it is worth touching. `vcli session list` finds bridge sessions,
`vcli tunnel status` shows the tunnel and daemon state, and `vcli tunnel diagnose`
answers the question that matters: is the bridge actually usable?

## Sub-features

- `session-list` enumerates the locally known bridge sessions.
- `session-sync-status` reports whether the remote session list could be refreshed.
- `session-alive-filter` hides sessions that are known but not alive.
- `tunnel-status` shows tunnel, daemon and resolved-config state together.
- `tunnel-diagnose` probes the daemon and reports reachability and SKILL liveness.
- `doctor-verdict` folds the four probes into one drivable / not-drivable verdict.

## How to get to it (user POV)

- Run `vcli session list` to see which Virtuoso instances are reachable.
- Run `vcli tunnel status` to see whether the tunnel and daemon are up.
- Run `vcli tunnel diagnose` to find out whether the bridge responds.
- Run `scripts/doctor.sh` to get one verdict before driving anything.

## Driving it with vcli

Preconditions:

- The binary is built at `target/debug/vcli`.
- `VB_HOME` points at an empty directory for this run, so the session cache starts empty.
- For the bridge legs, a Virtuoso instance is running with `resources/ramic_bridge.il` loaded in its CIW and its port reachable.

- **List sessions.** Run `vcli --format json session list`. Exit code `0` and stdout has `count`, a `sessions` array, `status` and `sync_status`. On an empty data dir `count` is `0` and `sessions` is `[]`.
- **Show the sync status is a real field.** In the same output, `sync_status` is `"ok"` in local mode. In remote mode (`VB_REMOTE_HOST` set) it becomes `"failed"` when the remote list cannot be refreshed, and the cached list is still returned. A failed sync is reported in that field, not by the exit code.
- **Tunnel and daemon state.** Run `vcli --format json tunnel status`. Exit code `0` and stdout has `config` (with `port`, `remote_host`, `backend`, `config_digest`, `timeout` and `roles`), `daemon` (with `responsive` and a human-readable `detail`) and `tunnel` (with `running`).
- **The decisive probe.** Run `vcli --format json tunnel diagnose`. Exit code `0` and stdout has `daemon_responsive`, `tcp_reachable`, `skill_eval_ok`, `virtuoso_version`, `port`, `latency_ms` and a one-line `summary`.
- **Read the verdict, not the exit code.** With no bridge running, `tunnel diagnose` still exits `0`; the honest answer is `"summary": "not reachable"` with `daemon_responsive: false` and `skill_eval_ok: false`. Assert on those booleans.
- **Full doctor.** Run `scripts/doctor.sh`. Exit code `0` and stdout is a JSON verdict with `tier_a`, `tier_b`, `reason` and `probes`. `tier_a` is `"drivable"` when the binary runs and the config resolves. `tier_b` is `"drivable"` only when the bridge is responsive *and* SKILL is alive; otherwise it is `"unreachable"` with a `reason` naming the failing probe.
- **Side effect.** Read `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`. Each probe is recorded with its exit code. The probes write no other state.

## Gotchas

- `vcli tunnel diagnose` and `vcli tunnel status` exit `0` whether or not the bridge is up. A run that asserts on exit codes alone will "pass" against a dead bridge. Always assert on `daemon_responsive` and `skill_eval_ok`.
- `vcli session list` filters out sessions that are cached but not alive, unless `--target` is given. An empty list means "no live sessions", which is different from "no sessions known".
- In remote mode `session list` opens an SSH connection to refresh the cached list. It is still read-only with respect to the OA database, but it is a network operation — do not run it against a host the run does not own.
- `tunnel status` reports `config.port` even when nothing is listening on it. The default port is `65000`; `daemon.detail` says `port not reachable` when the port is closed. A configured port is not a reachable port.
- `tunnel status` and `tunnel diagnose` are different commands with different shapes. Do not swap them in an assertion.
- `tier_b: "unreachable"` is a normal result on a host without Virtuoso, not a failure of the tool under test. Record it and move on to the tier-A features.
- The doctor must run again after any failed drive, and on every fresh session. A bridge that was responsive can stop being responsive.

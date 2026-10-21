# Call the SKILL bridge over RPC

The bridge is how `vcli` actually reaches Virtuoso: a TCP daemon inside Virtuoso
exposes a typed RPC surface, and `vcli rpc call` is the sanctioned way to invoke
it. Raw SKILL evaluation is deliberately gated off. This feature is the core of
the product, and it is the one feature that cannot be fully proven without a live
Virtuoso.

## Sub-features

- `rpc-call` invokes a typed RPC method and returns its result.
- `skill-exec-gated` refuses raw SKILL evaluation and points at the RPC path.
- `rpc-dry-run` shapes the request without contacting the bridge.
- `rpc-failure-contract` returns a structured error with a retry hint.
- `rpc-usage-error` rejects a malformed call with a usage exit code.
- `mcp-equivalent` reaches the same bridge over the MCP stdio transport.

## How to get to it (user POV)

- Run `vcli rpc call --method <name> --params '<json>'` in a terminal.
- Run `vcli cell open --lib <lib> --cell <cell> --dry-run` to preview a request.
- Connect to `vcli mcp serve` over stdio and call a tool.

## Driving it with vcli

Preconditions:

- The binary is built at `target/debug/vcli`.
- `VB_HOME` points at an empty directory for this run.
- For the success path: a Virtuoso instance running with `resources/ramic_bridge.il` loaded in its CIW, its port reachable, and a valid license. `scripts/doctor.sh` must report `tier_b: "drivable"`.

- **Raw SKILL is refused.** Run `vcli --format json skill exec "1+1"`. Exit code `1`, stdout is empty, and **stderr** carries `"error": "execution_failed"` with a message reading `raw SKILL exec is not permitted: use 'vcli rpc call' instead`. This is the gate, and it fires before any network activity.
- **Shape a request without the bridge.** Run `vcli --format json cell open --lib demo --cell top --dry-run`. Exit code `10` (`dry-run ok`) and stdout has `"action": "open"`, `"dry_run": true`, `"resource": "cell"` and a `target` object carrying `lib`, `cell`, `view` and `mode`.
- **Show that the dry run really skipped the bridge.** Run the same command without `--dry-run`: `vcli --format json cell open --lib demo --cell top`. Exit code `1`, stdout is empty, and **stderr** carries `"error": "connection_failed"` with a `message` containing `Connection refused`, a `suggestion` naming `virtuoso tunnel start`, and `"retryable": true`. The dry run produced a plan with no connection error while the live command tried and failed to connect — that contrast is the proof that `--dry-run` does not touch the bridge.
- **Real call.** Run `vcli --format json rpc call --method "schematic.open_cell_view" --params '{"lib":"demo","cell":"top"}'`. Exit code `0` and the cellview `demo/top` exists in the OA library afterwards. Confirm it from a second, read-only view.
- **Usage error.** Run `vcli --format json rpc call --method "schematic.open_cell_view" --params 'not-json'`. Exit code `1`, stdout is empty, and **stderr** carries `"error": "json_error"` and a `message` describing the parse failure. Pass the method and params as named flags — `--method` and `--params` — not positionally; a positional method is rejected as a usage error.
- **MCP equivalent.** Drive `vcli mcp serve` over stdio with a `tools/call` request naming a bridge tool. The response carries the same structured result the RPC path returns, which is the proof that both transports reach the same bridge.
- **Side effect.** Read `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`. Each call is recorded with its exit code — the dry run and the failed live call appear as separate records with `exit_code` `10` and `1`.

## Gotchas

- `vcli skill exec` is gated off by design. Use `vcli rpc call`. A verification run that tries `skill exec` and reports its refusal as a product failure has misread the gate.
- `vcli rpc call` takes `--method` and `--params` as named flags. `vcli rpc call skill.eval --params '{...}'` is a usage error (exit `2`).
- `--params` takes one JSON object as a single argument. Malformed JSON is a `json_error` with exit `1`, not a usage error.
- A `--dry-run` proves argument shaping only. It never contacts the bridge, never reaches Virtuoso, and never proves a side effect. Never use one to satisfy this feature's success path.
- `connection_failed` carries `"retryable": true` and a `suggestion`. It is the honest "no bridge" answer, not a defect. Report it as such.
- Dangerous SKILL patterns are blocked client-side, before the request is serialized. A blocked pattern is a refusal, not a transport error, and it is reported differently from `connection_failed`.
- With more than one bridge session and no `--session`, `vcli` refuses to guess and exits with an error. Pass `--session <id>` explicitly when several sessions exist.
- The success path is the only tier-B leg in this feature. On a host without Virtuoso, report it as `verified-unreachable` with the missing prerequisite named — the license, the CIW load, or the reachable port.

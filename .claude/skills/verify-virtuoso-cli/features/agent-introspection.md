# Introspect the agent-facing contract

`vcli` is built for agents to drive. Three surfaces let an agent discover what it
can do without reading the source: the CLI command schema, the typed RPC method
list, and the MCP stdio server's tool list. Proving them means proving the
machine-readable contract an agent parses, not just that the commands exist.

## Sub-features

- `schema-all` returns a JSON command tree with typed flags.
- `schema-noun-verb` returns one command or one subcommand.
- `schema-unknown` reports an unknown command as `not_found`.
- `schema-exit-codes` publishes the exit-code contract in the schema itself.
- `rpc-schema` returns the typed RPC method catalogue with parameter specs.
- `mcp-tools` serves the tool list over the MCP stdio transport.
- `surface-version-consistency` records the version each surface reports.

## How to get to it (user POV)

- Run `vcli schema --all` to get the whole command tree.
- Run `vcli schema tunnel start` to get one command's flags and examples.
- Run `vcli rpc schema` to get the RPC method catalogue.
- Connect to `vcli mcp serve` over stdio and send a `tools/list` request.

## Driving it with vcli

Preconditions:

- The binary is built at `target/debug/vcli`.
- `VB_HOME` points at an empty directory for this run.

- **Whole command tree.** Run `vcli --format json schema --all`. Exit code `0` and stdout is an object with `name`, `version`, `commands`, `global_flags` and `exit_codes`. Under `commands`, every entry has a `description`; commands with verbs carry a `subcommands` map; every flag carries `type`, `default` and `description`, and enum flags carry a `values` array.
- **One command.** Run `vcli --format json schema tunnel start`. Exit code `0` and stdout has a single key `"tunnel start"` whose value holds that subcommand's `flags` and `examples`.
- **Unknown command.** Run `vcli --format json schema nosuch`. Exit code `0` and stdout has `"error": "not_found"` and a `message` reading `unknown command: nosuch`. The command is *reported* as missing but the query itself succeeds, so assert on the `error` field rather than the exit code.
- **Exit-code contract.** In the `--all` output, the `exit_codes` object maps `"0"`, `"1"`, `"2"`, `"3"`, `"4"`, `"5"` and `"10"` to their meanings. Cross-check those against the codes observed in the other features: a conflict must be `5`, a not-found must be `3`, a dry run must be `10`.
- **RPC catalogue.** Run `vcli --format json rpc schema`. Exit code `0` and stdout has a `methods` array. Each method carries a `name`, a `summary`, a `returns` string and a `params` array whose entries carry `name`, `ptype`, `required` and `description`.
- **MCP tool list.** Drive the stdio server with a JSON-RPC handshake. Write three JSON lines to a file — `initialize`, `notifications/initialized` and `tools/list` — and pipe them in: `vcli --format json mcp serve < requests.jsonl > responses.jsonl`. Every request line needs a `params` member, including the notification; a line without it is answered with a `-32700` parse error. The `tools/list` result carries a `tools` array whose entries have `name` and `description`.
- **Version per surface.** Run `vcli --version` and read the `version` field of `schema --all`. Then read `serverInfo.version` from the MCP `initialize` response. Record all three values. They are **not** guaranteed to agree — the MCP server reports its own version literal, which is not derived from the crate version, so a run that only checks one of them will miss the drift.
- **Side effect.** Read `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`. Each schema query is recorded there with its exit code.

## Gotchas

- `schema --all` is a **curated subset**, not the whole CLI surface. It describes `init`, `tunnel`, `skill`, `cell` and `schema` — a fraction of the nouns listed by `vcli --help`. Never use it as proof of which commands exist; use `vcli --help`. When a new noun is added to the CLI, this tree does not grow by itself, and that is a finding to report, not something to patch during a verification run.
- `schema --all` and `schema <noun> <verb>` both exit `0` even when the noun is unknown. The `error` field carries the verdict.
- The `version` in `schema --all` comes from the crate version, so it tracks `vcli --version`. The MCP `serverInfo.version` does not.
- The MCP stdio server is strict JSON: a request line without `params` gets a `-32700` parse error rather than being ignored, and the server keeps running afterwards.
- `rpc schema` is a catalogue of RPC *methods*, which is a different namespace from the CLI nouns. `vcli rpc call` addresses those methods, and the catalogue is the authority on their parameter names.
- The schema's `exit_codes` block is a published contract. If it disagrees with the codes the CLI actually returns, that is a product defect to report — the map documents the published values, and the proof is that both agree.

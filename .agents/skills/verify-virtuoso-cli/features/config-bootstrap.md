# Bootstrap the connection config

`vcli init` creates the connection config that every other command reads, and
`vcli config check` explains where each resolved value came from. Together they are
the first thing a user does and the first thing worth proving after a change.

## Sub-features

- `config-init-create` creates the config template and reports the path it wrote.
- `config-init-conflict` refuses to overwrite an existing config and exits `5`.
- `config-init-ifnotexists` skips an existing config and exits `0`.
- `config-template-content` writes a commented template with the documented keys.
- `config-check-resolve` reports every resolved value with the layer it came from.
- `config-check-digest` fingerprints the resolved configuration.
- `config-check-diagnostics` reports configuration problems without failing the command.
- `config-env-override` proves an environment variable beats the default layer.

## How to get to it (user POV)

- Run `vcli init` in a terminal.
- Run `vcli init --if-not-exists` in a script that must not fail on a second run.
- Run `vcli config check` to see where every setting came from.

## Driving it with vcli

Preconditions:

- The binary is built at `target/debug/vcli`.
- `VB_HOME` points at an empty directory for this run.
- `scripts/doctor.sh` reports `tier_a: "drivable"`.

- **Create the config.** Run `vcli --format json init`. Exit code `0` and stdout has `"status": "created"` and a `"path"` that ends in `config/vcli/config.toml`.
- **Show the file the tool will actually read.** Read the file at that `path`. It contains the commented template with `remote_host`, `remote_user`, `port`, `timeout`, `read_timeout`, `jump_host`, `ssh_port` and `ssh_key` keys, each commented out.
- **Second init conflicts.** Run `vcli --format json init` again. Exit code `5` (`conflict`) and **stderr** carries `"error": "conflict"` and a `message` naming the path. stdout is empty. The existing file is unchanged — compare its content before and after.
- **Idempotent variant.** Run `vcli --format json init --if-not-exists`. Exit code `0` and stdout has `"status": "skipped"` and `"reason": "config file already exists"`. The file is still unchanged.
- **Resolve the config.** Run `vcli --format json config check`. Exit code `0` and stdout has a `digest`, a non-empty `entries` array, and a `diagnostics` array. Each entry carries a `key`, a `value`, a `source_label` and a `source` object tagged with its `layer` — one of `env_profile`, `env`, `file_profile`, `file_global`, `target` or `default`. Only the `default` layer carries an extra `kind` (`hardcoded` or `derived`); the other layers name their origin instead (the env var, the config file, or the target).
- **Prove the default layer is what resolved it.** In the same output, the `VB_REMOTE_HOST` entry has `"value": ""` and `"source_label": "default="` when no environment variable is set. That is the default layer, not a stray env var.
- **Prove an environment override wins.** Run `VB_REMOTE_HOST=eda-server vcli --format json config check`. The `VB_REMOTE_HOST` entry now has `"value": "eda-server"` and `"source_label": "env[VB_REMOTE_HOST]"`, and the `digest` differs from the previous run's digest.
- **Side effect.** Read `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`. It gained one JSON record per invocation, each carrying `ts`, `cmd` and `exit_code` — including the `5` for the conflicting init.

## Gotchas

- `vcli init` is not idempotent. The second call exits `5` and changes nothing. In scripts use `--if-not-exists`.
- vcli writes **errors to stderr and results to stdout**. A conflict prints nothing on stdout, so an assertion that reads stdout only will see an empty string and pass vacuously. Capture both channels.
- The config path follows `VB_CONFIG_DIR` → `VB_HOME/config` → `XDG_CONFIG_HOME`, and always sits under a `vcli/` subdirectory. Under a fresh `VB_HOME` it is `$VB_HOME/config/vcli/config.toml`, **not** `~/.config/vcli/config.toml`. Assert on the path the command reports, never on a hardcoded one.
- Every vcli invocation appends to the command history, so the history holds one more record than the number of commands you think you ran. Count records rather than assuming.
- `config check` exits `0` even when `diagnostics` is non-empty. A non-empty `diagnostics` array is the signal, not the exit code.
- The `digest` covers the *resolved* configuration, so it changes when an environment override changes. That makes it the cheapest available proof that env overrides are honored.
- `vcli config check` with no verb prints help and exits `2` (usage error) without inspecting anything. The verb `check` is required.

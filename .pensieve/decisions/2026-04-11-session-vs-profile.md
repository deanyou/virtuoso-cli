# Session vs Profile: Two-Layer Connection Addressing

## Decision
Session and Profile solve different layers of the connection problem. Keep them orthogonal.

## Context
Users asked about the difference between `--session` and `--profile`. Both relate to
"which Virtuoso to connect to" but at different levels.

## Details
- **Session** = auto-registered identity of a running Virtuoso process on a specific server.
  Created by `ramic_bridge.il`, stored in `sessions/<id>.json`. Selects which daemon port.
- **Profile** = manually configured environment (server, SSH params, timeouts).
  Set via `VB_*_<profile>` env vars. Selects which server and how to connect.

They compose: `--profile gpu1 --session eda-meow-3` means "connect to gpu1 server,
then talk to the 3rd Virtuoso instance on that server."

State files are profile-scoped (`state_{profile}.json`) but sessions are not
(sessions are server-local, discovered after connecting).

## Alternatives Considered
- Single unified "target" concept — rejected because session is auto-discovered and
  profile is pre-configured. Merging them would lose the auto-discovery benefit.

---
name: tunnel-connect
description: Connect to Virtuoso via SSH tunnel or local bridge. Use when setting up Virtuoso connection, starting the bridge, or troubleshooting connectivity issues.
disable-model-invocation: true
argument-hint: '[host or issue, e.g. "server1.company.com"]'
allowed-tools: Bash(virtuoso *) Read
---

# Connect to Virtuoso

Establish connection to Cadence Virtuoso via the virtuoso-cli bridge.

## Quick Connect (from RAMIC Bridge Banner)

When user shows you the RAMIC Bridge banner like this:
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : 4e3898b12b7c-user-6           │
│  Port    : 38669                         │
│  SSH     : 2222                          │
│  Version : 0.3.2                         │
│  Daemon  : ~/.cargo/bin/virtuoso-daemon  │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

Use these commands to connect:
```bash
# 1. Create SSH tunnel
ssh -f -N -L <Port>:127.0.0.1:<Port> -p <SSH> user@localhost

# 2. Test connection
VB_PORT=<Port> VB_SESSION=<Session> vcli skill exec '1+1'
```

Example from the banner above:
```bash
ssh -f -N -L 38669:127.0.0.1:38669 -p 2222 user@localhost
VB_PORT=38669 VB_SESSION=4e3898b12b7c-user-6 vcli skill exec '1+1'
```

## Local mode

1. Ensure Virtuoso is running with the bridge loaded in CIW:
   ```skill
   load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
   ```

2. Set environment and test:
   ```bash
   export VB_REMOTE_HOST=localhost
   virtuoso tunnel status --format json
   virtuoso skill exec "1+1"
   ```

## Remote mode (Docker/Remote)

1. Initialize config: `virtuoso init`
2. Edit `.env` — set `VB_REMOTE_HOST` at minimum
3. Start tunnel: `virtuoso tunnel start`
4. Verify: `virtuoso tunnel status --format json`

## Troubleshooting

- **Daemon exits immediately**: Check `RBPython` points to a valid python3 binary (use full path like `/usr/bin/python3`)
- **Connection reset**: The daemon may have crashed — check if `conn.shutdown()` error in `/tmp/RB.log`; restart with `RBDLog = t` then `RBStop()` then `RBStart()` in CIW
- **Port already in use**: Run `RBStopAll()` in CIW, or change `VB_PORT` in `.env`
- **Multiple sessions**: Use `VB_SESSION` to specify which session to connect to

## Jump Host Misconfig Cheat Sheet

The single most common "I connected but I don't see my cells" problem is
pointing `VB_REMOTE_HOST` at the **jump host** instead of the
**compute host** where Virtuoso actually runs. `vcli tunnel status`
now detects this automatically — see `daemon.hostname_check` in the
JSON output, or look for the `⚠ hostname mismatch` block in Table mode.

```
       local box          jump host          compute host
          │            (bastion / SSH)        (Virtuoso)
          │                   │                    │
   vcli ──┴── SSH ──────────▶│  SSH ────────────▶ │
   env:                        ENV:                ENV:
     VB_REMOTE_HOST = compute-eda-42  ← the COMPUTE host
     VB_JUMP_HOST   = bastion-01      ← the JUMP host
```

### Symptoms

- `vcli rpc call --method cell.info --params '{"lib":"myLib",...}'` returns
  `library not found` even though the library exists in the Virtuoso
  CIW window
- `vcli session list` shows a session file with the wrong `host` field
- `getHostName()` on the remote daemon returns the bastion hostname,
  not the EDA server hostname
- `vcli tunnel status --format json | jq .daemon.hostname_check.mismatch`
  is `true`

### Diagnostic recipe

```bash
# 1. See what the daemon reports
vcli --session $VB_SESSION tunnel status --format json | \
  jq '.daemon.hostname_check'
# {
#   "configured": "bastion-01",   ← wrong! this is the jump host
#   "actual":     "compute-eda-42",
#   "mismatch":   true
# }

# 2. Cross-check what getHostName() returns directly
vcli --session $VB_SESSION rpc call --method cell.info \
  --params '{"lib":"tsmcN28","cell":"INVX2"}' --format json
# If this succeeds, the daemon CAN reach the cell DB — your
# code's library path is just wrong, not the network.

# 3. Check who the SSH session is actually logged in as
ssh -J $VB_JUMP_HOST $VB_REMOTE_HOST 'hostname && whoami'

# 4. Check the local state files for the wrong host
ls ~/.cache/virtuoso_bridge/sessions/
cat ~/.cache/virtuoso_bridge/sessions/<id>.json | jq .host
```

### Fix

| Symptom | Fix |
|---------|-----|
| `daemon.hostname_check.mismatch == true` | `export VB_REMOTE_HOST=<compute host, not bastion>` |
| `vcli session list` shows wrong `host` | `vcli session cleanup` then restart with the right `VB_REMOTE_HOST` |
| `cell.info` returns "library not found" for a lib that exists in CIW | Check `cds.lib` DEFINE lines on the compute host — they're relative to the daemon's PWD, not yours |
| SSH works but daemon doesn't start | Check that `~/.bashrc` on compute host sources Cadence init (`source /path/to/cshrc` or similar) — Virtuoso must be in `$PATH` for `virtuoso` to be launchable via `system()` |

### Why this matters

`VB_REMOTE_HOST` is what the daemon binds to and what `getHostName()`
should return. If the jump host has any kind of CIW (e.g. a user
inadvertently ran `load("ramic_bridge.il")` there), the daemon
will happily come up on the jump host — your SSH tunnel works,
your session file is written, but every library lookup fails
because the jump host has no `cds.lib` pointing to your design.

`vcli tunnel status` is the fast way to catch this: if the
`hostname_check.mismatch` is `true`, **stop debugging and fix
`VB_REMOTE_HOST` first** — every other "library not found"
error message after that is a downstream symptom.


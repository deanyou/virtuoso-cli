---
name: tunnel-connect
description: Connect to Virtuoso via SSH tunnel or local bridge. Use when setting up Virtuoso connection, starting the bridge, or troubleshooting connectivity issues.
disable-model-invocation: true
argument-hint: [host or issue, e.g. "server1.company.com"]
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

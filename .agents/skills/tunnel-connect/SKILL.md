---
name: tunnel-connect
description: Connect to Virtuoso via SSH tunnel or local bridge. Use when setting up Virtuoso connection, starting the bridge, or troubleshooting connectivity issues.
disable-model-invocation: true
allowed-tools: Bash(*/virtuoso *) Read
---

# Connect to Virtuoso

Establish connection to Cadence Virtuoso via the virtuoso-cli bridge.

## Local mode

1. Ensure Virtuoso is running with the bridge loaded in CIW:
   ```skill
   RBPython = "/path/to/python3"
   RBDPath = "/path/to/virtuoso-cli/resources/daemons/ramic_bridge_daemon_3.py"
   load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
   ```

2. Set environment and test:
   ```bash
   export VB_REMOTE_HOST=localhost
   virtuoso tunnel status --format json
   virtuoso skill exec "1+1"
   ```

## Remote mode

1. Initialize config: `virtuoso init`
2. Edit `.env` — set `VB_REMOTE_HOST` at minimum
3. Start tunnel: `virtuoso tunnel start`
4. Verify: `virtuoso tunnel status --format json`

## Troubleshooting

- **Daemon exits immediately**: Check `RBPython` points to a valid python3 binary (use full path like `/usr/bin/python3`)
- **Connection reset**: The daemon may have crashed — check if `conn.shutdown()` error in `/tmp/RB.log`; restart with `RBDLog = t` then `RBStop()` then `RBStart()` in CIW
- **Port already in use**: Run `RBStopAll()` in CIW, or change `VB_PORT` in `.env`

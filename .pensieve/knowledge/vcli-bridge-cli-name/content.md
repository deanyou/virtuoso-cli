# vcli is the Bridge CLI — `virtuoso` in PATH is Cadence's Application Binary

## Source
Session 2026-04-19: Claude repeatedly called `virtuoso skill exec` thinking it was
the vcli bridge client. Each call launched a full ~640 MB Cadence Virtuoso process.

## Summary
The CLI tool for the RAMIC bridge is named `vcli`, not `virtuoso`.
The `virtuoso` command found in PATH is `/opt/cadence/IC231/tools/dfII/bin/virtuoso`
— Cadence's graphical application. Never call it from automation.

## Content

### Binary Locations

| What | Path | Purpose |
|------|------|---------|
| Bridge CLI (correct) | `target/release/vcli` (on PATH after install) | Talks to RAMIC bridge via socket |
| Cadence Virtuoso | `/opt/cadence/IC231/tools/dfII/bin/virtuoso` | Launches full GUI application |

### Symptom of Wrong Binary

```bash
# Wrong — launches full Virtuoso instance (~640MB RAM, 20-30s startup):
virtuoso skill exec 'modelFile(...)'

# Correct — sends SKILL to running session in <50ms:
vcli skill exec 'modelFile(...)'
```

Signs you called the wrong binary:
- Command takes >10s instead of <1s
- `ps aux | grep virtuoso` shows a NEW process with a different PID than the user's session
- The user's Virtuoso session is unaffected (they didn't see anything change in ADE)
- Shell outputs nothing (headless Virtuoso exits after failing to open display)

### Why the Confusion

Both `virtuoso` and `vcli` accept `skill exec` as arguments. The Cadence binary parses
and tries to evaluate SKILL in a fresh headless context, which either crashes or
exits without producing useful output.

The vcli binary must be in PATH explicitly:
```bash
export PATH="$HOME/git/virtuoso-cli/target/release:$PATH"
# or after cargo install: ~/.cargo/bin/vcli
```

### Connect to an Existing Session

```bash
# Always set VB_PORT and VB_SESSION from the bridge startup message:
VB_PORT=38991 VB_SESSION=meowu-meow-2 vcli skill exec 'design()'
```

## When to Use
- Any time `skill exec`, `sim run`, `sim setup` etc. commands are needed
- When debugging why SKILL commands seem to have no effect on the running session

## Context Links
- Related: [[ocean-design-cell-switch]] — correct setup sequence after connecting
- Related: [[ocean-createnetlist-prerequisites]] — full Ocean session setup

---
name: diag
description: |
  Read-only diagnostics for stuck Virtuoso state via `vcli diag ...`.
  Use when: (1) a cellview won't open and you suspect `.cdslck` lock conflicts,
  (2) "why is this view held by someone?" — find the holder without deleting
  anything, (3) a modal dialog has deadlocked the CIW and the SKILL path can't
  help (use `vcli window dismiss-dialog --x11` instead).
---

# Read-only diagnostics

All `vcli diag` commands are **read-only** by design. They never delete
locks, never close cellviews, never run a SKILL command that could
mutate state. Use them when "something is wrong but I don't know what."

## `vcli diag cdslck <LIB>`

Enumerate every `.cdslck` lock file under an OA library, report who
holds it (owner@host:pid:start_time) and how old it is.

```bash
# All locks under FT0001A_SH
vcli diag cdslck FT0001A_SH

# Only maestro view locks
vcli diag cdslck FT0001A_SH --view maestro
```

Sample output:

```json
{
  "library": "FT0001A_SH",
  "read_path": "/home/meow/projects/ft0001/FT0001A_SH",
  "count": 1,
  "locks": [
    {
      "path": "/home/meow/projects/ft0001/FT0001A_SH/INVX2/maestro/.cdslck",
      "relative": "/INVX2/maestro/.cdslck",
      "cellview": "/INVX2/maestro",
      "owner_record": "meow@eda:12345:1717820000",
      "owner": "meow",
      "host": "eda",
      "pid": 12345,
      "mtime": 1717820000,
      "age_seconds": 4237.2,
      "age_human": "1.2h"
    }
  ]
}
```

### Workflow when a lock is held

1. **Check if the holder is still alive:**
   ```bash
   # On the lock's host (from `host` field):
   ssh <host> ps -p <pid>
   ```
2. **If alive** — the holder has the cellview open. Coordinate or wait.
3. **If dead** — the lock is stale. **Confirm** with the owner, then:
   ```bash
   # On the lock's host:
   ssh <host> rm <path>
   ```
   ⚠️ **Never `rm -f` a live lock** — it corrupts the cellview.

### Implementation notes

- Resolves `readPath` via the named `cell.read_path` RPC (not raw SKILL
  exec), so non-admin users can run this.
- Enumerates locks with SSH `find` — does **not** go through the SKILL
  channel, which is precisely what you may be trying to debug.
- Batched `cat` + `stat` over one SSH round-trip, so even libraries
  with hundreds of locks finish in < 1 s.

## Related: `vcli window dismiss-dialog --x11`

When a modal dialog has deadlocked the CIW, the SKILL channel itself
is stuck. The X11 SSH bypass SSHes into the same host, finds the
modal with `xwininfo`, and sends a keypress to dismiss it.

```bash
# Default: send Enter
vcli window dismiss-dialog --x11

# Cancel button
vcli window dismiss-dialog --x11 --action escape

# "No" button (for Save As / dedupe dialogs)
vcli window dismiss-dialog --x11 --action alt-n
```

**Prerequisite**: `VB_REMOTE_HOST` set; the Python helper is vendored
in `resources/x11_dismiss_dialog.py` (no `pip install` needed) and
auto-uploaded to `/tmp/virtuoso_bridge/<client>/x11/`. Requires
`python3-Xlib` on the remote host.

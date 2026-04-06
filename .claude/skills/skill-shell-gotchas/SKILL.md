---
name: skill-shell-gotchas
description: |
  Critical SKILL language gotchas when integrating with shell/IPC in Cadence Virtuoso.
  Use when: (1) ipcBeginProcess exits with state=127 (command not found), (2) sh() 
  returns unexpected values like "t" instead of command output, (3) trying to capture 
  shell stdout in SKILL, (4) writing files from SKILL with fprintf/outfile producing 
  0-byte output, (5) getpid() undefined error in SKILL.
author: Claude Code
version: 1.0.0
date: 2026-04-06
---

# SKILL Shell & IPC Gotchas

## Problem 1: `sh()` Returns `t`/`nil`, Not stdout

### Symptom
`ipcBeginProcess` exits with `state=127` (command not found). Variable is set to `"t"` 
instead of a file path.

### Root Cause
`sh()` in SKILL returns `t` (success) or `nil` (failure) — it does NOT return stdout.

```skill
; WRONG — fromPath will be "t", not a path
fromPath = car(parseString(sh("which virtuoso-daemon 2>/dev/null") "\n\r"))
if(fromPath then  ; "t" is truthy → takes this branch with wrong value
    fromPath      ; returns "t", not a real path
```

```skill
; CORRECT — use isFile() to validate; skip which entirely
cargoPath = strcat(getShellEnvVar("HOME") "/.cargo/bin/virtuoso-daemon")
if(isFile(cargoPath) then cargoPath else "")
```

### Fix
Never use `sh()` to capture command output. Use it only for side effects (mkdir, kill, etc.).
For path resolution, use `getShellEnvVar("RB_DAEMON_PATH")` or check known fixed locations
with `isFile()`.

---

## Problem 2: `fprintf`/`outfile` Writes 0-Byte Files

### Symptom
`outfile`/`fprintf` calls succeed (return `t`) but the file is empty or not created.

### Root Cause
SKILL's `fprintf` has buffering issues in Virtuoso's IPC context — the buffer is not 
flushed to disk reliably.

### Fix
Use `sh()` with shell's `printf` to write files:

```skill
; WRONG
port = outfile("/tmp/session.json" "w")
fprintf(port "{\"id\":\"%s\",\"port\":%d}" sessionId portNum)

; CORRECT — delegate file writing to shell
sh(sprintf(nil "printf '{\"id\":\"%s\",\"port\":%d}' > \"%s\""
    sessionId portNum filename))
```

---

## Problem 3: `getpid()` Is Undefined in SKILL

### Symptom
`*Error* eval: undefined function - getpid`

### Root Cause
SKILL has no `getpid()` function. There is no standard way to get the current process PID.

### Fix
If PID is needed for session tracking, store `0` as a placeholder. Use TCP port reachability 
(`TcpStream::connect_timeout`) to check liveness instead of PID signals.

---

## Problem 4: `boundp` Preserves Stale Values Across `load()`

### Symptom
After fixing a bug in a variable's initialization, reloading the .il file doesn't pick 
up the fix because `unless(boundp('Var))` skips re-initialization.

### Root Cause
`unless(boundp('RBDPath))` only runs when the variable is unbound. If a previous load 
set it to a bad value (e.g., `"t"`), it stays bad across reloads.

### Fix
Add validity checks to the guard condition:

```skill
; WRONG — stale "t" value is preserved
unless(boundp('RBDPath)
    RBDPath = ...
)

; CORRECT — re-resolve if empty, nil, or not a valid file
when(!boundp('RBDPath) || RBDPath == "" || RBDPath == nil || !isFile(RBDPath)
    RBDPath = ...
)
```

---

## Problem 5: `ipcBeginProcess` PATH Is Stripped

### Symptom
`ipcBeginProcess` exits with `state=127` even though the binary exists on the system.

### Root Cause
Virtuoso's `ipcBeginProcess` launches via a stripped shell environment that does NOT 
inherit the user's `PATH`. Binaries in `~/.cargo/bin`, `/usr/local/bin`, etc. may not 
be found via bare name.

### Fix
Always pass the **absolute path** to the binary. Resolve it at load time:

```skill
; Priority resolution (highest to lowest):
; 1. RB_DAEMON_PATH env var (user override)
; 2. Known absolute install locations (isFile check)
RBDPath = let((fromEnv cargoPath)
    fromEnv = getShellEnvVar("RB_DAEMON_PATH")
    if(fromEnv then
        fromEnv
    else
        cargoPath = strcat(getShellEnvVar("HOME") "/.cargo/bin/virtuoso-daemon")
        if(isFile(cargoPath) then cargoPath else "")
    )
)
```

---

## Summary Table

| What you want to do | WRONG approach | CORRECT approach |
|---------------------|---------------|-----------------|
| Capture shell stdout | `sh("which foo")` → returns `"t"` | Use `isFile()` to probe paths |
| Write a file | `fprintf(port ...)` | `sh(sprintf(nil "printf '...' > file"))` |
| Get current PID | `getpid()` → undefined | Store 0; use TCP probe for liveness |
| Protect init across reload | `unless(boundp('V))` | `when(!boundp ... || !isFile(V))` |
| Run binary via ipc | bare name `"foo"` | absolute path `"/home/user/.cargo/bin/foo"` |

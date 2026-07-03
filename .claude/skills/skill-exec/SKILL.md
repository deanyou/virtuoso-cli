---
name: skill-exec
description: Execute SKILL code on Virtuoso. Use when running SKILL expressions, querying cellview data, listing libraries/cells, or interacting with Virtuoso programmatically.
argument-hint: "[SKILL expression to run]"
allowed-tools: Bash(vcli *) Bash(virtuoso *)
---

# Execute SKILL Code on Virtuoso

Run SKILL expressions via `vcli skill exec` and parse results.

## Quick reference

```bash
# Arithmetic
vcli skill exec "1+2" --format json

# String operations
vcli skill exec 'strcat("hello" " " "world")' --format json

# List all libraries
vcli skill exec 'foreach(mapcar lib ddGetLibList() lib~>name)' --format json

# List cells in a library
vcli skill exec 'let((lib) lib=ddGetObj("myLib") foreach(mapcar c lib~>cells c~>name))' --format json

# Get cell views
vcli skill exec 'let((cell) cell=ddGetObj("myLib" "myCell") foreach(mapcar v cell~>views v~>name))' --format json

# Current cellview info
vcli skill exec 'let((cv) cv=geGetEditCellView() list(cv~>libName cv~>cellName cv~>viewName))' --format json

# Instance count
vcli skill exec 'let((cv) cv=geGetEditCellView() length(cv~>instances))' --format json

# Net names
vcli skill exec 'let((cv result n) cv=geGetEditCellView() result=nil n=0 foreach(net cv~>nets when(n<20 result=cons(net~>name result) n=n+1)) result)' --format json

# Schematic read (read-only)
vcli skill exec 'let((cv) cv=dbOpenCellViewByType("lib" "cell" "schematic" nil "r") sprintf(nil "inst=%d nets=%d" length(cv~>instances) length(cv~>nets)))' --format json
```

## Multi-session usage

When multiple Virtuoso instances are running, specify the session explicitly:

```bash
vcli session list                                          # find alive session IDs
vcli --session meowu-meow-38371 skill exec 'getCurrentTime()'
export VB_SESSION=meowu-meow-38371                        # or set once for the shell
vcli skill exec 'getCurrentTime()'
```

## Connection failure recovery

> ⚠️ **Gotcha**: `--session <id>` bypasses auto-filtering. If Virtuoso restarts, the
> session ID changes (new port → new ID). Explicitly specifying a dead session returns
> `connection_failed: Connection refused` (exit 1) even though other sessions are alive.

Recovery pattern (observed signal: exit 1 on `skill exec` followed by `session list`):

```bash
# 1. Stale session detected
vcli --session meowu-meow-32987 skill exec 'getCurrentTime()'
# → connection_failed: Connection refused (exit 1)

# 2. Purge dead session files and find new IDs
vcli session cleanup
vcli session list

# 3. Retry with the new session ID
vcli --session meowu-meow-38371 skill exec 'getCurrentTime()'
```

## Important notes

- Use `--format json` for structured output (auto in pipe mode)
- Use `--timeout N` for long-running operations (default 30s)
- SKILL strings use `"`, escape with `\"` inside bash single quotes
- `let` blocks work for local variables; Ocean functions (simulator, design, run) must be at top level
- View names may not be standard `schematic` — check with `v~>name` not `v~>viewName`
- Always wrap slot access in `let`: `let((cv) cv=geGetEditCellView() cv~>cellName)` not `geGetEditCellView()~>cellName` — the latter crashes if no design is open

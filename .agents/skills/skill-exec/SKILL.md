---
name: skill-exec
description: Execute SKILL code on Virtuoso. Use when running SKILL expressions, querying cellview data, listing libraries/cells, or interacting with Virtuoso programmatically.
allowed-tools: Bash(*/virtuoso *)
---

# Execute SKILL Code on Virtuoso

Run SKILL expressions via `virtuoso skill exec` and parse results.

## Quick reference

```bash
# Arithmetic
virtuoso skill exec "1+2" --format json

# String operations
virtuoso skill exec 'strcat("hello" " " "world")' --format json

# List all libraries
virtuoso skill exec 'foreach(mapcar lib ddGetLibList() lib~>name)' --format json

# List cells in a library
virtuoso skill exec 'let((lib) lib=ddGetObj("myLib") foreach(mapcar c lib~>cells c~>name))' --format json

# Get cell views
virtuoso skill exec 'let((cell) cell=ddGetObj("myLib" "myCell") foreach(mapcar v cell~>views v~>name))' --format json

# Current cellview info
virtuoso skill exec 'let((cv) cv=geGetEditCellView() list(cv~>libName cv~>cellName cv~>viewName))' --format json

# Instance count
virtuoso skill exec 'let((cv) cv=geGetEditCellView() length(cv~>instances))' --format json

# Net names
virtuoso skill exec 'let((cv result n) cv=geGetEditCellView() result=nil n=0 foreach(net cv~>nets when(n<20 result=cons(net~>name result) n=n+1)) result)' --format json

# Schematic read (read-only)
virtuoso skill exec 'let((cv) cv=dbOpenCellViewByType("lib" "cell" "schematic" nil "r") sprintf(nil "inst=%d nets=%d" length(cv~>instances) length(cv~>nets)))' --format json
```

## Important notes

- Use `--format json` for structured output (auto in pipe mode)
- Use `--timeout N` for long-running operations (default 30s)
- SKILL strings use `"`, escape with `\"` inside bash single quotes
- `let` blocks work for local variables; Ocean functions (simulator, design, run) must be at top level
- View names may not be standard `schematic` — check with `v~>name` not `v~>viewName`

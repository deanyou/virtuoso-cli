---
id: content
type: knowledge
title: Cadence Virtuoso: Library Not Registered in Session
status: active
created: 2026-05-27
updated: 2026-05-27
tags: ["knowledge"]
---

# Cadence Virtuoso: Library Not Registered in Session

## Source
Session investigation 2026-04-17: `vcli sim netlist` returned `err_count == -1` with
`ddGetLibList()` returning an empty list — Virtuoso was started from `/home/meow/git/virtuoso-cli`
which had no `cds.lib` defining `FT0001A_SH`.

## Summary
If Virtuoso starts from a directory with no `cds.lib` that includes the target library,
`ddGetLibList()` will not contain that library. `dbOpenCellViewByType` and `createNetlist`
will silently fail with nil. The symptom overlaps with OSSHNL-109 (both produce `err_count == -1`
in `create_netlist_inner`). The two must be distinguished with a `ddGetLibList()` probe.

## Content

### Trigger Condition
Virtuoso's working directory (logged in `CDS.log` as `Working Directory:`) does not have a
`cds.lib` that includes the target library, AND no other mechanism (site/user `cds.lib`) brings
it in. Libraries are registered at startup from `cds.lib` files; DD database is not refreshed at
runtime unless explicitly told to.

### Diagnosis: Read CDS.log

```bash
grep "Working Directory" ~/CDS.log
# → Working Directory: meowu:/home/meow/git/virtuoso-cli   ← wrong
# → Working Directory: meowu:/home/meow/projects/ft0001    ← correct
```

### Probe from vcli / bridge

```skill
; Returns "found" if registered, nil otherwise
when(car(setof(l ddGetLibList() l~>name=="FT0001A_SH")) "found")
```

In `create_netlist_inner` (sim.rs), when `err_count == -1`, this probe distinguishes:
- `"found"` → OSSHNL-109 (cv held in "a" mode) → return `Ok("t")` and let caller verify
- anything else → library not registered → return `Err` with actionable message

### Fix: Correct Startup Directory
Start Virtuoso from the project directory whose `cds.lib` includes the library:
```bash
cd /home/meow/projects/ft0001
virtuoso &
```

### vcli Error Message (after the ddGetLibList probe)
```
Library 'FT0001A_SH' is not registered in the current Virtuoso session.
Virtuoso was started from '/home/meow/git/virtuoso-cli'.
Start Virtuoso from the project directory whose cds.lib includes 'FT0001A_SH',
or run hiLoadCDSLibDefs() in the CIW to register it at runtime.
```

## When to Use
- `vcli sim netlist` returns err_count == -1 for a library that definitely exists on disk
- `ddGetLibList()` from bridge does not show the expected library
- Virtuoso CDS.log shows wrong `Working Directory`
- `dbOpenCellViewByType` returns nil for a known-good cell

## Context Links
- Related: [[cadence-osshnl-109]] (same err_count == -1 symptom, different root cause)
- Related: [[ocean-createnetlist-prerequisites]] (full Ocean session setup for createNetlist)

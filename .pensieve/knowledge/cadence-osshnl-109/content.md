# Cadence OSSHNL-109: Schematic Modified Since Last Extraction

## Source
Discovered during `vcli sim netlist --recreate` debugging, April 2026.
Confirmed via `si.foregnd.log`: `ERROR (OSSHNL-109): The cellview 'FT0001A_SH/ota5t/schematic' has been modified since the last extraction. Run Check and Save.`

## Summary
`createNetlist` returns nil when a schematic is modified via SKILL (`dbSave`) without running `schCheck` first — the extraction timestamp becomes stale, and incremental netlisting fails.

## Content

### Trigger Conditions
- Schematic edited programmatically via SKILL (`dbReplaceProp` + `dbSave`)
- `schCheck(cv)` was NOT called before `dbSave`
- `createNetlist(?display nil)` (incremental mode) is attempted next

### Symptom
- `createNetlist` returns `nil` silently
- `artSimEnvLog` shows: `generate netlist... ...unsuccessful.`
- `si.foregnd.log` contains: `ERROR (OSSHNL-109): The cellview ... has been modified since the last extraction. Run Check and Save.`
- `recreateAll t` may bypass this (does its own check internally); incremental mode always hits it.

### Root Cause
`dbSave(cv)` saves the OA file but does NOT update Cadence's extraction timestamp.
`schCheck(cv)` is what updates the extraction timestamp that `createNetlist` checks.
Cadence GUI "Check and Save" runs both; programmatic `dbSave` only does the file write.

### Fix
Run `schCheck(cv)` before `dbSave(cv)`:
```skill
let((cv chk)
  cv = dbOpenCellViewByType("lib" "cell" "view")          ; opens in "a" (write) mode
  unless(cv
    cv = car(setof(ocv dbGetOpenCellViews()
                   and(ocv~>libName=="lib"
                       ocv~>cellName=="cell"
                       ocv~>viewName=="view"
                       ocv~>mode=="a"))))                  ; fallback: reuse held cv
  if(cv
    progn(chk = schCheck(cv)
          when(car(chk)==0 dbSave(cv))
          list(car(chk)))
    list(-1)))                                             ; -1 = cv not found
```
`schCheck` returns `(errorCount warningCount)`. `car(chk)==0` means no errors.

### Why `dbOpenCellViewByType` May Return nil
If Ocean/createNetlist holds the cv in "a" mode, a second `dbOpenCellViewByType` call returns nil.
The fallback via `dbGetOpenCellViews()` finds the already-open writable cv handle and reuses it.

### vcli Implementation
`src/commands/sim.rs::create_netlist_inner` — automatically retries after OSSHNL-109:
1. First attempt: `createNetlist(?display nil)` or `(?recreateAll t)`
2. On nil: run `schCheck+dbSave` via SKILL (with cv fallback)
3. Retry createNetlist

## When to Use
- Any time `createNetlist` returns nil after a SKILL-based schematic edit
- When `si.foregnd.log` contains OSSHNL-109
- When implementing programmatic schematic modifications that must be followed by netlisting

## Context Links
- Related: [[cadence-ic23-dbopencellviewbytype]] (cv open modes, already-held cv pattern)
- Related: [[smic-pdk-transistor-w-skill]] (what to set for W/L changes)

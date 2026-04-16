# Cadence IC23: dbOpenCellViewByType API and Open-Mode Behavior

## Source
Empirically verified in IC23.1 (Virtuoso), April 2026. Session testing with `FT0001A_SH/ota5t/schematic`.

## Summary
`dbOpenCellViewByType` in IC23 uses a 3-arg form that opens in "a" (write) mode by default; the 4-arg/5-arg forms fail unless the viewType is a valid OA identifier (which for standard schematic views it is not via string name).

## Content

### Working Forms in IC23

```skill
; 3-arg form — opens in "a" (write) mode by default ✅
cv = dbOpenCellViewByType("lib" "cell" "view")

; Confirmed behavior:
; cv~>mode  → "a"
; cv~>libName → "lib"
```

### Broken Forms

```skill
; 4-arg: 4th arg is viewType, NOT mode — "a" is not a valid OA viewType → nil ❌
cv = dbOpenCellViewByType("lib" "cell" "view" "a")

; 5-arg: viewType="schematic" is not valid OA type → nil ❌  
cv = dbOpenCellViewByType("lib" "cell" "view" "schematic" "a")
```

The OA viewType identifiers are not the same as the view name strings. Avoid passing viewType unless you have the correct OA type identifier.

### Already-Held cv (Returns nil)

When Ocean or another netlisting operation holds the cv in "a" mode, `dbOpenCellViewByType` returns nil even with the correct form. Detect via `dbGetOpenCellViews()`:

```skill
; Find already-open write-mode cv (must use and() — setof body is progn, only last
; expression acts as filter)
cv = car(setof(ocv dbGetOpenCellViews()
               and(ocv~>libName=="lib"
                   ocv~>cellName=="cell"
                   ocv~>viewName=="view"
                   ocv~>mode=="a")))
```

### SKILL `setof` Multi-Condition Pitfall

`setof(var list body...)` evaluates ALL body expressions as `progn` — only the LAST determines inclusion. Multiple conditions without `and()` act as independent `progn` statements:

```skill
; WRONG — only ocv~>mode=="a" is the actual filter ❌
setof(ocv cvs ocv~>libName=="lib" ocv~>cellName=="cell" ocv~>mode=="a")

; CORRECT — all three conditions filter together ✅
setof(ocv cvs and(ocv~>libName=="lib" ocv~>cellName=="cell" ocv~>mode=="a"))
```

### Listing All Open CVs

```skill
; List all open cvs with their modes
mapcar(lambda(cv list(cv~>libName cv~>cellName cv~>viewName cv~>mode))
       dbGetOpenCellViews())
```

### dbSave on Already-Open cv

`dbSave(cv)` on a cv opened with the 3-arg form (or retrieved from `dbGetOpenCellViews()`) works correctly. `dbSave` requires the cv to be in "a" mode — read-mode cv handles cannot be saved.

## When to Use
- Any time you need to open a schematic cellview for writing from SKILL
- When `dbOpenCellViewByType` returns nil unexpectedly
- When implementing the OSSHNL-109 fix (schCheck + dbSave flow)
- When debugging "cv locked" situations in IC23

## Context Links
- Leads to: [[cadence-osshnl-109]] (the main use case for write-mode cv access)
- Related: [[cadence-ic23-schcreateinst]] (other IC23 schematic SKILL APIs)

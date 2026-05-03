# Maestro View Bootstrap — maeOpenSetup + maeSaveSetup

## Source
2026-05-02: Arcadia-1/virtuoso-bridge-lite example 07_ensure_maestro_view.py (PR #67).
`open_gui_session` / `deOpenCellView("a")` assumes maestro view already on disk.

## Summary
First-ever open of a new cell in Maestro needs a two-step bootstrap before `vcli maestro open`,
otherwise `deOpenCellView` returns nil and a blocking GUI dialog freezes the SKILL channel.

## Content

### Symptom
- `vcli maestro open --lib L --cell C` returns nil on a brand-new cell
- Virtuoso shows "Data file does not exist" dialog — blocks SKILL channel indefinitely
- `ls <lib>/<cell>/maestro/` → directory does not exist

### Root Cause
`deOpenCellView(... "maestro" "maestro" nil "a")` requires `maestro/master.tag` and
`maestro/maestro.sdb` to already exist. These files are only created when the cell is
first saved inside Maestro — which never happens for a programmatically-created cell.

### Fix — Two SKILL calls (idempotent)

```bash
# Creates maestro/ directory + master.tag + maestro.sdb in memory, then flushes to disk
vcli skill exec 'maeOpenSetup("LIB" "CELL" "maestro")'
# → "fnxSession12"   (background session name)
vcli skill exec 'maeSaveSetup(?session "fnxSession12")'
# → t

# Now the normal open works
vcli maestro open --lib LIB --cell CELL
```

Calling this on an already-existing maestro view is safe — `maeOpenSetup` re-attaches
and `maeSaveSetup` is a no-op if nothing changed.

### When to Use
- After `vcli cell open` / `vcli schematic ...` creates a new testbench cell
- Any automation workflow that creates cells programmatically before opening Maestro
- When `vcli maestro open` fails on a cell that was just created

## Context Links
- Related: [[maestro-session-types]] — maestro vs adexl session types
- Related: [[cadence-ic23-dbopencellviewbytype]] — deOpenCellView modes

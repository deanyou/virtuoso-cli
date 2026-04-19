# Ocean: Switching Target Cell Requires Explicit design() Call

## Source
Session investigation 2026-04-17: `vcli sim netlist --lib FT0001A_SH --cell 5T_OTA_D_TO_S_sim`
produced a netlist for `ota5t` instead of `5T_OTA_D_TO_S_sim` after switching cells.

## Summary
When switching the Ocean target cell (e.g., from `ota5t` to `5T_OTA_D_TO_S_sim`),
`vcli sim netlist` may still generate the netlist for the previous cell because the
`resultsDir` binding doesn't update synchronously. Explicitly calling
`design("lib" "cell" "view")` and verifying the output before netlisting ensures
the correct target.

## Content

### Symptom

```bash
# After switching from ota5t to 5T_OTA_D_TO_S_sim:
vcli sim netlist --lib FT0001A_SH --cell 5T_OTA_D_TO_S_sim

# Output shows WRONG cell path:
# "netlist_path": "/home/.../simulation/ota5t/spectre/schematic/netlist/input.scs"
```

### Root Cause

`vcli sim netlist` calls `setup_skill(lib, cell, view, "spectre")` which runs
`simulator('spectre) design("lib" "cell" "view") resultsDir()` in one SKILL call.
The `design()` function updates the Ocean session, but `resultsDir()` at the end
may still reflect the previously active cell's path if the new cell's simulation
directory doesn't exist yet or the ADE session wasn't properly re-initialized.

When `createNetlist` then returns `"t"` (indicating success), `vcli sim netlist`
resolves the netlist path from `resultsDir()` — which is still the old cell's path.

### Fix: Explicit design() Before Netlisting

```bash
# Step 1: Explicitly switch design and verify
vcli skill exec 'design("FT0001A_SH" "5T_OTA_D_TO_S_sim" "schematic")'
# Verify:
vcli skill exec 'list(design() resultsDir())'
# → should show 5T_OTA_D_TO_S_sim and its simulation path

# Step 2: Now netlist
vcli sim netlist --lib FT0001A_SH --cell 5T_OTA_D_TO_S_sim --recreate
```

### Also: desVar() State is Per-Design

After switching with `design()`, all `desVar()` values reset to empty strings `""`.
Always set design variables after switching:

```bash
vcli skill exec 'progn(desVar("W34" 16e-6) desVar("vdc" 0.6) desVar("v1" 0.55) desVar("v2" 0.65))'
```

### Verification

```bash
vcli skill exec 'list(design() resultsDir())'
# Good: ((FT0001A_SH 5T_OTA_D_TO_S_sim schematic) "/path/to/5T_OTA_D_TO_S_sim/...")
# Bad:  ((FT0001A_SH ota5t schematic) "/path/to/ota5t/...")
```

### design() Returns nil After vcli sim setup on a Fresh Session

Even when `vcli sim setup` returns `"status": "success"`, calling `design()` (no
args) in a subsequent SKILL call may still return `nil`. This happens when the
Ocean session had no prior design registered.

**Workaround**: Always call `design("LIB" "CELL" "VIEW")` explicitly before
`sim run` or `createNetlist` if `design()` returns nil:

```bash
VB_PORT=38991 VB_SESSION=meowu-meow-2 vcli skill exec 'design()' --format json
# If nil, call explicitly:
VB_PORT=38991 VB_SESSION=meowu-meow-2 vcli skill exec 'design("FT0001A_SH" "Bandgap" "schematic")'
# Re-set modelFile after design() call (simulator reset clears it):
VB_PORT=38991 VB_SESSION=meowu-meow-2 vcli skill exec 'modelFile(...)'
```

## When to Use
- When switching simulation target cells between runs
- When `vcli sim netlist --cell X` produces a netlist at a different cell's path
- When `resultsDir()` returns a path for the wrong cell after `sim setup`
- When `design()` returns nil despite `vcli sim setup` succeeding

## Context Links
- Related: [[ocean-netlist-regen]] — full Ocean simulation reliability reference

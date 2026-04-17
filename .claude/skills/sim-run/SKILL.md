---
name: sim-run
description: Run circuit simulation (DC, tran, AC) on Virtuoso. Use when executing Spectre simulation, running analysis, or checking simulation results.
allowed-tools: Bash(*/virtuoso *)
---

# Run Simulation

Execute Spectre simulation via `virtuoso sim run`.

## Prerequisites

Simulation must be set up first (see `/sim-setup`):
- `simulator('spectre)` configured
- `design(lib cell view)` set
- `modelFile(...)` configured
- `desVar(...)` set for any parameterized variables
- `resultsDir(...)` set

## Run analysis

```bash
# DC operating point
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 120 --format json

# Transient
virtuoso sim run --analysis tran --stop 10u --timeout 300 --format json

# AC
virtuoso sim run --analysis ac --start 1 --stop 1e9 --dec 10 --timeout 300 --format json
```

## Important: resultsDir — DO NOT CHANGE

⚠️ **The ADE session binds `run()` to the resultsDir established when the GUI session
was first created. Changing it to any other path silently breaks `run()` (returns nil).**

```bash
# ✅ Find the canonical path from the runSimulation script
cat <netlist_dir>/runSimulation | grep "\-raw"
# → -raw ../../../../tmp/opt_5t_ota/psf  →  canonical = /tmp/opt_5t_ota

# ✅ If resultsDir is nil, restore to canonical path
virtuoso skill exec 'resultsDir("/tmp/opt_5t_ota")'

# ❌ Never do this before a run — it will break run()
virtuoso skill exec 'resultsDir("/tmp/some_new_path")'
```

`sim run` now returns a clear error if `resultsDir` is nil instead of auto-setting it
to a temp path (which would break the ADE session binding).

Also: **do not call `sim setup` again once a working ADE session exists** — it resets
the Ocean session state and can cause `run()` to return nil.

## Verify success

After `run()`, check spectre.out for errors:

```bash
# Check the log
virtuoso skill exec 'resultsDir()' --format json
# Then read the spectre.out file at <resultsDir>/psf/spectre.out

# A successful run has PSF data files (dcOp.dc, tran.tran, etc.)
# A failed run only has artistLogFile, simRunData, variables_file
```

## Key indicators

| run() output | Meaning |
|-------------|---------|
| Returns resultsDir path AND PSF data files exist | Simulation completed ✓ |
| Returns resultsDir path BUT only artistLogFile/simRunData/spectre.out | **Spectre failed** — run() does NOT check spectre's exit code |
| Returns nil, takes <0.01s | Analysis not configured or session lost |
| Returns nil, takes >0.1s | Netlisting ran but spectre may have failed |

⚠️ **`run()` returning a non-nil path is not sufficient proof of success.** Spectre can
terminate with fatal errors (e.g., SFE-868) and `run()` still returns the resultsDir.
Always verify PSF data files exist:
```bash
tail -2 <resultsDir>/psf/spectre.out
# ✓ "spectre completes with 0 errors"
# ✗ "spectre terminated prematurely due to fatal error"
```

## Common errors in spectre.out

| Error | Fix |
|-------|-----|
| SFE-868: ADE-generated path `.../oa/lib/../models/...` | Patch input.scs: replace with direct absolute model path + `section=tt` — see `/spectre-netlist-gotchas` §8 |
| SFE-868: Cannot open input file (other) | Model path wrong — verify file exists |
| SFE-675: no valid section name | Empty section `""` in modelFile for .lib — remove it |
| SFE-1997: parameter not assigned | Set `desVar()` for the missing parameter |
| OSSHNL-116: Cannot descend into views | Subcell missing spectre view — remove instance or add view |

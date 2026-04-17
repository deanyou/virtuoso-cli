# Spectre Model Path: ADE OA Indirection vs Direct Invocation

## Source
Session investigation 2026-04-17: SFE-868 failure when running spectre directly on
ADE-generated input.scs for FT0001A_SH/5T_OTA_D_TO_S_sim.

## Summary
ADE-generated netlists use an OA-relative model path that only resolves correctly
when spectre runs with the `+adespetkn` token (ADE interactive mode); direct
`spectre input.scs` resolves the path as a plain filesystem path and fails.

## Content

### The Broken Path Pattern

ADE generates:
```
include "/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/oa/smic13mmrf_1233//../models/spectre/ms013_io33_v2p6_7p_spe.lib"
include "tt"
```

Filesystem resolution of `oa/smic13mmrf_1233//../models/`:
- `oa/smic13mmrf_1233/` → go up one level via `..` → `oa/`
- Result: `oa/models/spectre/` — **does NOT exist**

### Why It Works in ADE

When ADE runs spectre via `runSimulation`, the `+adespetkn=adespe` token enables
OA-aware path resolution. Spectre uses the Cadence OA database to locate libraries,
bypassing the filesystem `..` resolution issue.

### Symptom

```
Error found by spectre during circuit read-in.
    ERROR (SFE-868): "input.scs" 9: Cannot open the input file 
    '/foundry/.../oa/smic13mmrf_1233/../models/spectre/ms013_io33_v2p6_7p_spe.lib'
    ERROR (SFE-868): "input.scs" 10: Cannot open the input file 'tt'
spectre completes with 2 errors, 0 warnings, and 0 notices.
spectre terminated prematurely due to fatal error.
```

### Fix: Use the Absolute Direct Path

Replace the two include lines with the single direct path + section:
```
include "/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib" section=tt
```

Verify the path exists before patching:
```bash
ls /foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib
```

### Note: This Breaks on ADE Re-Netlist

After manually patching input.scs, calling `vcli sim netlist --recreate` will
overwrite the file with the ADE-generated (broken for direct use) version again.
The patch must be reapplied after each re-netlist when using direct spectre invocation.

### Also Note: `run()` Reports Success Even When Spectre Fails

`vcli sim run --analysis dc` reports success (execution_time ~3s) even when spectre
terminates with SFE-868. This is because `run()` returns the resultsDir path (non-nil)
once it initiates spectre, regardless of spectre's exit code. To verify success:
```bash
tail -3 <resultsDir>/psf/spectre.out
# Successful: "spectre completes with 0 errors"
# Failed: "spectre terminated prematurely due to fatal error"
```

And check that PSF data files exist:
```bash
ls <resultsDir>/psf/*.dc  # or *.ac, *.tran — absent on failure
```

## When to Use
- When running `spectre input.scs` directly (not through ADE/`vcli sim run`)
- When SFE-868 errors reference paths containing `oa/smic13mmrf_1233/../`
- When PSF directory only shows `artistLogFile`, `simRunData`, `spectre.out` after a sim run

## Context Links
- Related: [[spectre-netlist-gotchas]] — other SFE-* errors and their fixes

# Spectre vsource: AC Stimulus Parameter is `mag`, not `ac`

## Source
Session 2026-04-19: FT0001A_SH/Bandgap PSRR measurement.
SFE-30 warning when using `ac=1` on vsource; all AC results were 0+0j.

## Summary
Spectre's `vsource` element does not accept `ac=` — the small-signal stimulus parameter
is `mag=` (and `phase=`). Using `ac=1` silently ignores the parameter (SFE-30 warning)
and produces zero AC response across all nodes.

## Content

### Symptom

```
Warning from spectre during hierarchy flattening.
    WARNING (SFE-30): "input.scs" 148: Parameter `ac', specified for primitive
        `vsource', has been ignored because it is an invalid instance parameter.
```

All nodes in `ac1.ac` PSF file show `(0.000000000000000e+00 0.000000000000000e+00)`.
VDDA itself shows 0+0j even though it's the driven node.

### Root Cause

Spectre's `vsource` small-signal parameters (from `spectre -h vsource`):
```
97  mag=0 V     Small signal voltage.
98  phase=0 Deg Small signal phase.
```

SPICE uses `AC 1` on source elements; Spectre uses `mag=1` instead.

### Fix

```spectre
// Wrong (SPICE habit, silently ignored in Spectre):
VVDDA (VDDA 0) vsource dc=3.3 ac=1

// Correct for Spectre:
VVDDA (VDDA 0) vsource dc=3.3 mag=1
```

### Verification

After fixing, VDDA should appear as `1.0000` in the AC results:
```
"VDDA" (1.000000000000000e+00 0.000000000000000e+00)
```

### Also required: explicit `ac` analysis statement

Without an explicit `ac` analysis, Spectre only runs DC info passes.
Add after `simulatorOptions`:
```spectre
ac1 ac start=1 stop=100Meg dec=20
```

## When to Use
- When running AC analysis on a standalone Spectre netlist (not through ADE)
- When AC PSF results show all zeros for all nodes
- When adding PSRR, gain, or impedance measurements to a standalone netlist

## Context Links
- Based on: [[spectre-ade-model-path]] — other standalone netlist quirks
- Related: [[spectre-bandgap-dc-convergence]] — companion knowledge for standalone bandgap sim

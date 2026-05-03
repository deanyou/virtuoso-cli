# Maestro Simulator Mode — asiSetHighPerformanceOptionVal

## Source
2026-05-02: Arcadia-1/virtuoso-bridge-lite example 08_set_simulator_mode.py (PR #67).
The undocumented API for switching Spectre LX/MX/APS in Maestro.

## Summary
`+lx` flags and command env options are silently ignored; the actual Maestro API is
`asiSetHighPerformanceOptionVal` with `'uniMode` and `'spectreXPreset` parameters.

## Content

### Symptom of Wrong Approach
Using `+lx` or `spectre +preset=lx` in Maestro environment options:
- No error, no warning
- Simulation silently falls back to APS
- Runtime / accuracy unexpectedly wrong

### Correct API

```skill
; Get the test handle (required for both calls)
let((th)
  th = asiGetTest("TEST_NAME" "SESSION_NAME")
  ; Set the simulator family
  asiSetHighPerformanceOptionVal(th 'uniMode "Spectre X")
  ; Set the Spectre X preset
  asiSetHighPerformanceOptionVal(th 'spectreXPreset "LX"))
```

### Mode Table

| `'uniMode` | `'spectreXPreset` | Description |
|-----------|-----------------|-------------|
| `"Spectre"` | (omit) | Standard Spectre |
| `"APS"` | (omit) | APS (Cadence default) |
| `"Spectre X"` | `"LX"` | Spectre X — Light |
| `"Spectre X"` | `"MX"` | Spectre X — Medium |
| `"Spectre X"` | `"AX"` | Spectre X — Accurate |
| `"Spectre X"` | `"VX"` | Spectre X — Verification |
| `"Spectre X"` | `"CX"` | Spectre X — Custom |
| `"Spectre FX"` | (omit) | Fast X |

### Verification

```skill
; Should report +preset=lx (or whichever) in the resolved command line
maeGetCurrentNetlistOptionsValues(?session "SESSION_NAME" ?test "TEST_NAME")
```

### IC23 Notes
- `asiGetTest` is positional: `(testName sessionName)` — no keyword args
- Both `asiSetHighPerformanceOptionVal` calls are required when switching to Spectre X;
  omitting `'spectreXPreset` leaves the previous preset active

## When to Use
- Before any simulation that should use Spectre LX/MX for higher accuracy
- When a workflow needs to switch simulator mode programmatically between runs
- When simulation results seem to use APS despite config claiming otherwise

## Context Links
- Related: [[ic23-design-variable-namespaces]] — other IC23 API gotchas

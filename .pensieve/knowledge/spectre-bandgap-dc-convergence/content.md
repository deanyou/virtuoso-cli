# Spectre Bandgap DC Convergence: homotopy=all for Multi-Stable Circuits

## Source
Session 2026-04-19: FT0001A_SH/Bandgap standalone DC simulation.
Nodesets alone failed; `homotopy=all` in simulatorOptions resolved it.

## Summary
Bandgap circuits have two stable DC solutions. Spectre often converges to the
zero-current degenerate state. `nodeset` is insufficient — `homotopy=all` in
`simulatorOptions` enables source-stepping which finds the correct operating point.

## Content

### Symptom of Wrong Solution

| Node | Wrong (degenerate) | Correct |
|------|-------------------|---------|
| VBG | 3.14 V (≈ VDDA) | ~1.2 V |
| net0160 (NMOS mirror gate) | 0 V | ~0.6 V |
| net0165 (cascode gate) | 0 V | ~1.4 V |
| net0110 (PMOS diode) | 3.3 V (= VDDA) | ~2.6 V |
| start (startup node) | non-zero | ≈ 0 V |

Root cause: M5/M6 (NMOS current mirror bias) are off → no current in any branch
→ PMOS mirror collapses → VBG floats to VDDA via M33's body diode path.

### Why Nodesets Fail

`nodeset` provides initial hints but Spectre can deviate during Newton iterations.
The zero-current state is a true Jacobian fixed-point — the solver stays there even
with nodeset VBG=1.2 because the linearized system around that point still converges
to the wrong solution.

### Fix: homotopy=all

```spectre
simulatorOptions options psfversion="1.4.0" reltol=1e-3 vabstol=1e-6 \
    iabstol=1e-12 temp=27 tnom=27 gmin=1e-12 \
    homotopy=all \
    ...
```

`homotopy=all` enables all homotopy methods including source-stepping (ramps supply
from 0 → final value) which naturally starts the circuit from off-state and builds
up to correct operating point. Spectre log confirms: `homotopy = 5`.

Keep the `nodeset` lines — they help homotopy find the right branch faster:
```spectre
nodeset VBG=1.2 net0160=0.7 net0165=1.4
nodeset net068=2.5 net0110=2.0
nodeset INP=0.84 INN=0.84 net0100=0.79
```

### Also Required: Explicit dcop Analysis Statement

Without an explicit `dc` analysis statement, Spectre only runs info passes (no DC).
```spectre
dcop dc oppoint=rawfile annotate=status
```

### Verified DC Operating Point (FT0001A_SH/Bandgap, SMIC 0.13µm, TT/27°C)

| Node | Value |
|------|-------|
| VBG | 1.2112 V |
| INN / INP | 0.7593 V (VBE) |
| net0100 | 0.7043 V (Q0 emitter, 8× area) |
| net0160 | 0.5960 V (NMOS mirror bias) |
| net0165 | 1.4151 V (cascode bias) |
| net068 | 2.6081 V (PMOS mirror bias) |
| start | ~0 V (startup settled) |

### PSRR Results (same netlist, ac1 with mag=1 on VVDDA)

| Freq | PSRR |
|------|------|
| 1 Hz | 89.4 dB |
| 1 kHz | 62.0 dB |
| 10 kHz | 42.0 dB |
| 100 kHz | 22.0 dB |
| Unity-gain | ~1.26 MHz |

-20 dB/decade rolloff from ~100 Hz (single dominant pole). High-freq degradation
from C0 (MIM cap, net068↔VBG) providing direct VDDA→VBG coupling path at RF.

## When to Use
- When standalone Spectre bandgap DC converges to wrong solution (VBG ≈ VDDA)
- When `nodeset` alone doesn't fix convergence for strongly multi-stable circuits
- When starting from an Ocean-regenerated netlist (no ADE interactive session)

## Context Links
- Based on: [[smic-mosfet-terminal-order]] — fix CDF terminal order first
- Based on: [[spectre-vsource-ac-parameter]] — for follow-on PSRR analysis
- Related: [[spectre-ade-model-path]] — other ADE→standalone netlist issues

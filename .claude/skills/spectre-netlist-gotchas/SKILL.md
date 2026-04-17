---
name: spectre-netlist-gotchas
description: |
  Critical Spectre netlist-mode gotchas for standalone simulation (no Virtuoso ADE).
  Use when: (1) writing Spectre testbench .scs files with `simulator lang=spectre`,
  (2) noise analysis shows absurd values (megavolts of noise) — oprobe topology is wrong,
  (3) SFE-30 error on `ac=1` in vsource — use `mag=1` in native Spectre lang,
  (4) SFE-1997 error on `oprobe=<node_name>` — must be a circuit element not a node,
  (5) parsing PSF ASCII output from Spectre (dc_op.dc, ac_gain.ac, noise_an.noise),
  (6) phase margin calculation for inverting amplifier topologies,
  (7) slew rate measurement gives wrong result with small-signal step — need large signal,
  (8) ICMR from DC sweep — use transistor region fields, not CM gain (which is ≈ 0),
  (9) SFE-868 from ADE-generated netlist — oa/lib/../ model path only works in ADE interactive mode.
author: Claude Code
version: 1.0.0
date: 2026-04-07
---

# Spectre Netlist-Mode Gotchas

Lessons from standalone Spectre simulation (no ADE/Virtuoso), covering vsource syntax,
noise analysis setup, PSF ASCII parsing, and PM calculation for inverting topologies.

## 1. vsource AC Stimulus: `mag=` not `ac=`

### Problem
In native Spectre language (`simulator lang=spectre`), using `ac=1` on a vsource
causes **SFE-30** (invalid parameter).

### Root Cause
`ac=` is SPICE-compatibility syntax only. Native Spectre uses `mag=` for small-signal
AC amplitude.

### Fix
```spectre
// ✗ WRONG — SPICE-compat only
Vip (vip 0) vsource dc=0.9 ac=1

// ✓ CORRECT — native Spectre
Vip (vip 0) vsource dc=0.9 mag=1
```

### Discovery
Run `spectre -h vsource` to see all valid parameters. The relevant parameter is:
`mag=0 V (Small signal voltage)`.

---

## 2. Noise oprobe: Parallel Probe, Not Series

### Problem
Noise analysis with a series resistor as oprobe gives wildly wrong results (e.g.,
millions of nV/√Hz instead of hundreds). The simulation completes with 0 errors —
**no warning that the result is nonsensical**.

### Root Cause
When oprobe is a **series resistor** (e.g., 1Ω between vout and CL), Spectre measures
the **voltage across that resistor**, not the voltage at the output node.

At low frequencies, the CL capacitor is nearly open → almost no current flows through
the series resistor → voltage across it ≈ 0. The "gain" from input to oprobe becomes
tiny (e.g., 7.5e-9 V/V at 10 Hz instead of 118 V/V), and the input-referred noise
is inflated by 1/gain.

### Fix: Use a Large Parallel Resistor
```spectre
// ✗ WRONG — series probe, measures V across resistor (≈ 0 at low freq)
Rout_probe (vout vout_s) resistor r=1
CL_out (vout_s 0) capacitor c=CL
noise_an noise start=10 stop=100e3 dec=20 oprobe=Rout_probe iprobe=Vip

// ✓ CORRECT — parallel probe, negligible loading, measures full vout
CL_out (vout 0) capacitor c=CL
Rout_probe (vout 0) resistor r=1e12
noise_an noise start=10 stop=100e3 dec=20 oprobe=Rout_probe iprobe=Vip
```

The 1TΩ parallel resistor:
- Sees the full output voltage (vout) across it
- Negligibly loads the circuit (1TΩ ≫ Rout of OTA)
- Spectre correctly reports the output noise voltage spectral density

### Verification
Check the `"gain"` field in noise PSF output at low frequency. It should match your
AC gain (e.g., ~118 V/V for a 41.5 dB amplifier). If gain ≪ 1, the oprobe is wrong.

---

## 3. oprobe Must Be a Circuit Element (SFE-1997)

### Problem
Using a node name (e.g., `oprobe=vout`) causes error **SFE-1997**.

### Fix
oprobe requires a circuit **element** (resistor, port, etc.), not a node name.
Use the 1TΩ parallel resistor pattern from §2 above.

---

## 4. PSF ASCII Format (Spectre 23.1)

### DC (`dc_op.dc`)
```
VALUE
"M5:ids" "A" -9.966831545702541e-06 PROP(
"units" "A"
)
"vout" "V" 3.902670762490046e-01
```
Pattern: `"KEY" "UNIT" <float>` — key is lowercase `m5:ids`, not `M5:id`.

### AC (`ac_gain.ac`)
```
VALUE
"freq" 1.000000000000000e+00
"vout" (-1.188805328346738e+02 1.182042857455499e-03)
```
Pattern: `"freq" <float>` then `"vout" (<real> <imag>)` on the next relevant line.

### Noise (`noise_an.noise`)
```
VALUE
"freq" 1.000000000000000e+01
"M2" ( ... per-device noise contributions ... )
...
"out" 1.491e-03          ← output noise [V/√Hz]
"in"  1.254e-05          ← input-referred noise [V/√Hz]
"gain" 1.189e+02         ← transfer function [V/V]
```

**Critical**: The `"in"` field is in **V/√Hz** (not V²/Hz). To convert to nV/√Hz:
```python
noise_nv = in_value * 1e9    # ✓ CORRECT: V/√Hz → nV/√Hz

noise_nv = sqrt(in_value) * 1e9  # ✗ WRONG: treats as V²/Hz
```

Verify by checking the TYPE declaration section: `"in" "V/sqrt(Hz)"`.

---

## 5. Phase Margin for Inverting Amplifiers

### Problem
A 5T OTA (PMOS diff pair + NMOS mirror load) is inverting: vip↑ → vout↓.
The `atan2(im, re)` phase at DC is ≈ ±180°, not 0°. Naively computing
`PM = phase_at_GBW + 180°` gives values like 268° or -92°.

### Fix: Fold to (0°, 180°)
```python
pm_raw = phase_at_gbw + 180.0   # standard formula
pm = pm_raw % 360.0             # normalize to [0, 360)
if pm > 180.0:
    pm = 360.0 - pm             # fold to [0, 180]
```

For a single-pole system: PM ≈ 90° (correct). The folding handles both
`atan2` branch-cut cases (±180° at DC).

---

## 6. Slew Rate Measurement: Large-Signal Step Required

### Problem
Using a small step (e.g., 1 mV) in an open-loop OTA transient analysis gives a
misleadingly low slew rate (e.g., 0.08 V/µs instead of expected 6+ V/µs).

### Root Cause
Small-signal steps produce a **bandwidth-limited** response, not a slew-rate-limited
response. The output follows an exponential with time constant τ = 1/(2π·GBW), and
the max dV/dt is limited by bandwidth, not tail current.

For 1 mV input × 119 V/V gain = 119 mV output swing — the OTA never enters slew
limiting because the tail current can track the signal at all times.

Slew limiting occurs when the differential pair is fully steered to one side,
so the output current is clamped to I_tail.

### Fix
Use a step large enough to fully steer the diff pair (≫ 2×Vov of input pair):
```spectre
// ✗ WRONG — small signal, measures bandwidth not slew rate
Vip (vip 0) vsource dc=VICM mag=1 type=pulse \
  val0=VICM val1=VICM+1m delay=500n rise=1n fall=1n width=1u period=3u

// ✓ CORRECT — 100mV step fully steers PMOS pair (Vov ≈ 120mV)
Vip (vip 0) vsource dc=VICM mag=1 type=pulse \
  val0=VICM val1=VICM+100m delay=500n rise=1n fall=1n width=1u period=3u
```

### Verification
- Theoretical SR = I_tail / CL (e.g., 10 µA / 1 pF = 10 V/µs)
- Measured SR should be within ~50-100% of theoretical
- If SR ≪ theoretical, step amplitude is too small

---

## 7. ICMR from DC Sweep: Use Transistor Region, Not CM Gain

### Problem
Sweeping VICM (both inputs together) and computing dVout/dVICM gives
**common-mode gain ≈ 0**, not differential gain. This is correct behavior
for an open-loop OTA without CMFB, but useless for ICMR determination.

### Root Cause
ICMR is about the range where the OTA maintains **differential** gain,
but a CM sweep only exercises CM rejection. Vout stays nearly constant
throughout the valid ICMR range.

### Fix: Check Saturation Regions
In the DC sweep PSF, each transistor's `region` field is saved (with
`save M1:oppoint ...`). BSIM region codes:
- 0 = off, 1 = linear, 2 = saturation, 3 = subthreshold

ICMR = range where **all** transistors remain in saturation (region=2):
```python
# Parse "M1:region" ... "M5:region" from dc_sweep PSF
all_sat = [all(regions[f"M{j}"][i] == 2 for j in range(1, 6))
           for i in range(len(vicm_vals))]
icmr_lo = next(v for v, s in zip(vicm_vals, all_sat) if s)
icmr_hi = next(v for v, s in zip(reversed(vicm_vals), reversed(all_sat)) if s)
```

### Output Swing (from Same Data)
Output swing is determined by the output transistors' headroom:
- Vout_min = Vov_M4 (NMOS load enters linear)
- Vout_max = Vtail - |Vov_M2| (PMOS output enters linear)

Read `vdsat` from `dc_op.dc` oppoint data:
```python
vov_m4 = abs(op["m4:vdsat"])     # NMOS load overdrive
vov_m2 = abs(op["m2:vdsat"])     # PMOS diff pair overdrive
vtail  = op["vtail"]
output_swing = (vtail - vov_m2) - vov_m4
```

---

## 8. ADE-Generated Netlist: Model Path Fails for Direct Invocation (SFE-868)

### Problem
Running `spectre input.scs` directly on an ADE-generated netlist fails with SFE-868:
```
ERROR (SFE-868): Cannot open the input file
  '.../oa/smic13mmrf_1233/../models/spectre/ms013_io33_v2p6_7p_spe.lib'
ERROR (SFE-868): Cannot open the input file 'tt'
spectre terminated prematurely due to fatal error.
```

### Root Cause
ADE generates model include paths using an OA-relative pattern:
```
include ".../oa/smic13mmrf_1233//../models/spectre/ms013_io33_v2p6_7p_spe.lib"
include "tt"
```
The `..` goes up one level from `smic13mmrf_1233/` into `oa/` — resolving to
`oa/models/spectre/` which does not exist. When ADE runs spectre via `runSimulation`
with `+adespetkn=adespe`, it uses OA-aware path resolution that bypasses this
filesystem issue. Standalone spectre does plain filesystem `..` traversal and fails.

### Fix: Patch to Absolute Path Before Running
```bash
# Find the correct path (one level up from oa/):
# .../0.13um_1p3m_8k/oa/smic13mmrf_1233/../  →  .../0.13um_1p3m_8k/oa/  (wrong)
# correct:  .../0.13um_1p3m_8k/models/spectre/...

# Replace the two broken lines with a direct include + section:
sed -i \
  -e 's|include ".*/oa/smic13mmrf_1233//../models/spectre/ms013_io33_v2p6_7p_spe.lib"|include "/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib" section=tt|' \
  -e '/^include "tt"$/d' \
  input.scs
```

Or manually replace:
```
// ✗ ADE-generated (broken for standalone)
include ".../oa/smic13mmrf_1233//../models/spectre/ms013_io33_v2p6_7p_spe.lib"
include "tt"

// ✓ Direct path (works for standalone spectre)
include "/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib" section=tt
```

### Warning: Patch Is Overwritten on Re-Netlist
Every `vcli sim netlist --recreate` regenerates input.scs with the broken ADE path.
Reapply the patch after each re-netlist when using standalone spectre.

### Also: `vcli sim run` Reports Success Even When Spectre Fails This Way
`run()` returns the resultsDir (non-nil = "success" to Ocean) the moment spectre
is launched, before spectre's exit code is checked. Always verify:
```bash
tail -2 <resultsDir>/psf/spectre.out
# ✓ "spectre completes with 0 errors"
# ✗ "spectre terminated prematurely due to fatal error"
ls <resultsDir>/psf/*.dc 2>/dev/null || echo "NO DATA FILES — sim failed"
```

---

## Quick Reference: Spectre Help

```bash
spectre -h vsource    # all vsource parameters (mag, dc, type, ...)
spectre -h noise      # noise analysis options (oprobe, iprobe, ...)
spectre -h resistor   # resistor element parameters
```

## Notes
- These gotchas apply to **netlist mode** (`spectre file.scs`), not ADE-driven simulation
- In ADE/Virtuoso, noise setup is handled by the GUI and these issues don't arise
- The PSF ASCII format may vary slightly between Spectre versions; always verify with
  a small test file
- See also: `skill-shell-gotchas` for SKILL/IPC integration issues

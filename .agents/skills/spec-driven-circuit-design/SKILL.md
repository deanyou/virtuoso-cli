---
name: spec-driven-circuit-design
description: Spec-driven analog circuit design — decompose system specs into block/transistor-level requirements, validate feasibility via simulation, and iterate. Use when defining amplifier specs, checking if specs are achievable, decomposing system requirements to circuit blocks, or when the user says "design spec", "spec review", "is this spec feasible", or "spec breakdown".
allowed-tools: Bash(*/virtuoso *) Read Write Edit
---

# Spec-Driven Circuit Design

Systematic flow: System Spec → Block Decomposition → Feasibility Check → Circuit Design → Verification

## Design Flow

```
┌─────────────────────────────────────────────────────┐
│ 1. CAPTURE: Define system-level specs               │
│    (gain, GBW, noise, power, area, ...)             │
├─────────────────────────────────────────────────────┤
│ 2. SANITY CHECK: Validate spec consistency          │
│    (contradictions? physically impossible?)          │
├─────────────────────────────────────────────────────┤
│ 3. DECOMPOSE: System → Block → Transistor specs     │
│    (budget allocation across stages)                 │
├─────────────────────────────────────────────────────┤
│ 4. FEASIBILITY: Check against process limits        │
│    (gm/Id lookup → can we meet each sub-spec?)      │
├─────────────────────────────────────────────────────┤
│ 5. SIZE: gm/Id design per transistor                │
│    (W/L, bias conditions)                            │
├─────────────────────────────────────────────────────┤
│ 6. VERIFY: Simulate and compare to spec             │
│    (AC, tran, DC, noise, PVT)                        │
├─────────────────────────────────────────────────────┤
│ 7. ITERATE: Adjust specs or design if needed        │
│    (relax conflicting specs, re-size)                │
└─────────────────────────────────────────────────────┘
```

## 1. Spec Capture Template

```json
{
  "project": "LDO_error_amp",
  "system_spec": {
    "function": "Error amplifier for 1.2V LDO",
    "topology": null,
    "process": "smic13mmrf",
    "vdd": 3.3,
    "temperature_range": [-40, 125]
  },
  "performance_spec": {
    "gain_db":           {"min": 70,   "target": 80,   "unit": "dB"},
    "gbw_mhz":           {"min": 5,    "target": 10,   "unit": "MHz"},
    "phase_margin_deg":  {"min": 55,   "target": 65,   "unit": "°"},
    "cl_pf":             {"nom": 10,                   "unit": "pF"},
    "cmrr_db":           {"min": 60,                   "unit": "dB"},
    "psrr_db":           {"min": 60,                   "unit": "dB"},
    "slew_rate_Vus":     {"min": 5,                    "unit": "V/µs"},
    "noise_uVrms":       {"max": 50,                   "unit": "µV_rms", "bw": "10Hz-100kHz"},
    "input_offset_mV":   {"max": 5,                    "unit": "mV"},
    "power_uW":          {"max": 200,                  "unit": "µW"},
    "output_swing_V":    {"min": 0.2, "max": 3.1,     "unit": "V"},
    "input_cm_range_V":  {"min": 0.5, "max": 2.5,     "unit": "V"},
    "area_um2":          {"max": 5000,                 "unit": "µm²"}
  },
  "constraints": {
    "supply_current_budget_uA": 60,
    "num_stages": null,
    "compensation": null,
    "notes": "Must drive 10pF capacitive load from LDO pass device gate"
  }
}
```

## 2. Spec Sanity Check

Before designing, validate that specs don't conflict:

### Fundamental Tradeoffs

```
CHECK 1: Gain × Bandwidth
  Single-stage max: gain ≈ gm/gds, fT ≈ gm/(2πCgs)
  → gain × BW ≈ fT/A_v_per_stage
  Rule: If gain_db > 50 AND gbw > 100MHz → need multi-stage
  
CHECK 2: Slew Rate vs Power
  SR = I_tail / CL
  → I_min = SR × CL = 5V/µs × 10pF = 50µA
  → P_min = VDD × I_min = 3.3V × 50µA = 165µW
  Spec says P_max = 200µW → feasible (35µW margin) ✓
  
CHECK 3: Noise vs Power
  Vn² ≈ (16kT)/(3·gm) over BW
  → gm_min for noise = 16kT·BW / (3·Vn²)
  → requires minimum current budget

CHECK 4: Gain vs Output Swing
  High gain → cascode → less swing
  If output_swing > VDD - 4×Vov → cannot use simple cascode
  
CHECK 5: CMRR vs Input Range
  Rail-to-rail input → NMOS + PMOS pair → CMRR harder
  Folded-cascode → better CMRR but limited input range

CHECK 6: Speed vs Area
  Large W for speed → more area
  Area_max = 5000µm² → limits total W
```

### Topology-Aware Power Calculation

```
Telescopic OTA:
  I_total = I_tail
  Branches: 1 (tail → diff pair → cascode load)

Folded-Cascode OTA:
  I_total = I_tail + 2 × I_fold
  = 2×Id_input + 2×I_fold
  Typically I_fold ≈ 1.2 × Id_input
  → I_total ≈ 4.4 × Id_input

Two-Stage Miller:
  I_total = I_tail_1 + I_stage2
  = 2×Id_input + Id_output
  Typically Id_output ≈ 3-5 × Id_input (for PM)

Three-Stage:
  I_total = I_1 + I_2 + I_3 + bias
```

Use `virtuoso design size` to compute per-transistor sizing:
```bash
virtuoso design size --gmid 14 --l 500e-9 --gm <gm_req> --pdk <pdk>
```

### Automated Feasibility via Virtuoso-CLI

```bash
# Quick feasibility: check if gm/gain/fT limits support the spec
# Using process lookup table

# 1. What gain can L=500n achieve?
virtuoso skill exec 'desVar("L" 500e-9) desVar("VGS" 0.5)' 
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 60
virtuoso sim measure --analysis dcOp \
  --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))' \
  --expr 'value(getData("/NM0:gds" ?result "dcOpInfo"))'
# → single-stage gain = gm/gds ≈ 140 → 43dB
# → need 2 stages for 70dB (43+43 > 70 ✓)

# 2. What fT at this bias point?
virtuoso sim measure --analysis dcOp \
  --expr 'value(getData("/NM0:cgs" ?result "dcOpInfo"))'
# → fT = gm/(2π·Cgs) → check if > GBW requirement
```

## 3. Spec Decomposition

### Two-Stage Miller OTA Example

```
System Spec:  Gain=70dB, GBW=10MHz, CL=10pF, PM=60°
─────────────────────────────────────────────────────

Stage 1 (Diff Input + Cascode Load):
  ├── gain₁ ≥ 35dB (45 V/V)
  ├── gm₁ = 2π × GBW × Cc ≈ 2π × 10M × 3p = 188µS
  ├── Input pair: gm/Id=12-15, L=500n (noise + gain)
  ├── Load mirror: gm/Id=6-8, L=500n (low noise)
  └── Tail source: I_tail = 2 × Id_input

Stage 2 (Common Source):
  ├── gain₂ ≥ 35dB (45 V/V)  
  ├── gm₂ = 2π × GBW × CL = 2π × 10M × 10p = 628µS
  │   (for PM: gm₂ > 2.2 × gm₁ × CL/Cc)
  ├── Output device: gm/Id=8-10, L=300n (speed)
  └── Bias load: gm/Id=6, L=1µm (gain)

Compensation:
  ├── Cc = 0.22 × CL = 2.2pF (rule of thumb)
  │   Or: Cc > gm₁/(2π × GBW) to set dominant pole
  ├── Rz = 1/gm₂ (zero cancellation)
  └── PM ≈ 90° - arctan(GBW/fp₂) 
      fp₂ = gm₂/CL → check PM > 60°

Current Budget:
  ├── Stage 1: I_tail = 2 × 15.7µA = 31.4µA
  ├── Stage 2: I₂ = gm₂/(gm/Id₂) = 628/10 = 62.8µA
  ├── Bias: ~5µA
  └── Total: ~100µA → P = 3.3V × 100µA = 330µW
      ⚠ Exceeds 200µW budget!

→ DECISION: Relax GBW to 5MHz or increase power budget
```

### Decomposition Rules

| System Spec | Decomposition Rule |
|-------------|-------------------|
| Total Gain | Distribute across stages: A_total = A₁ × A₂ × ... |
| GBW | Sets gm₁ via Cc (Miller) or CL (single-stage) |
| Phase Margin | Determines Cc/Rz and gm₂/gm₁ ratio |
| Slew Rate | SR = I_tail/Cc (differential) or I/CL (output) |
| Noise | Input pair dominates → size gm₁, W₁ |
| CMRR | Tail source output impedance + matching |
| PSRR | Cascode + bias rejection ratio |
| Power | Sum of all branch currents × VDD |
| Swing | Limits Vov → constrains gm/Id range |
| Area | Sum of all W×L → constrains W choices |

## 4. Feasibility Matrix

After decomposition, build a feasibility matrix:

```
Transistor  Role          gm/Id  L      W      Id      Vov    OK?
──────────────────────────────────────────────────────────────────
M1,M2       input_pair    14     500n   2.7µm  13.5µA  92mV   ✓
M3,M4       active_load   7      500n   1.2µm  13.5µA  242mV  ✓
M5          tail_source   6      1µm    2.0µm  27µA    280mV  ✓
M6          output_gm     10     300n   6.0µm  50µA    160mV  ✓
M7          output_load   6      1µm    3.0µm  50µA    280mV  ✓
──────────────────────────────────────────────────────────────────
Total area: 2×(2.7×0.5) + 2×(1.2×0.5) + (2×1) + (6×0.3) + (3×1)
          = 2.7 + 1.2 + 2.0 + 1.8 + 3.0 = 10.7 µm²  ✓ (<5000)
Total Id:   27 + 50 + 5 = 82µA → P = 271µW  ⚠ (>200µW)
```

## 5. Simulation Verification Plan

```bash
# 1. DC: Operating point check
virtuoso sim setup --lib myLib --cell myOTA_TB
virtuoso sim run --analysis dc --param saveOppoint=t
virtuoso sim measure --analysis dcOp \
  --expr 'value(VDC("/OUT"))' \
  --expr 'value(IDC("/M1/D"))'

# 2. AC: Gain & bandwidth
virtuoso sim run --analysis ac --start 1 --stop 1e10 --dec 20
virtuoso sim measure --analysis ac \
  --expr 'dB20(value(VF("/OUT") 1))'               # DC gain
  --expr 'cross(dB20(VF("/OUT")) 0 1 "falling")'   # GBW
  --expr 'value(phase(VF("/OUT")) <ugb>) + 180'     # PM

# 3. Transient: Slew rate & settling
virtuoso sim run --analysis tran --stop 20u
virtuoso sim measure --analysis tran \
  --expr 'ymax(VT("/OUT"))' \
  --expr 'slewRate(VT("/OUT"))'

# 4. Noise
virtuoso sim run --analysis noise --start 10 --stop 100e3 --dec 20
# integrated noise = sqrt(integral of Sn(f))

# 5. PVT corners
virtuoso sim corner --file corners.json
```

## 6. Spec Iteration Decision Tree

```
Simulation vs Spec comparison:
  
IF gain < spec:
  → Increase L (more gain per stage)
  → Add cascode (doubles gain per stage)
  → Add another stage
  
IF GBW < spec:
  → Increase gm₁ (more current or lower gm/Id)
  → Reduce Cc (but check PM)
  → Reduce parasitic capacitance (smaller L)
  
IF PM < spec:
  → Increase Cc (slower but more stable)
  → Increase gm₂/gm₁ ratio
  → Add Rz nulling resistor
  
IF SR < spec:
  → Increase tail current
  → Reduce Cc
  
IF noise > spec:
  → Increase gm₁ (larger W₁ or more current)
  → Increase L₁ (less 1/f noise)
  → Choose PMOS input pair (less 1/f)
  
IF power > spec:
  → Reduce GBW target (relax speed)
  → Use subthreshold (higher gm/Id)
  → Reduce number of branches
  
IF area > spec:
  → Reduce W (accept lower gm/Id)
  → Use minimum L where possible
  → Share bias structures
  
IF multiple specs conflict:
  → Present tradeoff to user with quantified options
  → "Relaxing GBW from 10MHz to 5MHz saves 40µW and 5µm²"
```

## Spec Templates for Common Circuits

### LDO Error Amplifier
```
Gain: 60-80dB | GBW: 1-10MHz | PM: >60° | CL: 5-50pF
PSRR: >50dB | Noise: <50µV | Power: <100µW
Key: High PSRR, moderate speed, drives pass device gate
```

### ADC Front-End (SHA)
```
Gain: >60dB | GBW: >100MHz | Settling: <ns | SR: >100V/µs
Noise: <100µV | Power: <mW | Linearity: >10-bit
Key: Speed and linearity dominate, power secondary
```

### Sensor Readout (Instrumentation)
```
Gain: 40-60dB | GBW: 100kHz-1MHz | CMRR: >100dB
Noise: <1µV/√Hz | Power: <10µW | Offset: <10µV
Key: Ultra-low noise, high CMRR, low power
```

### Comparator
```
Propagation delay: <ns | Sensitivity: <mV | Power: <µW
Metastability: recovery <1ns | Kickback: <mV
Key: Speed and sensitivity, not linear gain
```

---
name: amp-copilot
description: Amplifier design copilot — topology selection, sizing via gm/Id lookup tables, PVT corner validation, and process-portable design. Use when designing amplifiers (OTA, opamp, comparator), selecting topology from specs, sizing transistors, or characterizing a new process node. Also triggers on keywords like amplifier, OTA, opamp, gain-bandwidth, CMRR, PSRR, slew rate.
allowed-tools: Bash(*/virtuoso *) Read Write
---

# Amplifier Copilot

Systematic amplifier design flow: specs → topology → gm/Id sizing → simulation → PVT validation.

Inspired by [Amplifier-Copilot](https://github.com/AmpCopilot/Amplifier-Copilot) (25 topologies, 7400+ pre-characterized designs, 4 process nodes).

## Design Flow Overview

```
  ┌─────────────────────────────────────────────┐
  │  1. SPEC CAPTURE                            │
  │     Gain, GBW, CL, VDD, PM, CMRR, ...      │
  ├─────────────────────────────────────────────┤
  │  2. TOPOLOGY SELECTION                      │
  │     Match specs → best topology             │
  ├─────────────────────────────────────────────┤
  │  3. PROCESS CHARACTERIZATION (gm/Id)        │
  │     Sweep VGS × L → lookup tables           │
  │     Store in process_data/<pdk>/             │
  ├─────────────────────────────────────────────┤
  │  4. TRANSISTOR SIZING                       │
  │     Role-based gm/Id → W/L per device       │
  ├─────────────────────────────────────────────┤
  │  5. SIMULATION & VERIFICATION               │
  │     DC → AC → tran → PVT corners           │
  └─────────────────────────────────────────────┘
```

## 1. Spec Capture

Gather design requirements in structured format:

```json
{
  "name": "bandgap_ota",
  "topology": null,
  "process": "smic13mmrf",
  "vdd": 1.2,
  "specs": {
    "gain_db": {"min": 60, "target": 70},
    "gbw_mhz": {"min": 5, "target": 10},
    "phase_margin_deg": {"min": 60},
    "cl_pf": 5,
    "cmrr_db": {"min": 60},
    "psrr_db": {"min": 50},
    "slew_rate_Vus": {"min": 5},
    "power_uw": {"max": 200},
    "input_range": "rail-to-rail",
    "output_range": {"min_headroom_mv": 200}
  }
}
```

## 2. Topology Selection

### 25 Amplifier Topologies (from Amplifier-Copilot database)

**Single-stage:**
| Topology | Gain | Speed | Area | Use case |
|----------|------|-------|------|----------|
| Telescopic | 40-60dB | High | Small | High-speed, limited swing |
| Folded-Cascode | 50-70dB | Med-High | Med | General purpose |
| Recycling FC | 60-80dB | High | Med | Enhanced speed |
| Current-Mirror | 40-50dB | Med | Small | Simple loads |

**Two-stage:**
| Topology | Gain | Speed | Area | Use case |
|----------|------|-------|------|----------|
| Miller OTA | 60-80dB | Med | Med | General purpose |
| Ahuja Comp | 70-90dB | Med-High | Med | Better PSRR |
| Indirect Comp | 70-90dB | High | Med | High speed two-stage |

**Multi-stage / Special:**
| Topology | Gain | Speed | Area | Use case |
|----------|------|-------|------|----------|
| 3-stage NMC | 80-120dB | Low | Large | Ultra-high gain |
| Rail-to-Rail | 50-70dB | Med | Large | Full swing I/O |
| Class-AB | 50-70dB | Med | Med | High drive |
| Comparator | - | Very High | Small | Decision circuit |

### Selection Logic

```
IF gain > 80dB → multi-stage (Miller, NMC)
IF gain 50-80dB AND gbw > 100MHz → recycling FC / indirect comp
IF gain 50-80dB AND gbw < 100MHz → folded-cascode / Miller
IF gain < 50dB → telescopic / current-mirror
IF input_range == "rail-to-rail" → rail-to-rail topology
IF output swing > VDD-400mV → class-AB output
IF power < 10µW → subthreshold (gm/Id > 20)
```

## 3. Process Characterization (gm/Id Lookup Tables)

### Process Data Directory Structure

Store per-process characterization data for reuse across designs:

```
process_data/
├── smic13mmrf/
│   ├── config.json          # Process metadata
│   ├── nmos_lookup.json     # NMOS gm/Id tables
│   ├── pmos_lookup.json     # PMOS gm/Id tables
│   └── corners.json         # PVT corner definitions
├── tsmc22ull/
│   ├── config.json
│   ├── nmos_lookup.json
│   ├── pmos_lookup.json
│   └── corners.json
└── tsmc65/
    └── ...
```

### Process Config (`config.json`)

```json
{
  "name": "smic13mmrf",
  "node": "130nm",
  "vdd_options": [1.2, 3.3],
  "nmos_device": "n12",
  "pmos_device": "p12",
  "l_min": 120e-9,
  "l_values": [200e-9, 300e-9, 500e-9, 1e-6, 2e-6],
  "w_default": 1e-6,
  "vgs_range": [0.2, 1.2],
  "vgs_step": 0.05,
  "model_file": "/foundry/smic/013mmrf/.../ms013_io33_v2p6_7p_spe.lib",
  "model_sections": ["tt", "res_tt", "dio_tt", "bjt_tt", "mim_tt"],
  "testbench": {
    "lib": "FT0001A_SH",
    "nmos_cell": "gmid",
    "pmos_cell": "gmid_pmos",
    "nmos_inst": "/NM0",
    "pmos_inst": "/PM0"
  }
}
```

### Lookup Table Format (`nmos_lookup.json`)

```json
{
  "process": "smic13mmrf",
  "device": "n12",
  "w": 1e-6,
  "data": [
    {
      "l": 500e-9,
      "points": [
        {"vgs":0.35, "gmid":20.68, "gain":184.4, "id":0.88e-6,  "idw":0.88, "vov":-0.008, "ft":1.037e9, "vth":0.358, "gds":4.77e-9, "cgs":5.4e-14},
        {"vgs":0.40, "gmid":17.38, "gain":179.5, "id":2.29e-6,  "idw":2.29, "vov":0.042,  "ft":1.793e9, "vth":0.358, "gds":12.8e-9, "cgs":6.2e-14},
        {"vgs":0.45, "gmid":13.93, "gain":163.3, "id":5.01e-6,  "idw":5.01, "vov":0.092,  "ft":2.752e9, "vth":0.358, "gds":42.7e-9, "cgs":8.1e-14},
        {"vgs":0.50, "gmid":11.00, "gain":140.1, "id":9.31e-6,  "idw":9.31, "vov":0.142,  "ft":3.765e9, "vth":0.358, "gds":66.4e-9, "cgs":12e-14},
        {"vgs":0.60, "gmid":7.19,  "gain":95.0,  "id":22.64e-6, "idw":22.64,"vov":0.242,  "ft":5.633e9, "vth":0.358, "gds":171e-9,  "cgs":18e-14}
      ]
    },
    {
      "l": 200e-9,
      "points": [...]
    }
  ]
}
```

### Characterization Script (via virtuoso-cli)

To characterize a new process, run this automated flow:

```bash
# 1. Set up simulation environment
virtuoso sim setup --lib <LIB> --cell <GMID_TB> --view schematic
virtuoso skill exec 'resultsDir("/tmp/process_char")'
virtuoso skill exec 'modelFile(list("<model_path>" "tt") ...)'

# 2. Sweep VGS × L and extract oppoint
for L in 200e-9 300e-9 500e-9 1e-6 2e-6; do
  for VGS in $(seq 0.20 0.05 1.20); do
    virtuoso skill exec "desVar(\"L\" $L)"
    virtuoso skill exec "desVar(\"VGS\" $VGS)"
    virtuoso skill exec 'run()' --timeout 60
    
    # Extract all oppoint parameters
    virtuoso sim measure --analysis dcOp \
      --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))' \
      --expr 'value(getData("/NM0:ids" ?result "dcOpInfo"))' \
      --expr 'value(getData("/NM0:gds" ?result "dcOpInfo"))' \
      --expr 'value(getData("/NM0:vth" ?result "dcOpInfo"))' \
      --expr 'value(getData("/NM0:cgs" ?result "dcOpInfo"))' \
      --format json
  done
done

# 3. Parse results into lookup table JSON
# 4. Save to process_data/<pdk>/nmos_lookup.json
```

## 4. Transistor Sizing

### Role-Based gm/Id Selection

Each transistor in the amplifier has a role that determines its optimal gm/Id:

```
def size_transistor(role, spec, lookup_table):
    # Select gm/Id based on role
    gmid_target = {
        "input_pair":    12-15,  # balance noise, gain, speed
        "cascode":       8-12,   # moderate Vov for headroom
        "current_mirror": 5-8,   # low gm → low noise contribution
        "tail_source":    5-8,   # matching > speed
        "output_stage":   8-12,  # balance swing and drive
        "high_swing":    15-20,  # minimize Vov for swing
        "high_speed":     4-6,   # maximize fT
    }[role]
    
    # Calculate Id from gm requirement
    Id = gm_required / gmid_target
    
    # Lookup Id/W from table at chosen L
    IdW = interpolate(lookup_table, gmid_target, L)
    
    # Calculate W
    W = Id / IdW
    
    return W, L, Id, gmid_target
```

### Two-Stage Miller OTA Sizing Example

```
Device   Role           gm/Id   L      W       Id
─────────────────────────────────────────────────────
M1,M2    input_pair     14      500n   2.7µm   13.5µA
M3,M4    mirror_load    6       500n   1.0µm   13.5µA  
M5       tail_source    6       1µm    2.0µm   27µA
M6       output_gm      10      300n   8.0µm   50µA
M7       output_load     6       1µm    3.0µm   50µA
Cc       compensation   -       -      1.5pF   -
Rc       zero-nulling   -       -      2kΩ     -
```

## 5. PVT Corner Validation

### Standard Corner Set

```json
{
  "corners": [
    {"name": "tt_25",   "section": "tt", "temp": 25,   "vdd_scale": 1.0},
    {"name": "ss_125",  "section": "ss", "temp": 125,  "vdd_scale": 0.9},
    {"name": "ff_m40",  "section": "ff", "temp": -40,  "vdd_scale": 1.1},
    {"name": "sf_25",   "section": "sf", "temp": 25,   "vdd_scale": 1.0},
    {"name": "fs_25",   "section": "fs", "temp": 25,   "vdd_scale": 1.0}
  ]
}
```

### Validation Metrics (13 performance parameters)

```bash
# After simulation at each corner, extract:
virtuoso sim measure --analysis ac \
  --expr 'dB20(value(VF("/OUT") 1))'                    # DC gain
  --expr 'cross(dB20(VF("/OUT")) 0 1 "falling")'        # UGB
  --expr 'value(phase(VF("/OUT")) <ugb>)+180'            # Phase margin

virtuoso sim measure --analysis tran \
  --expr 'slewRate(VT("/OUT") 10 90 "rising")'           # Slew rate+
  --expr 'slewRate(VT("/OUT") 90 10 "falling")'          # Slew rate-
  --expr 'settlingTime(VT("/OUT") <final> 0.1)'          # 0.1% settling

# CMRR (needs dedicated testbench)
# PSRR (needs dedicated testbench)
# Input offset, noise, power
```

### Pass/Fail Report

```
                    tt_25   ss_125  ff_m40  sf_25   fs_25   SPEC
Gain (dB)           72.1    68.3    74.8    70.2    71.5    >60 ✓
GBW (MHz)           8.2     5.8     11.3    7.9     8.5     >5  ✓
PM (°)              65      71      58      63      67      >60 ⚠
SR+ (V/µs)          8.1     5.2     12.4    7.8     8.3     >5  ✓
Power (µW)          32      28      38      31      33      <50 ✓
```

## Process Portability

### Porting to a New Process

When switching to a new PDK (e.g., TSMC 22nm):

1. **Create testbench**: Single NMOS + PMOS with VGS/L as design variables
2. **Run characterization**: `virtuoso process char --lib myLib --cell gmid_n --inst /NM0 --type nmos --output process_data/tsmc22ull`
3. **Save lookup tables**: Auto-generated at `process_data/tsmc22ull/nmos_lookup.json`
4. **Re-size**: `virtuoso design size --gmid 14 --l 100e-9 --gm 188e-6 --pdk tsmc22ull`
5. **Validate**: Run PVT corners with new models

### Quick Validation with Verilog-A Ideal Model

Before transistor-level design, validate specs with an ideal behavioral model.
Use the `/veriloga` skill to create an ideal opamp with target specs:

```bash
# Create ideal opamp with your target gain/GBW/SR
# Then simulate to verify specs are achievable with the topology
# This catches spec conflicts before investing in transistor sizing
```

The **gm/Id targets remain the same** across processes — only the lookup tables (Id/W, gain, fT vs gm/Id) change. This is the core portability advantage.

### Key Process-Dependent Parameters

| Parameter | Changes with process? | Impact |
|-----------|----------------------|--------|
| gm/Id target | No | Design intent unchanged |
| Id/W at given gm/Id | **Yes** | W changes |
| Self-gain at given gm/Id | **Yes** | May need different L |
| fT at given gm/Id | **Yes** | Speed limit changes |
| Vth | **Yes** | Bias point shifts |
| Min L | **Yes** | L floor changes |

## Quick Reference: SKILL Oppoint Parameters

```
gm, gds, ids, vth, vdsat, cgs, cgd, cgg, gmbs
self_gain (= gm/gds), gmoverid (= gm/id), ft, region
```

Access via: `getData("/INST:param" ?result "dcOpInfo")`
Or: `OS("/INST" "param")` for waveform data

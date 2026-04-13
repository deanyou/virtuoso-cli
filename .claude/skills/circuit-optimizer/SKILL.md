---
name: circuit-optimizer
description: Bayesian optimization for circuit auto-tuning — closed-loop optimizer where Claude acts as the BO engine. Sweeps gm/Id + L parameters, runs Spectre, scores against specs, and iterates. Supports progressive PVT corners. Use when optimizing circuit sizing, auto-tuning amplifier parameters, or running design-space exploration. Triggers on "optimize", "auto-tune", "bayesian", "find best sizing".
allowed-tools: Bash(*/virtuoso *) Read Write
---

# Circuit Optimizer (Bayesian)

Closed-loop circuit optimization: Claude as surrogate model, virtuoso-cli as simulator.

See full design: `docs/plans/2026-04-05-bayesian-optimization-design.md`

## When to Use

- After initial sizing (from amp-copilot or manual) needs refinement
- When multiple specs conflict and manual iteration is tedious
- When PVT robustness is needed
- "Optimize my OTA", "auto-tune this circuit", "find best sizing for these specs"

## Prerequisites

1. A **testbench** exists in Virtuoso with parameterized device sizes (desVar)
2. **gm/Id lookup data** exists for the process (`process_data/<pdk>/`)
3. **Simulation setup** works (sim-setup skill has been run at least once)

## Step-by-Step Execution

### Step 1: Build Problem Definition

Gather from user or introspect from schematic:

```json
{
  "optimization": {
    "testbench": {"lib": "LIB", "cell": "CELL_TB", "view": "schematic"},
    "parameters": [
      {"name": "gmid_M1", "type": "gmid", "device": "input_pair", "range": [8, 22], "init": 14},
      {"name": "L_M1",    "type": "L",    "device": "input_pair", "range": [300e-9, 2e-6], "init": 500e-9},
      {"name": "gmid_M3", "type": "gmid", "device": "active_load", "range": [5, 15], "init": 7},
      {"name": "L_M3",    "type": "L",    "device": "active_load", "range": [300e-9, 2e-6], "init": 500e-9},
      {"name": "Cc",      "type": "comp", "range": [0.5e-12, 5e-12], "init": 2.2e-12}
    ],
    "specs": {
      "gain_db":  {"min": 70, "target": 80, "weight": 1.0},
      "gbw_hz":   {"min": 5e6, "target": 10e6, "weight": 1.0},
      "pm_deg":   {"min": 55, "target": 65, "weight": 0.8},
      "power_w":  {"max": 200e-6, "target": 100e-6, "weight": 0.5},
      "sr_Vus":   {"min": 5, "weight": 0.3}
    },
    "measurements": {
      "gain_db":  {"analysis": "ac",   "expr": "value(dB20(VF(\"/OUT\")) 1)"},
      "gbw_hz":   {"analysis": "ac",   "expr": "cross(dB20(VF(\"/OUT\")) 0 1 \"falling\")"},
      "pm_deg":   {"analysis": "ac",   "expr": "value(phase(VF(\"/OUT\")) cross(dB20(VF(\"/OUT\")) 0 1 \"falling\")) + 180"},
      "power_w":  {"analysis": "dcOp", "expr": "value(IDC(\"/V0/PLUS\")) * 3.3"},
      "sr_Vus":   {"analysis": "tran", "expr": "slewRate(VT(\"/OUT\")) / 1e6"}
    },
    "corners": {
      "phase1": [{"model": "tt", "temp": 27}],
      "phase2": [{"model": "tt", "temp": 27}, {"model": "ff", "temp": 27}, {"model": "ss", "temp": 27}],
      "phase3": [{"model": "tt", "temp": 27}, {"model": "ff", "temp": -40}, {"model": "ss", "temp": 125}]
    },
    "budget": {"max_iterations": 50, "max_sim_time_min": 120}
  }
}
```

**Parameter types:**
- `gmid`: gm/Id ratio. Range typically [5, 25]. Converted to W via lookup.
- `L`: channel length. Range [Lmin, 2u].
- `comp`: compensation element (Cc, Rz). Direct desVar.

**Spec fields:**
- `min`: hard lower bound (constraint)
- `max`: hard upper bound (constraint)
- `target`: optimization goal (only matters after feasibility)
- `weight`: relative importance among targets

### Step 2: Convert gm/Id to W

For each `gmid` parameter, compute W using the gm/Id lookup table:

```bash
# Look up Id/(W/L) at the given gm/Id and L
virtuoso skill exec 'RB__gmid_target = 14.0' --format json
virtuoso skill exec 'RB__L = 500e-9' --format json

# From lookup table: find Id_norm = Id/(W/L) at this gm/Id and L
# Then: gm = gmid * Id, and Id = gm / gmid
# W = Id / (Id_norm * L)  ... or use the pre-built lookup

# Using amp-copilot process data:
# Read process_data/smic13mmrf/nmos_gmid_lookup.json
# Interpolate to get Id_WL at (gmid=14, L=500n)
# Then compute: gm_required -> Id = gm/gmid -> W = Id / (Id_WL / L)
```

For the optimizer, the flow is:
1. User specifies required `gm` for each device (from spec decomposition)
2. Optimizer tunes `gmid` and `L`
3. W = gm / (gmid * Id_norm(gmid, L))

If `gm` is not fixed, use current budget: `Id = I_budget / num_branches`, then `W = Id / Id_norm`.

### Step 3: Run One Iteration

```bash
# 1. Set design variables
virtuoso skill exec 'desVar("W_M1" 3.1e-6)' --format json
virtuoso skill exec 'desVar("L_M1" 500e-9)' --format json
virtuoso skill exec 'desVar("W_M3" 1.4e-6)' --format json
# ... all parameters

# 2. Run required analyses
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 120 --format json
virtuoso sim run --analysis ac --start 1 --stop 1e10 --dec 20 --timeout 120 --format json
virtuoso sim run --analysis tran --stop 20u --timeout 120 --format json

# 3. Measure all specs
virtuoso sim measure --analysis ac \
  --expr 'value(dB20(VF("/OUT")) 1)' \
  --expr 'cross(dB20(VF("/OUT")) 0 1 "falling")' \
  --format json

virtuoso sim measure --analysis dcOp \
  --expr 'value(IDC("/V0/PLUS")) * 3.3' \
  --format json

# 4. For Phase 2/3: repeat with different model files
virtuoso skill exec 'modelFile(list("/path/models.lib" "ff"))' --format json
# ... re-run and re-measure
```

### Step 4: Score the Result

```
SCORING FUNCTION (compute in Claude, not SKILL):

1. Feasibility check:
   For each spec with min/max:
     violation = max(0, spec_min - measured) / spec_min    # undershoot
               + max(0, measured - spec_max) / spec_max    # overshoot
   feasibility_cost = sum of all violations

2. If infeasible (feasibility_cost > 0):
   cost = 1000 + feasibility_cost
   
3. If feasible:
   target_cost = sum(weight_i * |1 - measured_i / target_i|) for specs with targets
   cost = target_cost

4. For Phase 2/3 (multi-corner):
   cost = max(cost across all corners)
```

### Step 5: Update History

Write/update the history JSON file:

```bash
# History lives at: process_data/<pdk>/opt_history/<cell>_<timestamp>.json
# Append new iteration to history array
# Update best if this iteration has lower cost
```

History JSON structure:
```json
{
  "meta": {"cell": "...", "pdk": "...", "phase": 1, "iteration": 12, "status": "running"},
  "problem": {"...problem definition..."},
  "best": {
    "iteration": 9,
    "params": {"gmid_M1": 13.2, "L_M1": 6.5e-7},
    "derived": {"W_M1": 3.1e-6, "Id_M1": 14.2e-6},
    "results": {"tt_27": {"gain_db": 74.2, "gbw_hz": 8.1e6}},
    "cost": 0.12,
    "feasible": true
  },
  "history": [
    {"iter": 0, "phase": 1, "params": {}, "results": {}, "cost": 1000.35, "feasible": false, "note": "gain below min"}
  ]
}
```

### Step 6: Suggest Next Point (Surrogate Reasoning)

This is where Claude acts as the Bayesian optimizer. Follow this protocol:

**Every iteration, reason through:**

1. **Review history** — Sort by cost. Identify top 3-5 points.

2. **Identify trends** — Which parameters improved results when changed?
   - "Increasing L_M1 from 300n to 500n improved gain by 8dB"
   - "gmid_M3 below 6 always causes PM violation"

3. **Choose strategy** (3:1 exploit:explore ratio):
   - **Exploit** (iterations 0,1,2, 4,5,6, 8,9,10, ...): Perturb best point.
     Pick 1-3 parameters that most correlate with improvement.
     Adjust by 10-20% in the promising direction.
   - **Explore** (iterations 3, 7, 11, ...): Sample a point far from all visited.
     Use midpoints of unvisited parameter subregions.

4. **Bound check** — Ensure all parameters within range.

5. **Physical check** — gm/Id in [5, 25], L >= Lmin, W > 0.

### Step 7: Decide Continue/Phase-Up/Stop

```
IF all specs met AND cost < 0.05:
  → CONVERGE. Report final sizing.

IF no improvement for 5 consecutive iterations:
  IF current phase < 3:
    → PHASE UP. Move to next corner set. Reset stall counter.
  ELSE:
    → STOP. Report best achievable.

IF iteration >= budget.max_iterations:
  → STOP. Report best.

OTHERWISE:
  → CONTINUE to next iteration.
```

## Progress Report Format

Print after every iteration:

```
── Iteration 12/50 (Phase 1: TT) ──────────────────────
Parameters: gmid_M1=13.2  L_M1=650n  gmid_M3=8.1  L_M3=500n  Cc=2.8p
Derived:    W_M1=3.1um    W_M3=1.4um  Id_M1=14.2uA

Results vs Spec:
  gain_db:  74.2  (min:70 ✓  target:80 △)
  gbw_hz:   8.1M  (min:5M ✓  target:10M △)
  pm_deg:   62    (min:55 ✓  target:65 △)
  power_w:  178u  (max:200u ✓  target:100u ✗)
  sr_Vus:   6.3   (min:5 ✓)

Cost: 0.31 (feasible ✓)  Best: 0.12 @ iter 9
Strategy: Exploit — reducing gmid_M6 to lower power
───────────────────────────────────────────────────────
```

## Final Report

When optimization completes, report:

```
══ OPTIMIZATION COMPLETE ══════════════════════════════
Status: CONVERGED after 31 iterations (Phase 2)
Total simulation time: 47 min

Best Design (iteration 28):
  gmid_M1=12.8  L_M1=700n  → W_M1=3.5um  Id_M1=15.1uA
  gmid_M3=7.5   L_M3=500n  → W_M3=1.6um  Id_M3=15.1uA
  gmid_M6=9.2   L_M6=350n  → W_M6=5.8um  Id_M6=48uA
  Cc=2.5pF

Corner Results:
         gain_db  gbw_hz  pm_deg  power_w  sr_Vus
  tt_27:  76.3    9.2M    63      185u     7.1    ✓
  ff_27:  70.1    12.8M   56      220u     9.2    ✓ (power marginal)
  ss_27:  81.2    6.1M    68      158u     5.2    ✓

All specs met across Phase 2 corners.
History: process_data/smic13mmrf/opt_history/miller_ota_tb_20260405.json
══════════════════════════════════════════════════════
```

## Resume Support

To resume a previous optimization:

```bash
# List available histories
ls process_data/*/opt_history/*.json

# Claude reads the file, picks up at meta.iteration + 1
# Continues in the current phase with accumulated history
```

## Integration with Other Skills

| Skill | Integration Point |
|-------|-------------------|
| amp-copilot | Provides initial sizing (iteration 0) |
| gm-over-id | Lookup W from gm/Id + L |
| sim-setup | Configure testbench before first iteration |
| sim-run | Execute Spectre each iteration |
| sim-measure | Extract spec values |
| spec-driven-circuit-design | Provides spec template and decomposition |

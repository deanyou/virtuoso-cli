# Bayesian Optimization Skill for Circuit Auto-Tuning

**Date:** 2026-04-05
**Status:** Design approved

## Summary

Closed-loop circuit optimizer where Claude acts as the Bayesian optimization engine.
No external Python/ML dependencies — uses virtuoso-cli skills for simulation and
gm/Id lookup tables for parameter space.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Use cases | Parameter tuning + topology-aware + PVT-robust | Full-stack optimization |
| Engine | Claude as optimizer (no external libs) | Zero deps, works with just virtuoso-cli |
| Objective | Hierarchical: feasibility first, then targets | Matches spec template min/max/target structure |
| Parameter space | gm/Id + L per device | Smooth, low-dimensional, physically meaningful |
| PVT strategy | Progressive: TT → FF/SS → temperature | Fast convergence, practical robustness |

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    OPTIMIZATION LOOP                      │
│                                                          │
│  ┌─────────┐    ┌──────────┐    ┌─────────┐             │
│  │ Suggest  │───>│ Simulate │───>│ Measure │             │
│  │ Next Pt  │    │ (Spectre)│    │ Results │             │
│  └────^─────┘    └──────────┘    └────┬─────┘            │
│       │                               │                  │
│       │         ┌──────────┐          │                  │
│       └─────────│ Update   │<─────────┘                  │
│                 │ History  │                             │
│                 └──────────┘                             │
└──────────────────────────────────────────────────────────┘
```

### Iteration Sequence

1. **SUGGEST** — Claude analyzes history, picks next parameter set
2. **CONVERT** — gm/Id + L -> W via lookup table
3. **SET** — desVar() for each parameter
4. **SIMULATE** — run DC, AC, tran as needed
5. **MEASURE** — extract all spec values
6. **SCORE** — compute feasibility + target cost
7. **RECORD** — append to history JSON
8. **DECIDE** — continue, phase-up, or stop

### Three-Phase Progressive Flow

- **Phase 1** — TT corner only, find feasible region, then optimize targets (~15-25 iterations)
- **Phase 2** — Add FF/SS corners, re-optimize from Phase 1 best (~10-15 iterations)
- **Phase 3** — Add temperature extremes (-40C/125C), final hardening (~5-10 iterations)

## Problem Definition

```json
{
  "optimization": {
    "testbench": {"lib": "LIB", "cell": "CELL_TB", "view": "schematic"},
    "parameters": [
      {"name": "gmid_M1", "type": "gmid", "device": "input_pair", "range": [8, 22], "init": 14},
      {"name": "L_M1", "type": "L", "device": "input_pair", "range": [300e-9, 2e-6], "init": 500e-9},
      {"name": "Cc", "type": "comp", "range": [0.5e-12, 5e-12], "init": 2.2e-12}
    ],
    "specs": {
      "gain_db": {"min": 70, "target": 80, "weight": 1.0},
      "gbw_hz": {"min": 5e6, "target": 10e6, "weight": 1.0},
      "pm_deg": {"min": 55, "target": 65, "weight": 0.8},
      "power_w": {"max": 200e-6, "target": 100e-6, "weight": 0.5}
    },
    "measurements": {
      "gain_db": {"analysis": "ac", "expr": "value(dB20(VF(\"/OUT\")) 1)"},
      "gbw_hz": {"analysis": "ac", "expr": "cross(dB20(VF(\"/OUT\")) 0 1 \"falling\")"}
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

## Scoring Function (Hierarchical)

```
Feasibility:
  violation(spec) = max(0, spec_min - measured) / spec_min   # undershoot
                  + max(0, measured - spec_max) / spec_max   # overshoot
  feasibility_cost = sum(violation(s) for all hard constraints)

  IF feasibility_cost > 0: cost = 1000 + feasibility_cost  (infeasible)
  IF feasibility_cost == 0: cost = target_cost              (feasible)

Target optimization (only when feasible):
  target_cost = sum(w_i * |1 - measured_i/target_i| for all targets)

Corner-aware (Phase 2/3):
  cost = max(cost across all corners in current phase)
```

## Surrogate Strategy (Claude Reasoning)

1. Sort history by cost — identify 3-5 best points
2. Identify which parameters most correlate with improvement
3. **Exploit** (3/4 iterations): perturb best point +/-10-20% in promising dimensions
4. **Explore** (1/4 iterations): sample far from visited regions
5. Respect parameter bounds and gm/Id physical constraints

## Stopping Criteria

- All specs met with margin AND target cost < threshold -> **converge**
- No improvement for 5 consecutive iterations -> **phase up** or **stop**
- Budget exhausted -> **stop with best**

## History File

Persisted at `process_data/<pdk>/opt_history/<cell>_<timestamp>.json`.
Contains: meta, problem definition, best result, full iteration history.
Supports resume across Claude sessions.

## Skill Dependencies

- `gm-over-id` — lookup W from gm/Id + L
- `sim-setup` — configure testbench
- `sim-run` — execute Spectre
- `sim-measure` — extract results
- `skill-exec` — set desVar, read oppoint

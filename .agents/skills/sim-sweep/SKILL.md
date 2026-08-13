---
name: sim-sweep
description: Run parameter sweeps and PVT corner simulations on Virtuoso. Use when sweeping design variables, running corner analysis, or characterizing circuits across operating conditions.
disable-model-invocation: true
allowed-tools: Bash(*/virtuoso *) Read Write
---

# Parameter Sweep & Corner Analysis

## Parameter sweep

Sweep a design variable and collect measurements:

```bash
virtuoso sim sweep \
  --var W --from 1e-6 --to 5e-6 --step 1e-6 \
  --measure 'value(IDC("/NM0/D"))' \
  --measure 'value(getData("/NM0:gm" ?result "dcOpInfo"))' \
  --analysis dc \
  --timeout 600 \
  --format json
```

## PVT corner analysis

Create a `corners.json` file:

```json
{
  "simulator": "spectre",
  "design": {
    "lib": "FT0001A_SH",
    "cell": "gmid",
    "view": "schematic"
  },
  "model_file": "/foundry/smic/.../models/spectre/ms013_io33_v2p6_7p_spe.lib",
  "analysis": {
    "type": "dc",
    "saveOppoint": "t"
  },
  "corners": [
    {"name": "tt",      "section": "tt",  "temp": 27,   "vdd": 1.2},
    {"name": "ss_hot",  "section": "ss",  "temp": 125,  "vdd": 1.08},
    {"name": "ff_cold", "section": "ff",  "temp": -40,  "vdd": 1.32}
  ],
  "measures": [
    {"name": "Id",  "expr": "value(IDC(\"/NM0/D\"))"},
    {"name": "gm",  "expr": "value(getData(\"/NM0:gm\" ?result \"dcOpInfo\"))"}
  ]
}
```

Then run:

```bash
virtuoso sim corner --file corners.json --timeout 600 --format json
```

## Tips

- Set `desVar` values BEFORE running sweep (sweep only varies the specified `--var`)
- Sweep generates all values first, then runs simulations sequentially
- For corner analysis, `vars` in corner entries become `desVar` calls (e.g., `"vdd": 1.2` → `desVar("vdd" 1.2)`)
- Use `--timeout` generously — each sweep/corner point runs a full simulation

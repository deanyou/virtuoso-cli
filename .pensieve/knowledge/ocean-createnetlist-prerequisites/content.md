# Ocean: createNetlist Prerequisites for a Complete Netlist

## Source
Session investigation 2026-04-17: `vcli sim netlist --recreate` for `5T_OTA_D_TO_S_sim`
produced a netlist missing model includes and with unset parameters, causing spectre errors
SFE-23 (undefined model) and SFE-1997 (parameter not assigned).

## Summary
`createNetlist` requires four Ocean session variables to be set before invocation.
Missing any one produces a "successful" netlist that spectre cannot run:
- `simulator('spectre)` — sets the netlist format
- `design("lib" "cell" "view")` — sets the target cell
- `modelFile(...)` — generates the `include` lines in the netlist
- `desVar(name value)` — generates the `parameters` line in the netlist

`vcli sim setup` sets the first two. The user must separately set `modelFile` and `desVar`
before calling `vcli sim netlist`.

## Content

### What Each Variable Controls in the Netlist

| Ocean variable | Netlist output |
|----------------|----------------|
| `simulator('spectre)` | `simulator lang=spectre` header |
| `design(lib cell view)` | Subcircuit + port order |
| `modelFile(list(path section) ...)` | `include "path" section=section` lines |
| `desVar(name value)` | `parameters name=value` line |

### modelFile Syntax

Each entry is a **flat** `list(path section)`, not a nested list:

```skill
; Correct:
modelFile(
  list("/foundry/smic/013mmrf/.../ms013_io33_v2p6_7p_spe.lib" "tt")
  list("/foundry/smic/013mmrf/.../ms013_io33_v2p6_7p_spe.lib" "res_tt")
)

; Wrong (nested):
modelFile(list(list("/path" "tt") list("/path" "res_tt")))

; Wrong (single string):
modelFile("/path/to/models.lib")
```

### desVar Syntax

```skill
desVar("W34" 1.6e-5)
desVar("vdc" 0.6)
desVar("v1" 0.55)
desVar("v2" 0.65)
```

One call per variable. Values reset to empty string `""` after `design()` switch.
Empty string desVar causes SFE-1997 in spectre ("parameter not assigned").

### Recovering Values from a Backup Netlist

If the Ocean session has stale or empty desVar values, read them from a backup netlist:

```bash
# Look for a recent backup (Cadence keeps input.scs.bakN)
ls -la netlist/input.scs.bak*
grep "^parameters" netlist/input.scs.bak2
# → parameters W34=1.6e-05 vdc=0.6 v1=0.55 v2=0.65
```

Also check for model include paths in the backup:
```bash
grep "^include" netlist/input.scs.bak2
# → include "/foundry/.../ms013_io33_v2p6_7p_spe.lib" section=tt
```

### Verification Before Netlisting

```bash
vcli skill exec 'list(simulator() design() modelFile() desVar("W34") desVar("vdc"))'
# Good: (spectre (FT0001A_SH 5T_OTA_D_TO_S_sim schematic) (("/foundry/..." "tt") ...) 1.6e-05 0.6)
# Bad:  (spectre (FT0001A_SH 5T_OTA_D_TO_S_sim schematic) nil "" "")
```

`modelFile()` returning nil or desVar returning `""` means the netlist will be incomplete.

### Full Setup Sequence

```bash
# 1. Switch simulator and design
vcli sim setup --lib FT0001A_SH --cell 5T_OTA_D_TO_S_sim --view schematic

# 2. Set model files
vcli skill exec 'modelFile(list("/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib" "tt") list("/foundry/smic/013mmrf/pdk/20250911/cadence/0.13um_1p3m_8k/models/spectre/ms013_io33_v2p6_7p_spe.lib" "res_tt"))'

# 3. Set design variables
vcli skill exec 'progn(desVar("W34" 1.6e-5) desVar("vdc" 0.6) desVar("v1" 0.55) desVar("v2" 0.65))'

# 4. Netlist
vcli sim netlist --lib FT0001A_SH --cell 5T_OTA_D_TO_S_sim --recreate
```

### Running Spectre Directly (after netlist)

Run from the `schematic/` directory, not `netlist/`:

```bash
cd /path/to/5T_OTA_D_TO_S_sim/spectre/schematic
spectre netlist/input.scs -raw psf +aps ++aps
```

Running from inside `netlist/` causes CMI-2011 (cannot open input file) because the
subcircuit path in the netlist is relative to the schematic directory.

## When to Use
- Before running `vcli sim netlist` on a freshly started Ocean session
- When spectre reports SFE-23 (undefined model) or SFE-1997 (parameter not assigned)
- When the netlist has no `include` or `parameters` lines
- After switching design cells (desVar values reset after `design()`)

## Context Links
- Related: [[ocean-design-cell-switch]] (desVar reset after design switch)
- Related: [[spectre-ade-model-path]] (ADE model path gotchas)
- Related: [[cadence-virtuoso-library-not-registered]] (library must be registered first)

---
name: sim-setup
description: Set up Virtuoso simulation with Ocean SKILL. Use when configuring simulator, design target, model files, or design variables before running simulation.
allowed-tools: Bash(*/virtuoso *)
---

# Simulation Setup

Configure Ocean environment for circuit simulation via virtuoso-cli.

## Steps

### 1. Set simulator and design

```bash
virtuoso sim setup --lib <LIB> --cell <CELL> --view <VIEW> --format json
```

View defaults to `schematic`. Check available views first if setup returns nil:

```bash
virtuoso skill exec 'let((cell) cell=ddGetObj("LIB" "CELL") foreach(mapcar v cell~>views v~>name))' --format json
```

### 2. Set model files (if not already configured in ADE)

```bash
virtuoso skill exec 'modelFile(list("/path/to/models.lib" "tt") list("/path/to/models.lib" "res_tt"))' --format json
```

**Critical**: Do NOT include entries with empty section names `""` for `.lib` files — spectre will fail with SFE-675. Only `.ckt` files can have empty sections.

### 3. Set design variables

```bash
virtuoso skill exec 'desVar("L" 130e-9)' --format json
virtuoso skill exec 'desVar("VGS" 0.6)' --format json
```

Check required variables by looking at spectre.out errors — they list undefined parameters.

### 4. Set results directory

```bash
virtuoso skill exec 'resultsDir("/tmp/my_sim_results")' --format json
```

## Critical workflow

The correct order for a fresh simulation is:

```bash
# 1. Setup simulator and design
virtuoso sim setup --lib LIB --cell CELL

# 2. Set resultsDir BEFORE anything else
virtuoso skill exec 'resultsDir("/tmp/my_sim")' --format json

# 3. Set modelFile with CORRECT absolute paths (verify files exist!)
virtuoso skill exec 'modelFile(list("/path/to/model.lib" "tt") list("/path/to/model.lib" "res_tt"))' --format json

# 4. Set design variables
virtuoso skill exec 'desVar("L" 130e-9)' --format json

# 5. Run
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 120

# 6. Measure
virtuoso sim measure --analysis dcOp --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))'
```

## Known issues

- **`simulator('spectre)` resets modelFile** — always re-set modelFile after `sim setup`
- **Ocean functions don't work inside `let` blocks** — call `simulator()`, `design()`, `analysis()`, `run()` at top level
- **Ocean state persists across CLI calls** — no need to re-setup between runs, but modelFile must be set each session
- **`design()` returns nil**: two causes — (a) the cell may not have the view you specified (check `v~>name`), or (b) the library is not registered (Virtuoso started from wrong directory — restart from the project dir whose `cds.lib` includes the library)
- **`run()` takes <0.3s and no spectre.out**: modelFile not set or paths wrong — spectre silently fails
- **`run()` takes >1s with spectre.out**: real execution happened — check spectre.out for errors
- **Netlisting error OSSHNL-116**: a subcell has no spectre/schematic view (e.g., `notes` cell)
- **Model path errors**: verify paths exist on disk — PDK symlinks can break (use direct path, not through `oa/`)

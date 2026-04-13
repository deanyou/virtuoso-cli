# Maestro Python API

Python wrapper for Cadence Maestro (ADE Assembler) SKILL functions.

**Package:** `virtuoso_bridge.virtuoso.maestro`

```python
from virtuoso_bridge import VirtuosoClient
from virtuoso_bridge.virtuoso.maestro import open_session, close_session, read_config
```

## Two Session Modes

| | Background (`open_session`) | GUI (`deOpenCellView`) |
|---|---|---|
| Lock file | Creates `.cdslck` | Creates `.cdslck` |
| Read config | Yes | Yes |
| Write config | Yes | Yes (needs `maeMakeEditable`) |
| Run simulation | Can start, but `close_session` cancels it | Yes |
| `wait_until_done` | Returns immediately (does not wait) | Blocks until done |
| Close | `close_session` → lock removed | `hiCloseWindow` |

**Use background for read/write config. Use GUI for simulation.**

## Standard Simulation Flow

See **[simulation-flow.md](simulation-flow.md)** for the complete 8-step guide (clean sessions → open GUI → run → read results), common pitfalls, and optimization loop patterns.

## Session Management

`maestro/session.py`

| Python | SKILL | Description |
|--------|-------|-------------|
| `open_session(client, lib, cell) -> str` | `maeOpenSetup` | Background open, returns session string |
| `close_session(client, session)` | `maeCloseSession` | Background close |
| `find_open_session(client) -> str \| None` | `maeGetSessions` + `maeGetSetup` | Find first active session with valid test |

```python
session = open_session(client, "PLAYGROUND_AMP", "TB_AMP_5T_D2S_DC_AC")
# ... do work ...
close_session(client, session)
```

## Read — Three independent functions

`maestro/reader.py`

All return `dict[str, tuple[str, str]]` where key = label, value = `(skill_expr, raw_output)`.

### read_config — test setup

| Key | SKILL |
|-----|-------|
| `maeGetSetup` | `maeGetSetup(?session session)` |
| `maeGetEnabledAnalysis` | `maeGetEnabledAnalysis(test ?session session)` |
| `maeGetAnalysis:<name>` | `maeGetAnalysis(test name ?session session)` — one per enabled analysis |
| `maeGetTestOutputs` | `maeGetTestOutputs(test ?session session)` — returns `(name type signal expression)` |
| `variables` | `maeGetSetup(?session session ?typeName "variables")` |
| `parameters` | `maeGetSetup(?session session ?typeName "parameters")` |
| `corners` | `maeGetSetup(?session session ?typeName "corners")` |

### read_env — system settings

| Key | SKILL |
|-----|-------|
| `maeGetEnvOption` | `maeGetEnvOption(test ?session session)` — model files, view lists, etc. |
| `maeGetSimOption` | `maeGetSimOption(test ?session session)` — reltol, temp, gmin, etc. |
| `maeGetCurrentRunMode` | `maeGetCurrentRunMode(?session session)` |
| `maeGetJobControlMode` | `maeGetJobControlMode(?session session)` |
| `maeGetSimulationMessages` | `maeGetSimulationMessages(?session session)` |

### read_results — simulation results

| Key | SKILL |
|-----|-------|
| `maeGetResultTests` | `maeGetResultTests()` |
| `maeGetOutputValues` | SKILL loop: `maeGetOutputValue` + `maeGetSpecStatus` for each output |
| `maeGetOverallSpecStatus` | `maeGetOverallSpecStatus()` |
| `maeGetOverallYield` | `maeGetOverallYield(history)` |

History name is auto-detected from `asiGetResultsDir`. Returns empty dict if no results.

### export_waveform — download wave data

| Python | SKILL / OCEAN |
|--------|---------------|
| `export_waveform(client, session, expression, local_path, *, analysis="ac", history="")` | `maeOpenResults` → `selectResults` → `ocnPrint` → `maeCloseResults` |

For outputs that return `"wave"` instead of a scalar. Downloads the waveform as a text file (freq/time vs value).

```python
session = open_session(client, "PLAYGROUND_AMP", "TB_AMP_5T_D2S_DC_AC")

# Read config
for key, (expr, raw) in read_config(client, session).items():
    print(f"[{key}] {expr}")
    print(raw)

# Read env
for key, (expr, raw) in read_env(client, session).items():
    print(f"[{key}] {expr}")
    print(raw)

# Read results
for key, (expr, raw) in read_results(client, session).items():
    print(f"[{key}] {expr}")
    print(raw)

# Export waveform
export_waveform(client, session,
    'dB20(mag(VF("/VOUT") / VF("/VSIN")))',
    "output/gain_db.txt", analysis="ac")

export_waveform(client, session,
    'getData("out" ?result "noise")',
    "output/noise.txt", analysis="noise")

close_session(client, session)
```

## Write — Test

`maestro/writer.py`

| Python | SKILL | Description |
|--------|-------|-------------|
| `create_test(client, test, *, lib, cell, view="schematic", simulator="spectre", session="")` | `maeCreateTest` | Create a new test |
| `set_design(client, test, *, lib, cell, view="schematic", session="")` | `maeSetDesign` | Change DUT for existing test |

```python
create_test(client, "TRAN2", lib="myLib", cell="myCell")
set_design(client, "TRAN2", lib="myLib", cell="newCell")
```

## Write — Analysis

| Python | SKILL | Description |
|--------|-------|-------------|
| `set_analysis(client, test, analysis, *, enable=True, options="", session="")` | `maeSetAnalysis` | Enable/disable analysis, set options |

```python
# Enable transient with stop=60n
set_analysis(client, "TRAN2", "tran", options='(("stop" "60n") ("errpreset" "conservative"))')

# Enable AC
set_analysis(client, "TRAN2", "ac", options='(("start" "1") ("stop" "10G") ("dec" "20"))')

# Disable tran
set_analysis(client, "TRAN2", "tran", enable=False)
```

## Write — Outputs & Specs

| Python | SKILL | Description |
|--------|-------|-------------|
| `add_output(client, name, test, *, output_type="", signal_name="", expr="", session="")` | `maeAddOutput` | Add waveform or expression output |
| `set_spec(client, name, test, *, lt="", gt="", session="")` | `maeSetSpec` | Set pass/fail spec |

```python
# Waveform output
add_output(client, "OutPlot", "TRAN2", output_type="net", signal_name="/OUT")

# Expression output
add_output(client, "maxOut", "TRAN2", output_type="point", expr='ymax(VT(\\"/OUT\\"))')

# Spec: maxOut < 400mV
set_spec(client, "maxOut", "TRAN2", lt="400m")

# Spec: BW > 1GHz
set_spec(client, "BW", "AC", gt="1G")
```

## Write — Variables

| Python | SKILL | Description |
|--------|-------|-------------|
| `set_var(client, name, value, *, type_name="", type_value="", session="")` | `maeSetVar` | Set global variable or corner sweep |
| `get_var(client, name, *, session="")` | `maeGetVar` | Get variable value |

```python
set_var(client, "vdd", "1.35")
get_var(client, "vdd")  # => '"1.35"'

# Corner sweep
set_var(client, "vdd", "1.2 1.4", type_name="corner", type_value='("myCorner")')
```

## Write — Parameters (Parametric Sweep)

| Python | SKILL | Description |
|--------|-------|-------------|
| `get_parameter(client, name, *, type_name="", type_value="", session="")` | `maeGetParameter` | Read parameter value |
| `set_parameter(client, name, value, *, type_name="", type_value="", session="")` | `maeSetParameter` | Add/update parameter |

```python
set_parameter(client, "cload", "1p")
set_parameter(client, "cload", "1p 2p", type_name="corner", type_value='("myCorner")')
```

## Write — Environment & Simulator Options

| Python | SKILL | Description |
|--------|-------|-------------|
| `set_env_option(client, test, options, *, session="")` | `maeSetEnvOption` | Set model files, view lists, etc. |
| `set_sim_option(client, test, options, *, session="")` | `maeSetSimOption` | Set reltol, temp, gmin, etc. |

```python
# Change model file section
set_env_option(client, "TRAN2",
    '(("modelFiles" (("/path/model.scs" "ff"))))')

# Change temperature
set_sim_option(client, "TRAN2", '(("temp" "85"))')
```

## Write — Corners

| Python | SKILL | Description |
|--------|-------|-------------|
| `set_corner(client, name, *, disable_tests="", session="")` | `maeSetCorner` | Create/modify corner (empty) |
| `setup_corner(client, name, *, model_file="", model_section="", variables={}, session="")` | `maeSetCorner` + `maeSetVar` + `axl*` | **Recommended.** Create fully configured corner with model file, section, and variables — no XML editing |
| `load_corners(client, filepath, *, sections="corners", operation="overwrite")` | `maeLoadCorners` | Load corners from CSV |

```python
# Create a fully configured corner (recommended)
setup_corner(client, "tt_25",
             model_file="/path/to/mypdk.scs",
             model_section="tt",
             variables={"temperature": "25", "vdd": "1.2"},
             session=session)

# Create empty corner only
set_corner(client, "myCorner", disable_tests='("AC" "TRAN")')

# Load corners from CSV
load_corners(client, "my_corners.csv")
```

## Write — Run Mode & Job Control

| Python | SKILL | Description |
|--------|-------|-------------|
| `set_current_run_mode(client, run_mode, *, session="")` | `maeSetCurrentRunMode` | Switch run mode |
| `set_job_control_mode(client, mode, *, session="")` | `maeSetJobControlMode` | Set Local/LSF/etc. |
| `set_job_policy(client, policy, *, test_name="", job_type="", session="")` | `maeSetJobPolicy` | Set job policy |

```python
set_current_run_mode(client, "Single Run, Sweeps and Corners")
set_job_control_mode(client, "Local")
```

## Write — Simulation

| Python | SKILL | Description |
|--------|-------|-------------|
| `run_simulation(client, *, session="", callback="")` | `maeRunSimulation` | Run (async), returns history name |
| `wait_until_done(client, timeout=600, _marker="")` | SSH poll | Wait for marker file (used by `run_and_wait`) |
| `run_and_wait(client, *, session="", timeout=600)` | `maeRunSimulation(?callback ...)` + SSH poll | **Recommended.** Run + wait without blocking SKILL channel |

```python
# Recommended: run_and_wait (no race condition, SKILL stays free)
history, status = run_and_wait(client, session=session, timeout=600)

# Or manual two-step (if you need custom callback):
# history = run_simulation(client, session=session)
# ... SKILL channel is free, do other work ...
```

## Write — Export

| Python | SKILL | Description |
|--------|-------|-------------|
| `create_netlist_for_corner(client, test, corner, output_dir)` | `maeCreateNetlistForCorner` | Export netlist for one corner |
| `export_output_view(client, filepath, *, view="Detail")` | `maeExportOutputView` | Export results to CSV |
| `write_script(client, filepath)` | `maeWriteScript` | Export setup as SKILL script |

```python
create_netlist_for_corner(client, "TRAN2", "myCorner_2", "./myNetlistDir")
export_output_view(client, "./results.csv")
write_script(client, "mySetupScript.il")
```

## Write — Migration

| Python | SKILL | Description |
|--------|-------|-------------|
| `migrate_adel_to_maestro(client, lib, cell, state)` | `maeMigrateADELStateToMaestro` | ADE L → Maestro |
| `migrate_adexl_to_maestro(client, lib, cell, view="adexl", *, maestro_view="maestro")` | `maeMigrateADEXLToMaestro` | ADE XL → Maestro |

```python
migrate_adel_to_maestro(client, "myLib", "myCell", "spectre_state1")
migrate_adexl_to_maestro(client, "myLib", "myCell")
```

## Write — Save

| Python | SKILL | Description |
|--------|-------|-------------|
| `save_setup(client, lib, cell, *, session="")` | `maeSaveSetup` | Save maestro to disk |

```python
save_setup(client, "myLib", "myCell", session=session)
```

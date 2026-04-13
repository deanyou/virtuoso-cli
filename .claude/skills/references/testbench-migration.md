# Copy a Testbench to Another Library

Workflow for replicating a testbench + Maestro setup in a new library.

## Step 1: Copy the DUT cell

```python
# dbCopyCellView needs a db object as first arg, not a string
client.execute_skill(
    'dbCopyCellView(dbOpenCellViewByType("SRC_LIB" "MY_DUT" "schematic") '
    '"DST_LIB" "MY_DUT" "schematic")')
client.execute_skill(
    'dbCopyCellView(dbOpenCellViewByType("SRC_LIB" "MY_DUT" "symbol") '
    '"DST_LIB" "MY_DUT" "symbol")')
```

## Step 2: Create the testbench schematic

Use `schematic.edit()` + `inst()` + `label_term()`. Then set source parameters
per-instance via CDF — do NOT use `schHiReplace` with `cellName` matching, it
hits ALL instances of that cell type.

```python
# CORRECT: set param on a specific instance by name
client.execute_skill(
    'let((cv inst cdf p) '
    'cv=dbOpenCellViewByType("LIB" "CELL" "schematic" "schematic" "a") '
    'inst=car(setof(x cv~>instances x~>name=="V8")) '
    'cdf=cdfGetInstCDF(inst) p=cdfFindParamByName(cdf "vdc") '
    'p~>value="Vcm")')

# WRONG: schHiReplace with cellName match — sets ALL vsin instances to same value
# schHiReplace(?replaceAll t ?propName "cellName" ?condOp "==" ?propValue "vsin" ...)
```

**Important:** Always use `dbOpenCellViewByType(lib cell "schematic" "schematic" "a")`
instead of `geGetEditCellView()`. The latter depends on the current GUI window focus
and fails when Maestro or other non-schematic windows are active.

## Step 3: CDF parameter name gotchas

CDF param names differ from Spectre netlist param names:

| analogLib cell | CDF param | Spectre netlist param |
|----------------|-----------|----------------------|
| vsin | `vdc` | `sinedc` (auto-mapped by netlister) |
| vsin | `va` | `ampl` |
| vsin | `sinephase` | `sinephase` |
| vsin | `freq` | `freq` |
| vdc | `vdc` | `dc` |
| idc | `idc` | `dc` |
| cap | `c` | `c` |

Always check actual CDF param names with `cdfGetInstCDF(inst)~>parameters` before setting.

## Step 4: Create Maestro

Use `.il` files for `maeSetAnalysis` (backtick syntax required for options lists).
Python `execute_skill()` can't handle backtick + nested quotes reliably.

```python
# Create session + test
session = open_session(client, LIB, CELL)  # creates empty maestro
client.execute_skill(f'maeCreateTest("TEST_NAME" ?session "{session}" '
                     f'?lib "{LIB}" ?cell "{CELL}" ?view "schematic")')

# Set variables via Python
maeSetVar("Fs", "1G", ?session session)

# Set analysis via .il file (backtick syntax)
client.load_il("setup_tran.il")  # contains maeSetAnalysis(test "tran" ?enable t ?options `(...))

# Set outputs via Python (each needs a unique name, NOT nil)
maeAddOutput("VOUTP", test, ?outputType "net" ?signalName "/VOUTP")
```

**Analysis options: use bare variable names, not `VAR("...")`.**
`VAR()` is a Maestro runtime function that does NOT translate to Spectre netlist.
The netlister expects bare variable names (e.g. `t_end`, not `VAR("t_end")`).

## Step 5: Run simulation

`maeRunSimulation` requires a GUI Maestro window — open with `deOpenCellView` first.
`maeWaitUntilDone` is unreliable — poll spectre.out via SSH instead.

```python
# Open Maestro in GUI (required for maeRunSimulation)
client.execute_skill('deOpenCellView("LIB" "CELL" "maestro" "maestro" nil "a")')

# Run
client.execute_skill(f'maeRunSimulation(?session "{session}")')

# Poll completion via SSH (reliable)
import time
for i in range(120):
    r = runner.run_command(f'grep "completes" {sim_dir}/spectre.out 2>/dev/null || echo running')
    if "completes" in r.stdout:
        break
    time.sleep(5)
```

## Key pitfalls

| Pitfall | Symptom | Fix |
|---------|---------|-----|
| `maeAddOutput` with nil name | `argument #1 should be a string` | Every output needs a string name |
| `VAR("x")` in tran options | `Function 'VAR' is not defined` in Spectre | Use bare variable name `x` |
| `maeRunSimulation` returns nil | No GUI Maestro window open | Open with `deOpenCellView(... "maestro" ...)` first |
| Delete cell with open Maestro | Crash / dialog storm | Clear schematic instances instead of deleting cell |
| `geGetEditCellView` wrong window | `cdfGetInstCDF(nil)` errors | Use `dbOpenCellViewByType` with explicit lib/cell |
| `maeOpenSetup` empty test list | `maeGetSetup` returns nil | Call `maeCreateTest` after `maeOpenSetup` |
| `maeWaitUntilDone` returns nil | Background session, or sim already done | Poll spectre.out via SSH instead |
| `schHiReplace` by cellName | Sets ALL instances of that cell type | Set per-instance via `cdfGetInstCDF` |
| CDF `va` vs netlist `ampl` | Wrong param name, value not set | Check CDF names with `cdfGetInstCDF(inst)~>parameters` |

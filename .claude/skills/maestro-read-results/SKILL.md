---
name: maestro-read-results
description: |
  Read Maestro/ADE simulation output values directly from PSF binary files — no Virtuoso GUI
  or bridge required. Parses maestro.sdb + active.state XML to extract output expressions,
  resolves the PSF directory from history.sdb, evaluates Ocean-style expressions (getData,
  dB20, phaseDeg, bandwidth, ymax, VF, VT…), and returns structured JSON results.

  Use this skill whenever:
  - The user asks "what is the gain / phase margin / bandwidth from the simulation?"
  - `maeGetOutputValue` returned nil (requires GUI results loaded in memory)
  - The user wants to check simulation results offline or from a script
  - The user wants to read PSF files / evaluate output expressions from a Maestro session
  - Any request involving Maestro output expressions, PSF files, or ADE results without GUI
allowed-tools:
  - Bash(python3 *)
  - Bash(find *)
  - Bash(grep *)
  - Read
argument-hint: "[maestro_dir] [--run <run_name>] [--test <test_name>] [--list]"
---

## Arguments routing

ARGUMENTS = everything the user typed after `/maestro-read-results`.

| Pattern | Action |
|---------|--------|
| `<path> [--list]` | Run script with provided path and flags directly |
| `<path>` only | Run script, read all outputs for latest run |
| Empty / vague | Search for `maestro.sdb` files, let user pick |

```bash
# If ARGUMENTS contains a path, use it directly:
python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py $ARGUMENTS

# If no path given, discover candidates first:
find ~/projects -name "maestro.sdb" -size +0c -maxdepth 10 2>/dev/null | head -20
```

## Overview

`maestro.sdb` and `active.state` are plain XML files storing all simulation configuration,
output expressions, corner definitions, and history paths. The PSF binary results are
readable via the `psf` tool (`/opt/cadence/IC231/bin/psf`). This skill combines both to
evaluate output expressions without any GUI.

## Quick Start

```bash
# List available runs in a Maestro session
python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py /path/to/maestro --list

# Read all output values for the latest run
python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py /path/to/maestro

# Specific run or test
python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py /path/to/maestro --run ExplorerRun.0
python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py /path/to/maestro --test myTestName
```

## Finding the Maestro Directory

The Maestro directory contains `maestro.sdb`. Common locations:

```bash
# From vcli maestro list-sessions output (psf_base field)
find ~/projects -name "maestro.sdb" -maxdepth 8 2>/dev/null | head -10

# Typical layout:
# <lib_dir>/<cell>/<view>/maestro/  (ADE Explorer)
# <sim_dir>/maestro/                 (Assembler)
```

The `maestro_dir` to pass is the directory containing `maestro.sdb`.

## Output Format

```json
{
  "test": "FT0001A_SH_5T_OTA_D_TO_S_sim_1",
  "run": "ExplorerRun.0",
  "timestamp": "Apr 21 22:53:48 2026",
  "psf_dir": "/path/to/psf",
  "outputs": [
    {
      "name": "gain_dc",
      "expression": "getData(\"net1\" ?result \"ac\")[0]",
      "eval_type": "point",
      "value": 0.3947,
      "dB": -8.075,
      "phase_deg": 179.88
    }
  ]
}
```

## Supported Ocean Functions

| Function | Description |
|----------|-------------|
| `getData(net ?result type)` | Load waveform/scalar from PSF (`"dc"`, `"ac"`, `"tran"`) |
| `VF(net)` / `VT(net)` | Shorthand for AC / transient getData |
| `dB20(wave)` / `db(wave)` | Magnitude in dB |
| `mag(wave)` | Absolute magnitude |
| `phase(wave)` / `phaseDeg(wave)` | Phase in radians / degrees |
| `phaseDegUnwrapped(wave)` | Unwrapped phase in degrees |
| `ymax(wave)` / `ymin(wave)` | Maximum / minimum y value |
| `bandwidth(wave, db_drop, dir)` | Frequency at db_drop from peak |
| `slewRate(wave, ...)` | Approximate max dV/dt |
| Arithmetic | `+` `-` `*` `/` between waveforms and scalars |

## Troubleshooting

**`psf` tool not found**: Check for alternate Cadence installs:
```bash
find /opt/cadence -name "psf" -type f 2>/dev/null
```
Then pass it via env: `PSF_TOOL=/path/to/psf python3 ${CLAUDE_SKILL_DIR}/scripts/read_results.py ...`

**`outputs: []`**: The `active.state` XML may use a different `sevOutputStruct` layout.
Run with `--list` first to confirm the run directory is found, then inspect:
```bash
grep -o 'sevOutputStruct[^<]*' /path/to/maestro/active.state | head -5
```

**Expression eval error**: Print the raw expression from the JSON output and test manually:
```bash
python3 -c "
import sys; sys.path.insert(0, '${CLAUDE_SKILL_DIR}/scripts')
import read_results as r
# ... load psf data, call r.eval_expression(expr, env)
"
```

**Wrong PSF directory**: History uses `$AXL_HISTORY_NAME` variable. Check:
```bash
grep -o 'AXL_HISTORY_NAME[^"]*' /path/to/maestro/history.sdb | head -3
```

---
name: sim-plot
description: |
  Visualize Virtuoso simulation results as matplotlib charts. Use after:
  (1) sim sweep — line plot of measurements vs swept variable,
  (2) sim corner — grouped bar chart across PVT corners,
  (3) sim measure — horizontal bar of scalar measurements,
  (4) AC/Bode plot from PSF getData results (magnitude + phase),
  (5) process_data lookup tables — gm/Id curves for all L values.
  Auto-detects chart type from JSON structure. Saves PNG via plot_sim.py.
author: Claude Code
version: 1.0.0
date: 2026-04-06
---

# sim-plot: Matplotlib Visualization

Pipe `--format json` output into `plot_sim.py` to get charts. The script
lives at `.claude/skills/sim-plot/scripts/plot_sim.py`.

## Usage by Chart Type

### 1. Parameter Sweep → Line Plot

```bash
virtuoso sim sweep \
  --var W34 --from 2e-6 --to 24e-6 --step 4e-6 \
  --analysis dc \
  --expr 'openResults("/tmp/opt/psf") selectResult('"'"'acSweep) dB20(value(VF("net1") 1))' \
  --format json | \
  python3 .claude/skills/sim-plot/scripts/plot_sim.py \
    --output plots/w34_sweep.png \
    --title "5T OTA: Gain vs W34"
```

Or save JSON first then plot:
```bash
virtuoso sim sweep ... --format json > /tmp/sweep.json
python3 .claude/skills/sim-plot/scripts/plot_sim.py \
  --input /tmp/sweep.json --output plots/sweep.png
```

**Expected JSON structure**:
```json
{
  "variable": "W34",
  "headers": ["W34", "gain_dB", "gbw_hz"],
  "data": [
    {"W34": "2e-06", "gain_dB": "9.9", "gbw_hz": "9e5"},
    {"W34": "8e-06", "gain_dB": "42.3", "gbw_hz": "1.14e7"}
  ]
}
```

---

### 2. Corner Analysis → Grouped Bar Chart

```bash
virtuoso sim corner --file corners.json --format json | \
  python3 .claude/skills/sim-plot/scripts/plot_sim.py \
    --output plots/corner.png --title "PVT Corner Results"
```

**Expected JSON structure**:
```json
{
  "corners": 3,
  "headers": ["corner", "temp", "gain_dB", "gbw_hz"],
  "data": [
    {"corner": "tt", "temp": "27", "gain_dB": "43.2", "gbw_hz": "1.15e7"},
    {"corner": "ff", "temp": "27", "gain_dB": "40.1", "gbw_hz": "1.4e7"},
    {"corner": "ss", "temp": "27", "gain_dB": "45.8", "gbw_hz": "9.2e6"}
  ]
}
```

---

### 3. Scalar Measurements → Horizontal Bar Chart

```bash
virtuoso sim measure --analysis dcOp \
  --expr 'getData("I0.NM0:gm" ?result "dcOpInfo")' \
  --expr 'getData("I0.NM0:gds" ?result "dcOpInfo")' \
  --format json | \
  python3 .claude/skills/sim-plot/scripts/plot_sim.py \
    --output plots/oppoint.png
```

---

### 4. AC Bode Plot

The AC PSF data must be converted to the Bode JSON format first. Use Ocean
`getData` to extract frequency, magnitude, and phase:

```bash
# Step 1: Get frequency and output data from PSF
virtuoso skill exec '
openResults("/tmp/opt_5t_ota/psf")
selectResult('"'"'acSweep)
RB__vout = VF("net1")
RB__freq = frequency(RB__vout)
RB__mag = dB20(RB__vout)
RB__ph = phase(RB__vout)
list(
  sprintf(nil "%s" RB__freq)
  sprintf(nil "%s" RB__mag)
  sprintf(nil "%s" RB__ph)
)
' --format json > /tmp/ac_raw.json

# Step 2: Claude converts to Bode JSON format and plots
# (Claude writes a small Python conversion + call)
```

**Bode JSON format accepted by plot_sim.py**:
```json
{
  "freq": [1, 10, 100, 1000, 10000, 100000, 1000000, 10000000],
  "mag_db": [43.2, 43.2, 43.1, 42.8, 40.0, 30.0, 20.0, 0.1],
  "phase_deg": [-1, -5, -10, -30, -60, -120, -160, -178]
}
```

The plot shows:
- Top: Magnitude vs frequency (dB), marks GBW (0 dB crossing)
- Bottom: Phase vs frequency (°), marks phase margin at GBW

---

### 5. gm/Id Lookup Table → Transistor Curves

```bash
python3 .claude/skills/sim-plot/scripts/plot_sim.py \
  --input process_data/smic13mmrf/nmos_lookup.json \
  --output plots/nmos_gmid.png \
  --title "NMOS gm/Id Lookup (SMIC 0.13µm)"
```

Produces 4 subplots:
- Gain (dB) vs gm/Id for each L
- fT (GHz) vs gm/Id
- Id (µA/µm) vs gm/Id (linear)
- Id (µA/µm) vs gm/Id (log)

---

## Common Workflows

### After a W-sweep optimization run
```bash
# Already have the data from this session's 5T OTA sweep:
echo '{
  "status": "success",
  "variable": "W34",
  "headers": ["W34", "gain_dB", "gbw_hz", "gm_uS", "gmId"],
  "data": [
    {"W34": "2e-06", "gain_dB": "9.9",  "gbw_hz": "9e5",   "gm_uS": "174", "gmId": "7.0"},
    {"W34": "4e-06", "gain_dB": "36.0", "gbw_hz": "1.08e7","gm_uS": "366", "gmId": "11.8"},
    {"W34": "8e-06", "gain_dB": "42.3", "gbw_hz": "1.14e7","gm_uS": "371", "gmId": "11.9"},
    {"W34": "16e-06","gain_dB": "43.2", "gbw_hz": "1.15e7","gm_uS": "372", "gmId": "11.9"},
    {"W34": "24e-06","gain_dB": "43.3", "gbw_hz": "1.14e7","gm_uS": "372", "gmId": "11.9"}
  ]
}' | python3 .claude/skills/sim-plot/scripts/plot_sim.py \
  --output plots/5t_ota_w34.png --title "5T OTA: Gain & GBW vs W34 (PMOS load)"
```

### After process char
```bash
python3 .claude/skills/sim-plot/scripts/plot_sim.py \
  --input process_data/smic13mmrf/nmos_lookup.json \
  --output plots/nmos_char.png
```

---

## Script Location

```
.claude/skills/sim-plot/scripts/plot_sim.py
```

**Requirements**: Python 3.8+, matplotlib, numpy  
**Check**: `python3 -c "import matplotlib, numpy; print('OK')"`

**Install if missing**:
```bash
pip install matplotlib numpy
```

## Output

- Saves PNG to `--output` path (default: `sim_plot.png` in current dir)
- Prints: `Chart saved: /path/to/output.png`
- DPI: 150 by default (`--dpi 300` for publication quality)

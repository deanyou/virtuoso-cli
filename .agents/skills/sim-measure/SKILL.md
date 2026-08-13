---
name: sim-measure
description: Extract waveform measurements from Virtuoso simulation results. Use when measuring voltage, current, gm, gm/Id, bandwidth, settling time, or any simulation metric.
allowed-tools: Bash(*/virtuoso *)
---

# Measure Simulation Results

Extract metrics from completed simulations via `virtuoso sim measure`.

## Usage

```bash
virtuoso sim measure --analysis <TYPE> --expr '<EXPRESSION>' [--expr '<EXPR2>'] --format json
```

## DC operating point measurements

```bash
# Voltage at a node
virtuoso sim measure --analysis dcOp --expr 'value(VDC("/net1"))' --format json

# Drain current
virtuoso sim measure --analysis dcOp --expr 'value(IDC("/NM0/D"))' --format json

# Operating point parameter (gm, gds, vth, etc.)
virtuoso sim measure --analysis dcOp --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))' --format json
virtuoso sim measure --analysis dcOp --expr 'value(getData("/NM0:ids" ?result "dcOpInfo"))' --format json
virtuoso sim measure --analysis dcOp --expr 'value(getData("/NM0:vth" ?result "dcOpInfo"))' --format json

# Multiple measurements at once
virtuoso sim measure --analysis dcOp \
  --expr 'value(VDC("/net1"))' \
  --expr 'value(IDC("/NM0/D"))' \
  --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))' \
  --format json
```

## Transient measurements

```bash
# Peak voltage
virtuoso sim measure --analysis tran --expr 'ymax(VT("/OUT"))' --format json

# Minimum voltage
virtuoso sim measure --analysis tran --expr 'ymin(VT("/OUT"))' --format json

# Crossing time (settling)
virtuoso sim measure --analysis tran --expr 'cross(VT("/OUT") 0.6 1 "rising")' --format json

# Value at specific time
virtuoso sim measure --analysis tran --expr 'value(VT("/OUT") 10e-6)' --format json
```

## AC measurements

```bash
# Gain at DC
virtuoso sim measure --analysis ac --expr 'value(dB20(VF("/OUT")) 1)' --format json

# Unity-gain bandwidth
virtuoso sim measure --analysis ac --expr 'cross(dB20(VF("/OUT")) 0 1 "falling")' --format json

# Phase margin
virtuoso sim measure --analysis ac --expr 'value(phase(VF("/OUT")) cross(dB20(VF("/OUT")) 0 1 "falling"))' --format json
```

## Available signal accessors

| Function | Domain | Description |
|----------|--------|-------------|
| `VDC("/node")` | dc | DC voltage |
| `IDC("/inst/term")` | dc | DC current |
| `VT("/node")` | tran | Transient voltage |
| `IT("/inst/term")` | tran | Transient current |
| `VF("/node")` | ac | AC voltage (complex) |
| `IF("/inst/term")` | ac | AC current (complex) |
| `getData("/inst:param" ?result "dcOpInfo")` | dcOp | Operating point param |

## Waveform math functions

| Function | Description |
|----------|-------------|
| `ymax(wave)` | Maximum value |
| `ymin(wave)` | Minimum value |
| `value(wave time)` | Value at time/freq |
| `cross(wave val n dir)` | Time when wave crosses val |
| `dB20(wave)` | 20*log10(magnitude) |
| `phase(wave)` | Phase in degrees |
| `bandwidth(wave level)` | -3dB bandwidth |
| `integ(wave)` | Integral over sweep range |
| `rms(wave)` | RMS value |
| `average(wave)` | Average value |
| `delay(wave1 val1 edge1 wave2 val2 edge2 n)` | Propagation delay |
| `slewRate(wave pct1 pct2 dir)` | Slew rate |
| `settlingTime(wave final tol)` | Settling time |

## Noise measurements

```bash
# Run noise analysis first
virtuoso sim run --analysis noise --start 10 --stop 100e3 --dec 20 --timeout 120

# Spot noise at 1kHz
virtuoso sim measure --analysis noise \
  --expr 'value(getData("/OUT" ?result "noise") 1e3)'

# Integrated noise (10Hz-100kHz)
virtuoso sim measure --analysis noise \
  --expr 'integ(getData("/OUT" ?result "noise") 10 100e3)'
```

## Common amplifier measurements (complete recipe)

```bash
# === DC operating point ===
virtuoso sim run --analysis dc --param saveOppoint=t
virtuoso sim measure --analysis dcOp \
  --expr 'value(VDC("/OUT"))' \
  --expr 'value(IDC("/M1/D"))'

# === AC: gain, GBW, phase margin ===
virtuoso sim run --analysis ac --start 1 --stop 10e9 --dec 20
DC_GAIN=$(virtuoso sim measure --analysis ac --expr 'dB20(value(VF("/OUT") 1))' --format json | jq -r '.measures[0].value')
UGB=$(virtuoso sim measure --analysis ac --expr 'cross(dB20(VF("/OUT")) 0 1 "falling")' --format json | jq -r '.measures[0].value')
PM=$(virtuoso sim measure --analysis ac --expr 'value(phase(VF("/OUT")) cross(dB20(VF("/OUT")) 0 1 "falling")) + 180' --format json | jq -r '.measures[0].value')
echo "Gain=${DC_GAIN}dB  GBW=${UGB}Hz  PM=${PM}°"

# === Transient: slew rate, settling ===
virtuoso sim run --analysis tran --stop 20u
virtuoso sim measure --analysis tran \
  --expr 'slewRate(VT("/OUT") 10 90 "rising")' \
  --expr 'slewRate(VT("/OUT") 90 10 "falling")'
```

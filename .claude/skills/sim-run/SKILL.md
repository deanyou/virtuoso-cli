---
name: sim-run
description: Run circuit simulation (DC, tran, AC) on Virtuoso. Use when executing Spectre simulation, running analysis, or checking simulation results.
argument-hint: '[analysis, e.g. "tran 10us" or "ac 1Hz-1GHz" or "dc"]'
allowed-tools: Bash(virtuoso *)
---

# Run Simulation

Execute Spectre simulation via `virtuoso sim run`.

## Prerequisites

Simulation must be set up first (see `/sim-setup`):
- `simulator('spectre)` configured
- `design(lib cell view)` set
- `modelFile(...)` configured
- `desVar(...)` set for any parameterized variables
- `resultsDir(...)` set

## Run analysis

```bash
# DC operating point
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 120 --format json

# Transient
virtuoso sim run --analysis tran --stop 10u --timeout 300 --format json

# AC
virtuoso sim run --analysis ac --start 1 --stop 1e9 --dec 10 --timeout 300 --format json
```

## Important: resultsDir — DO NOT CHANGE

⚠️ **The ADE session binds `run()` to the resultsDir established when the GUI session
was first created. Changing it to any other path silently breaks `run()` (returns nil).**

```bash
# ✅ Find the canonical path from the runSimulation script
cat <netlist_dir>/runSimulation | grep "\-raw"
# → -raw ../../../../tmp/opt_5t_ota/psf  →  canonical = /tmp/opt_5t_ota

# ✅ If resultsDir is nil, restore to canonical path
virtuoso skill exec 'resultsDir("/tmp/opt_5t_ota")'

# ❌ Never do this before a run — it will break run()
virtuoso skill exec 'resultsDir("/tmp/some_new_path")'
```

`sim run` now returns a clear error if `resultsDir` is nil instead of auto-setting it
to a temp path (which would break the ADE session binding).

Also: **do not call `sim setup` again once a working ADE session exists** — it resets
the Ocean session state and can cause `run()` to return nil.

## Verify success

After `run()`, check spectre.out for errors:

```bash
# Check the log
virtuoso skill exec 'resultsDir()' --format json
# Then read the spectre.out file at <resultsDir>/psf/spectre.out

# A successful run has PSF data files (dcOp.dc, tran.tran, etc.)
# A failed run only has artistLogFile, simRunData, variables_file
```

## Key indicators

| run() output | Meaning |
|-------------|---------|
| Returns resultsDir path AND PSF data files exist | Simulation completed ✓ |
| Returns resultsDir path BUT only artistLogFile/simRunData/spectre.out | **Spectre failed** — run() does NOT check spectre's exit code |
| Returns nil, takes <0.01s | Analysis not configured or session lost |
| Returns nil, takes >0.1s | Netlisting ran but spectre may have failed |

⚠️ **`run()` returning a non-nil path is not sufficient proof of success.** Spectre can
terminate with fatal errors (e.g., SFE-868) and `run()` still returns the resultsDir.
Always verify PSF data files exist:
```bash
tail -2 <resultsDir>/psf/spectre.out
# ✓ "spectre completes with 0 errors"
# ✗ "spectre terminated prematurely due to fatal error"
```

## Common errors in spectre.out

| Error | Fix |
|-------|-----|
| SFE-868: ADE-generated path `.../oa/lib/../models/...` | Patch input.scs: replace with direct absolute model path + `section=tt` — see `/spectre-netlist-gotchas` §8 |
| SFE-868: Cannot open input file (other) | Model path wrong — verify file exists |
| SFE-675: no valid section name | Empty section `""` in modelFile for .lib — remove it |
| SFE-1997: parameter not assigned | Set `desVar()` for the missing parameter |
| OSSHNL-116: Cannot descend into views | Subcell missing spectre view — remove instance or add view |

## Spectre mode selection (measured)

> **Source**: Adapted from
> [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite),
> commit `2110dde` (2026-05-31), MIT-licensed. Measured on an 11-bit
> sub-radix-2 SAR ADC tran (N=128 coherent FFT, `ax` baseline ≈ 220 s).

Precision ordering — the trade-off between solver strictness and ENOB:

| arg | preset | speed | ENOB Δ vs `aps` | use for |
|---|---|---|---|---|
| `"spectre"` | (none) | slowest | reference | least license demand, basic direct |
| `"aps"` | `+preset=aps` | 1.0× (gold) | 0.000 | sign-off accuracy reference |
| `"cx"` | `+preset=cx` | 1.2× | −0.03 | sign-off for designs with mixed-signal stiff loops (cmp metastability) |
| `"ax"` | `+preset=ax` | **2.0×** | **−0.03** (within noise) | **default for daily work** |
| `"mx"` | `+preset=mx` | 3.8× | −0.29 | design exploration, corner sweeps where 0.3 ENOB is acceptable |
| `"lx"` | `+preset=lx` | 5.9× | −2.8 (**unusable for SAR**) | small-signal AC / linear DC sweeps; not for circuits with cmp/regen |
| `"vx"` | `+preset=vx` | 8.8× | −8.5 (**totally fails**) | verification-style connectivity / DC convergence only — never for transient signal fidelity |

```python
spectre_mode_args("ax")     # default for daily transient work
spectre_mode_args("aps")    # reference / sign-off
spectre_mode_args("mx")     # fast iteration if ENOB ≤ 0.3 loss is OK
```

**Critical**: SAR / latched-comparator circuits and any topology with
metastable regeneration depend on tight `reltol` (1e-4 or better) to
resolve LSB-scale differential inputs. `lx` relaxes `reltol` to ~1e-3
and drops ENOB by ~3 bits on such circuits; `vx` disables LTE bounding
entirely and produces garbage. Reserve those two for non-signal-fidelity
work (DC, connectivity, link-test).

If a Maestro config you inherit specifies `+preset=lx` or `+preset=vx`
for a transient performance sim, that's almost always a bug.

## When (and when not) to replace cells with Verilog-A for speedup

> **Source**: Same commit `2110dde` (2026-05-31), MIT-licensed.

Verilog-A behavioral replacement of cells is a tempting acceleration
lever, but the speedup is **non-monotonic in cell size** — replacing big
cells helps, replacing small cells **hurts**. Measured on an 11-bit SAR
ADC tran (`ax` mode, N=64, baseline 132 s):

| Cell replaced | Transistor count | Wall-time change | Result |
|---|---|---|---|
| Output capture DFFs (1-pin behavior, 12 instances × 1 D-FF each) | 12 × ~10 MOS | 0% (neutral) | ✓ Easy, no gain — skip unless cleaning the netlist |
| Per-bit SAR latch with feedback (12 × ~12 MOS + 4 std cells) | ~200 MOS total | **−13% (slower)** | ✗ `transition()` event-queue overhead × 11 concurrent instances exceeds the BSIM equation savings |
| StrongARM comparator (47 MOS) | 47 MOS, 1 instance | **+9-17%** | ✓ Big cell, single instance — clear win |

**Rule of thumb**: VA replacement helps when the cell is **large**
(≥ 40 MOS) and instantiated **once or twice**. It hurts when the cell is
**small** (< 20 MOS) and **many instances** share the same input event
source — each `@(cross())` adds to the spectre event queue; with N
concurrent instances watching the same node, queue overhead grows ~N×
while the BSIM savings stay linear in N.

**The actually-effective SAR speed levers** (measured, not from VA):

| Lever | Mechanism | Typical speedup | ENOB cost |
|---|---|---|---|
| Cut FFT N (e.g., 128 → 64) | Tran stop time scales linearly | ~40% | 0 (within meas noise) |
| `strobeoutput=strobeonly` + lean save | Cuts download + parse overhead; file size 1000× smaller | ~5-10% wall, 1500× disk | 0 |
| Replace 1-2 big cells (cmp / opamp) with VA | Skip BSIM equations for ~50+ MOS | ~10-20% | depends on VA fidelity |
| Drop LPE std-cell models for schematic-spi | Remove per-cell wire parasitics | ~20% | minor timing shift |
| Increase `maxstep` | Fewer solver iterations | ~20% per 2× | depends on circuit, risky for cmp metastability |
| Spectre mode `ax → mx` | Looser solver tolerance | ~50% | −0.3 ENOB on SAR |

The first four stack without ENOB cost. The last two trade accuracy for
speed.

## Output size control: save list, strobing, format

> **Source**: Same commit `2110dde` (2026-05-31), MIT-licensed.

By default the `.scs` netlist's `tran tran ...` directive saves at every
solver timestep for every signal — a clocked SAR-style transient at
`maxstep=5p` over hundreds of `ns` produces 100+ MB of PSF ASCII per
signal group. Three knobs:

### 1. `saveOptions options save=<mode>` + explicit `save` list

```scs
save CLKS RSTP I_SAR.VTOPP DOUT\<11\> ... DOUT\<0\>
saveOptions options save=selected
```

- `save=allpub` — every public node + every terminal current (huge default).
- `save=selected` — **only** the nodes/terminals in the explicit `save` line.
- `save=lvlpub` — pub down to a given hierarchy level.

For production runs of large mixed-signal designs, **always use
`save=selected`** with a curated 10-20 signal list. `save=allpub` is the
most common cause of runaway PSF size on lab-cluster sims.

### 2. `strobeoutput=<mode>` (gotcha: "all" is bigger, not smaller)

The `tran tran ...` directive accepts `strobeperiod` and `strobeoutput`:

```scs
tran tran stop=t_end maxstep=5p \
    strobeperiod=1/Fs strobeoutput=strobeonly ...
```

| Mode | What gets saved | Use for |
|---|---|---|
| `strobeoutput=all` | **Every solver timestep PLUS strobed samples** (biggest file) | Debugging — need waveform shape between samples |
| `strobeoutput=strobeonly` | **Only** strobed samples (1 sample per `strobeperiod`) | ENOB / SNDR / corner sweeps where you only need per-cycle values |

The name "all" misleads — it means "both continuous and strobed views,"
not "all signals." Switching to `strobeonly` typically cuts file size
500×-1500× on N=64..256 sims. **For ENOB-only runs of a clocked ADC**,
`strobeonly` is the right default.

### 3. `output_format` — PSF ASCII vs binary

`virtuoso sim run` uses `output_format="psfascii"` by default (the
in-tree PSF ASCII parser handles it). **`output_format="psfbin"` is NOT
supported by the in-tree parser** — it produces a `.raw` directory the
local side cannot read. If you need 10× smaller PSF files: ship a binary
parser (e.g., wrap `psf_utils`). Until then, the size lever is
`save=selected` + `strobeoutput=strobeonly`, not the format.

## Transient noise (`tranNoise=yes`)

> **Source**: Same commit `2110dde` (2026-05-31), MIT-licensed.

`tran tran` is **deterministic by default** — no thermal / 1/f noise
injected. Most BSIM models have noise params but they only fire during
`noise` analysis or when `tranNoise=yes` is on the `tran` line:

```scs
tran tran stop=t_end maxstep=5p \
    tranNoise=yes noisefmax=50G noiseseed=1 noisetmin=1 binnum=16 noiseruns=1 \
    write="spectre.ic" writefinal="spectre.fc" annotate=status
```

| Param | Meaning | Default-ish value |
|---|---|---|
| `tranNoise=yes` | Enable the noise injection at all | off |
| `noisefmax=<f>` | Max frequency for noise integration | 5×Fclock or 1× signal BW (smaller = faster) |
| `noiseseed=<n>` | RNG seed for one run | 1 |
| `noisetmin=<t>` | Earliest time when noise becomes active | 0 (or 1×Ts to skip startup) |
| `binnum=<n>` | Frequency-bin discretization (Wiener model) | 16 |
| `noiseruns=<n>` | **Stochastic Monte Carlo runs** — N seeds, ensemble output | **1** (Maestro defaults to **100**, which is **100× compute**) |

**Gotcha**: When inheriting a `tran` line from Maestro, `noiseruns=100`
is common. That makes spectre repeat the full transient 100 times with
different noise seeds for ensemble statistics — fine for jitter
histograms / phase noise analyses, but **lethal for ENOB measurement**
(which only needs one realization). Override to `noiseruns=1` unless you
genuinely want ensemble.

ENOB cost of enabling noise on a 11-bit SAR: roughly −0.3 to −0.5 bit
(strongarm cmp noise is the dominant source). Compute cost of
`tranNoise=yes noiseruns=1` is ~1.5-2× a noiseless tran.

## Gotchas: Spectre 21.1 + IC618 lab cluster

> **Source**: Adapted from
> [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite),
> commit `7786645` (2026-05-31), MIT-licensed. Silent or near-silent
> foot-guns from real lab runs.

- **`-param X=Y` CLI flag is BROKEN.** Spectre 21.1 parses the value as
  a second input netlist → `SPECTRE-132: input file has been re-specified
  as 'X=Y'`. **Workaround**: bake parameters into the netlist per sweep
  point (e.g., regenerate the master with
  `txt.replace("parameters X=0", f"parameters X={val}")`).
- **`parameters X=Y` re-declaration after `include "header.scs"` does
  not update DEPENDENT expressions.** E.g., header has
  `parameters N=64 t_end=((N+N_extra)/Fs)`, then later `parameters N=256`
  — N updates but `t_end` stays at 276 ns (eagerly bound from the first
  declaration). Symptom: tran stops far too early. **Fix**: copy header
  locally and edit the `parameters` line in place.
- **Default `timeout=600 s` is too short for noised long-tran.** With
  `tranNoise=yes` + N≥256 or 6+-way parallel contention, a single run
  can exceed 600 s wall while spectre is still progressing — bridge
  reports "Remote command timed out" but `spectre.out` actually shows
  clean completion. **Fix**: bump to 3600 s in `sim run` calls.
- **PSF parser keeps `\<>` escape chars in signal names.** Saved signal
  `DOUT\<0\>` parses as dict key `r"DOUT\<0\>"`, not `"DOUT<0>"`.
  Symptom: `KeyError: 'DOUT<0>'` even though save list looks right.
- **`strobeoutput=all` in psfascii outputs only the continuous tran.**
  Despite the docs implying "both continuous + strobed", Spectre 21.1's
  psfascii emitter writes just the continuous stream into
  `tran.tran.tran`. You'll get ~140k samples per signal instead of N
  strobed values. **Fix**: either Python-strobe yourself with
  `np.searchsorted(t, k/Fs + offset)`, or use `strobeoutput=strobeonly`
  (which DOES work and shrinks the PSF ~1500×).

---
name: spectre-netlist-template
description: |
  Add stimulus and analysis templates to a Spectre netlist that has no excitation.
  Use when: (1) Ocean/ADE-generated netlist has no testbench wrapper (no vsource,
  no analysis statements), (2) you need to measure a specific figure of merit
  (GBW, PM, PSRR, CMRR, noise, load regulation, offset, propagation delay, etc.),
  (3) you are setting up standalone Spectre simulation from a bare subcircuit netlist.
  Identifies circuit type from port names / device types, then inserts the matching
  stimulus + analysis block from the canonical templates below.
author: Claude Code
version: 1.0.0
date: 2026-04-19
source: /opt/cadence/IC231/doc/spectremod/Chap1.html (vsource/isource/port syntax)
        /opt/cadence/IC231/doc/spectreref/chap3.html (analysis statements)
---

# Spectre Netlist Template — Stimulus & Analysis

All syntax verified against Cadence IC231 / Spectre 20.1 reference documentation.

---

## Step 0 — Read and Classify the Netlist

Before adding any template:
1. Read the `.scs` file; locate the top-level subcircuit declaration:
   `subckt CELL_NAME port1 port2 ...`
2. Identify circuit type from the port list + device inventory:

| Circuit type | Identifying signals | Dominant devices |
|---|---|---|
| OTA / Opamp | `VIN+` / `VIN-` / `VOUT` / `VOUTP` / `VOUTN` | MOSFETs, tail current source |
| Fully-differential OTA | Two differential outputs (`VOUTP VOUTN`) | MOSFETs + CMFB |
| LDO / Regulator | `VIN` / `VOUT` / `FB` / `EN` | pass FET, error amp |
| Comparator | `VIN+` / `VIN-` / `VOUT` / `CLK` (if clocked) | strong-arm / regenerative latch |
| Bandgap reference | `VREF` / `VBG` / `VBGR` / `IREF` | BJTs, poly resistors |
| Current mirror / bias | `IBIAS` / `IOUT` / `VBIAS` | stacked MOSFETs |
| Active filter | `VIN` / `VOUT` | OTA + C (no large devices) |
| Ring oscillator / VCO | `VCTRL` / `VOUT` / `OUT` + odd-number inverter chain | inverters |
| LNA / RF amp | `RF_IN` / `RF_OUT` / port elements | LC matching, RF MOSFETs |

If unsure, inspect instance types: `nmos4 pmos4` → analog amplifier; mostly inverters →
oscillator/digital; `bjt` / `bsim3v3` + poly resistors → bandgap.

---

## Syntax Reference (from IC231 documentation)

### vsource parameters
```spectre
// DC bias supply
VVDD (VDD 0) vsource dc=<V>
VVSS (VSS 0) vsource dc=<-V>        ; for negative rail

// AC small-signal (MUST use mag=, NOT ac=; ac= is SPICE-only → SFE-30)
VIN  (VIN 0)  vsource dc=<bias_V> mag=1     ; single-ended AC input
VIP  (VIP 0)  vsource dc=<bias_V> mag=0.5   ; differential AC input (+)
VIN_ (VIN_ 0) vsource dc=<bias_V> mag=-0.5  ; differential AC input (-)

// PSRR supply perturbation
VVDD_psrr (VDD 0) vsource dc=<V> mag=1      ; inject on supply

// Pulse (for tran)
VPULSE (NODE 0) vsource type=pulse val0=<V0> val1=<V1> \
    period=<T> rise=<tr> fall=<tf> width=<pw> delay=<td>

// Sine (for tran / distortion)
VSINE (NODE 0) vsource type=sine sinedc=<bias> ampl=<A> freq=<Hz>

// PWL (arbitrary waveform)
VPWL (NODE 0) vsource type=pwl wave=[<t0> <v0> <t1> <v1> ...]
```

### isource parameters
```spectre
// DC current bias / load
ILOAD  (VDD VOUT) isource dc=<A>      ; load current sink (+ flows from VDD into VOUT)
IREF   (VREF 0)   isource dc=<A>      ; reference current

// AC perturbation
IIN    (VIN 0)    isource dc=0 mag=1  ; AC current input

// Pulse (load step for LDO)
ISTEP (VOUT 0) isource type=pulse val0=<I0> val1=<I1> \
    period=<T> rise=<tr> fall=<tf> width=<pw>
```

### port element (S-parameter / RF)
```spectre
// Port for sp analysis — resistance sets reference impedance
PORT1 (RF_IN 0) port r=50 num=1
PORT2 (RF_OUT 0) port r=50 num=2
```

### iprobe (current probe — zero-voltage series sense)
```spectre
IPROBE0 (VOUT_INT VOUT) iprobe    ; zero-voltage drop, senses current at VOUT
```

---

## Analysis Statement Syntax (from IC231 Spectre Reference Manual)

```spectre
// ─── DC operating point ───
dcop dc oppoint=rawfile save=allpub

// ─── DC sweep (parameter) ───
dcswp dc param=<param_name> start=<val> stop=<val> step=<val> \
    oppoint=rawfile save=allpub

// ─── DC sweep (voltage/current source) ───
dcswp dc dev=<vsource_name> start=<V> stop=<V> lin=<N> oppoint=rawfile

// ─── AC frequency sweep ───
ac1 ac start=1 stop=100Meg dec=50

// ─── Transient ───
tran1 tran stop=<time> maxstep=<step> errpreset=moderate

// ─── Noise ───
//   outputport: element name (NOT node name) → use 1TΩ parallel resistor
//   inputport:  driving vsource element name
noise_an noise start=1 stop=100Meg dec=50 \
    outputport=<Rprobe_name> inputport=<Vsrc_name>

// ─── Stability (loop gain, PM) ───
//   probe: iprobe element OR element:p/n for current measurement point
stb1 stb start=1 stop=100Meg dec=50 probe=<iprobe_or_element>

// ─── Transfer function ───
xf1 xf start=1 stop=100Meg dec=50 probe=<output_node>

// ─── S-parameters ───
sp1 sp start=100Meg stop=10G dec=20 ports=[PORT1 PORT2]

// ─── Periodic Steady State (oscillator / VCO) ───
pss1 pss fund=<Hz> harms=20 errpreset=moderate maxacfreq=10G

// ─── simulatorOptions global ───
simulatorOptions options temp=27 tnom=27 scale=1e-6 \
    audioverilog=no gmin=1e-12 rforce=1 noisefloor=1e-11 \
    save=allpub dc_pivot_check=yes pivtol=1e-13 vabstol=1e-6 \
    iabstol=1e-12 compatible=spice2
```

---

## Circuit-Type Templates

### 1. OTA / Single-Ended Opamp
**Ports expected**: `VDD VSS VIN+ VIN- VOUT` (± VBIAS, VCMFB)

```spectre
// ── Supplies ──────────────────────────────────────────────
VVDD  (VDD  0) vsource dc=VDD
VVSS  (VSS  0) vsource dc=VSS          ; 0 if single-supply
VCMFB (VCMFB 0) vsource dc=VCM        ; if CMFB input exposed

// ── Common-mode bias ──────────────────────────────────────
VICM  (VCM_NODE 0) vsource dc=VCM     ; adjust to mid-rail

// ── Differential AC input: ±mag/2 for 1V differential ─────
VIP   (VIN_P 0) vsource dc=VCM mag=0.5   ; positive half
VIN_  (VIN_N 0) vsource dc=VCM mag=-0.5  ; negative half

// ── Load ──────────────────────────────────────────────────
CL    (VOUT 0) capacitor c=CL          ; load cap (parameter)
Rload (VOUT 0) resistor  r=Rload       ; if resistive load

// ── Noise oprobe (parallel, NOT series) ───────────────────
Rprobe (VOUT 0) resistor r=1T          ; 1TΩ parallel sense element

// ── Analyses ──────────────────────────────────────────────
parameters VDD=1.8 VSS=0 VCM=0.9 CL=1p Rload=1T

dcop    dc  oppoint=rawfile save=allpub
ac1     ac  start=1 stop=1G dec=50
stb1    stb start=1 stop=1G dec=50 probe=Rprobe
noise1  noise start=1 stop=1G dec=50 outputport=Rprobe inputport=VIP
```

**Measurements from results**:
- GBW: AC magnitude crosses 0 dB → read frequency
- Phase margin: phase at GBW frequency
- Loop gain (stb): `loopGain` signal in PSF; PM from phase of loop gain
- Input-referred noise: `noise` analysis → `in` (input-referred) signal

---

### 2. Fully-Differential OTA
**Ports expected**: `VDD VSS VIN+ VIN- VOUTP VOUTN VCMFB`

```spectre
VVDD  (VDD  0) vsource dc=VDD
VVSS  (VSS  0) vsource dc=VSS
VCMFB (VCMFB 0) vsource dc=VCM        ; ideal CMFB for open-loop test

VIP   (VIN_P 0) vsource dc=VCM mag=0.5
VIN_  (VIN_N 0) vsource dc=VCM mag=-0.5

// Differential output load
CLp   (VOUTP 0) capacitor c=CL
CLn   (VOUTN 0) capacitor c=CL
Rprobep (VOUTP 0) resistor r=1T
Rproben (VOUTN 0) resistor r=1T

parameters VDD=1.8 VSS=0 VCM=0.9 CL=1p

dcop   dc  oppoint=rawfile save=allpub
ac1    ac  start=1 stop=1G dec=50
stb1   stb start=1 stop=1G dec=50 probe=Rprobep
noise1 noise start=1 stop=1G dec=50 outputport=Rprobep inputport=VIP
```

---

### 3. LDO / Voltage Regulator
**Ports expected**: `VIN VOUT FB GND` (± EN, VBIAS)

```spectre
VVIN  (VIN  0) vsource dc=VIN
VCMFB (FB   0) vsource dc=VFB         ; if FB is a direct pin (no resistor divider)
// or: external resistor divider R1/R2 from VOUT to FB to GND

// Load: nominal + step
ILOAD (VOUT GND_NODE) isource dc=ILOAD_DC
ISTEP (VOUT GND_NODE) isource type=pulse \
    val0=ILOAD_DC val1=ILOAD_MAX delay=5u period=30u rise=100n fall=100n width=10u

// Loop-break iprobe for Bode plot
Ibrk  (VOUT_SENSE VOUT) iprobe        ; insert in feedback path

// Noise probe
Rprobe (VOUT GND_NODE) resistor r=1T

parameters VIN=3.3 VREF=1.2 VFB=VREF ILOAD_DC=1m ILOAD_MAX=100m

dcop   dc   oppoint=rawfile save=allpub
// Line regulation
linereg dc  dev=VVIN start=2 stop=5 lin=31
// Load regulation
loadreg dc  dev=ILOAD start=0 stop=ILOAD_MAX lin=51
stb1    stb  start=1 stop=100Meg dec=50 probe=Ibrk
tran1   tran stop=100u maxstep=10n errpreset=moderate   ; load step
noise1  noise start=1 stop=10Meg dec=50 outputport=Rprobe inputport=VVIN
```

---

### 4. Comparator (static / strong-arm)
**Ports expected**: `VDD VSS VIN+ VIN- VOUT CLK` (clocked) or `VIN+ VIN- VOUT` (static)

```spectre
VVDD (VDD 0) vsource dc=VDD
VVSS (VSS 0) vsource dc=VSS

// Static comparator: ramp one input, hold the other at VCM
VPOS (VIN_P 0) vsource type=pwl \
    wave=[0 VCM  1n VCM  101n VSTOP]   ; slow ramp to find threshold
VNEG (VIN_N 0) vsource dc=VCM

// Clocked comparator: differential pulse, external CLK
VCLK  (CLK 0) vsource type=pulse val0=0 val1=VDD \
    period=TCLK rise=100p fall=100p width=TCLK_H delay=0
VDIFF (VIN_P VIN_N) vsource dc=0 type=pulse \
    val0=-VDIFF_NEG val1=VDIFF_POS rise=1p fall=1p width=TCLK delay=TCLK_H

parameters VDD=1.8 VSS=0 VCM=0.9 VSTOP=VDD TCLK=2n TCLK_H=1n \
           VDIFF_NEG=10m VDIFF_POS=10m

dcop  dc   oppoint=rawfile save=allpub
tran1 tran stop=10n maxstep=10p errpreset=moderate

// Hysteresis (static comparator)
hysteresis dc dev=VPOS start=0 stop=VDD lin=101 hysteresis=yes
```

**Measurements**:
- Propagation delay: tran → time from CLK edge to VOUT 50% crossing
- Offset: DC sweep → VOUT switches midpoint vs VCM
- Hysteresis: DC hysteresis sweep → forward/reverse threshold difference

---

### 5. Bandgap Reference
**Ports expected**: `VDD GND VREF` (± IREF, VSTART, VTRIM)

```spectre
VVDD  (VDD 0) vsource dc=VDD

// Supply ramp for startup check
VRAMP (VDD_RAMP 0) vsource type=pwl \
    wave=[0 0  100n 0  1u VDD_MAX]    ; slow ramp on supply rail

// Noise probe
Rprobe (VREF 0) resistor r=1T

parameters VDD=3.3 VDD_MAX=3.6 TEMP_START=-40 TEMP_STOP=125

dcop   dc  oppoint=rawfile save=allpub

// Line regulation: VREF vs VDD
linereg dc  dev=VVDD start=1.5 stop=VDD_MAX lin=51

// Temperature coefficient: requires corner sweep (use parametric or Monte Carlo)
// tempco  dc  param=temp start=TEMP_START stop=TEMP_STOP lin=166

// PSRR
VVDD_psrr (VDD 0) vsource dc=VDD mag=1   ; AC inject on supply
psrr   ac  start=1 stop=100Meg dec=50

noise1 noise start=1 stop=10Meg dec=50 outputport=Rprobe inputport=VVDD_psrr

tran1  tran stop=10u maxstep=10n errpreset=moderate   ; startup check
```

**Measurements**:
- TC [ppm/°C]: `(VREF_max - VREF_min) / (VREF_nom * ΔT) × 1e6`
- PSRR: AC analysis → 20log(VREF_out / VVDD_in)
- Line regulation: DC sweep → dVREF/dVDD [mV/V]

---

### 6. Current Mirror / Bias Generator
**Ports expected**: `VDD VSS IBIAS_IN IBIAS_OUT` (± VBIAS, VCASC)

```spectre
VVDD  (VDD 0) vsource dc=VDD
VVSS  (VSS 0) vsource dc=VSS

// Reference current source
IREF  (VDD IBIAS_IN) isource dc=IREF_DC

// Output compliance sweep
VCMP  (IBIAS_OUT 0) vsource dc=VSTART  ; sweeps to check compliance

// Small-signal: output impedance
VTEST (IBIAS_OUT_AC IBIAS_OUT) vsource dc=0 mag=1  ; inject test voltage

parameters VDD=1.8 VSS=0 IREF_DC=10u VSTART=0.1

dcop    dc  oppoint=rawfile save=allpub
// Mirror ratio check
compliance dc  dev=VCMP start=0.1 stop=VDD lin=51

// Output impedance via AC
ac_rout ac  start=1 stop=100Meg dec=50     ; measure ITEST/VTEST at IBIAS_OUT
```

---

### 7. Active Filter (OTA-C / Gm-C / Sallen-Key)
**Ports expected**: `VDD VSS VIN VOUT`

```spectre
VVDD (VDD 0) vsource dc=VDD
VVSS (VSS 0) vsource dc=VSS

// AC input (singled-ended)
VIN_ac (VIN 0) vsource dc=VCM mag=1

// Step input for tran
VSTEP (VIN 0) vsource type=pulse val0=0 val1=VSTEP_AMP \
    delay=100n rise=1p fall=1p width=10u period=20u

// Output sense
Rprobe (VOUT 0) resistor r=1T

parameters VDD=1.8 VSS=0 VCM=0.9 VSTEP_AMP=100m

dcop  dc  oppoint=rawfile save=allpub
ac1   ac  start=1 stop=1G dec=100       ; frequency response (fine resolution)
tran1 tran stop=50u maxstep=1n errpreset=moderate
noise1 noise start=1 stop=1G dec=50 outputport=Rprobe inputport=VIN_ac
```

---

### 8. Ring Oscillator / VCO
**Ports expected**: `VDD VSS OUT` (± VCTRL for VCO)

```spectre
VVDD  (VDD 0) vsource dc=VDD
VVSS  (VSS 0) vsource dc=VSS
VCTRL (VCTRL 0) vsource dc=VCTRL_NOM   ; VCO control voltage

parameters VDD=1.8 VSS=0 VCTRL_NOM=0.9 Tosc=2n

// Tran: measure oscillation period
tran1 tran stop=100n maxstep=1p errpreset=moderate skipdc=no

// PSS: periodic steady state → spectral purity
pss1 pss fund=1/Tosc harms=20 errpreset=moderate maxacfreq=100G

// VCO gain (KVCO): DC sweep of VCTRL
vco_kvco dc dev=VCTRL start=0.2 stop=VDD-0.2 lin=51
```

**Measurements from tran**: zero-crossings of OUT → period → frequency
**PSS**: phase noise, KVCO from pss/pac

---

### 9. LNA / RF Amplifier (S-parameters)
**Ports expected**: `VDD VSS RF_IN RF_OUT` (± VBIAS, IBIAS)

```spectre
VVDD (VDD 0) vsource dc=VDD
VVSS (VSS 0) vsource dc=VSS
VBIAS (VBIAS 0) vsource dc=VBIAS_NOM

// S-parameter ports (50Ω reference)
PORT1 (RF_IN  0) port r=50 num=1
PORT2 (RF_OUT 0) port r=50 num=2

parameters VDD=1.2 VSS=0 VBIAS_NOM=0.6

dcop dc oppoint=rawfile save=allpub

// S-parameter sweep
sp1 sp start=100Meg stop=10G dec=20 ports=[PORT1 PORT2]

// Noise figure (input-referred noise)
noise1 noise start=100Meg stop=10G dec=20 \
    outputport=PORT2 inputport=PORT1

// Large-signal tran (1-dB compression point — vary ampl)
tran1 tran stop=50n maxstep=1p errpreset=moderate
```

**Measurements**:
- S11 (return loss), S21 (gain), S12 (isolation), S22 (output match) from sp analysis
- Noise figure from noise analysis `in` signal
- IIP3: two-tone tran → FFT

---

## Workflow

1. **Read netlist** → identify subcircuit name + port list
2. **Match circuit type** from table in Step 0
3. **Choose template** from the matching section above
4. **Instantiate the subcircuit** at top level:
   ```spectre
   simulator lang=spectre
   global 0
   include "input.scs"         ; ADE-generated subcircuit
   
   parameters VDD=1.8 ...
   
   // ── DUT instantiation ──
   XDUT (VDD VSS VIN_P VIN_N VOUT) CELL_NAME
   
   // ── Stimulus from template ──
   ...
   
   // ── Analyses from template ──
   ...
   ```
5. **Adjust** node names to match actual DUT port order
6. **Verify** `mag=1` on all AC excitation sources (not `ac=1`)
7. **Run**: `spectre input.scs -raw psf -format psfascii`

---

## Automated Script: inject_stimulus.py

For batch or scripted use, the skill ships a Python helper that auto-detects circuit
type and generates a complete testbench file — no Virtuoso running required.

```bash
# Auto-detect from ADE-exported netlist, write bandgap_tb.scs
python ${CLAUDE_SKILL_DIR}/scripts/inject_stimulus.py bandgap.scs

# Explicit type override
python ${CLAUDE_SKILL_DIR}/scripts/inject_stimulus.py amp.scs --type ota

# Override output path
python ${CLAUDE_SKILL_DIR}/scripts/inject_stimulus.py amp.scs -o amp_testbench.scs

# Pass cell name to help disambiguation (e.g. "Bandgap" → bandgap type)
python ${CLAUDE_SKILL_DIR}/scripts/inject_stimulus.py chip.scs --cell Bandgap

# List detected subcircuits and bail (useful for inspection)
python ${CLAUDE_SKILL_DIR}/scripts/inject_stimulus.py chip.scs --list

# Then run directly with Spectre
spectre bandgap_tb.scs -raw psf -format psfascii
```

The script requires no third-party libraries (stdlib only).
After generation, review and adjust parameter values (VDD, CL, temperatures, etc.)
to match the target PDK and operating conditions.

---

## Common Mistakes (see also spectre-netlist-gotchas)

| Mistake | Symptom | Fix |
|---------|---------|-----|
| `ac=1` on vsource | SFE-30 | Use `mag=1` |
| No `mag=` on AC source | AC output all zeros | Add `mag=1` (or `mag=0.5`) |
| Series resistor as oprobe | Noise result millions × too high | Use 1TΩ parallel Rprobe |
| node name as oprobe/iprobe | SFE-1997 | Use element name, not node |
| `val0`/`val1` wrong units | Pulse has wrong swing | Check V vs A (vsource vs isource) |
| Missing `global 0` | Ground not connected | Add `global 0` at top |
| Wrong port order in XDUT | Circuit biased wrong | Match port order to subckt declaration |

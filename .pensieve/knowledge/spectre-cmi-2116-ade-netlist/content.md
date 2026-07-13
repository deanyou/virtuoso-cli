---
id: content
type: knowledge
title: Spectre CMI-2116: ADE Netlist MOSFET Terminals Missing
status: active
created: 2026-05-27
updated: 2026-05-27
tags: ["knowledge", "spectre"]
---

# Spectre CMI-2116: ADE Netlist MOSFET Terminals Missing

## Source
Session 2026-04-19: FT0001A_SH/CMOP/schematic standalone simulation.
Repeated CMI-2116 errors when running ADE-generated netlist without `-env ade` token.

## Summary
ADE-generated netlists write MOSFET instance lines without terminal connections; terminals are
resolved at runtime from the OA database via the `+adespetkn` token. Running these netlists
with `spectre` directly always fails with CMI-2116. The fix is to build a topology-complete
standalone netlist by extracting terminal connections from the OA schematic via SKILL.

## Content

### Error Pattern

```
Error found by spectre during circuit read-in.
    ERROR (CMI-2116): subckt `CMOP': device `M2': Too few terminals: 0 < 4
spectre completes with N errors, 0 warnings, and 0 notices.
```

Occurs even when:
- The model file path is correct (SFE-868 is not the cause)
- The netlist is copied to a new directory
- Different runs of the same ADE-generated netlist are tried

### Root Cause

ADE netlists write MOSFET lines as:
```spectre
M2 n33 w=(280n) l=350n ...    ← NO terminal net connections
M3 p33 w=(280n) l=300n ...    ← device type + params only
```

The terminal-to-net mapping is stored in the OA binary database.
When Spectre runs with `-env ade +adespetkn=TOKEN`, it reads the OA database live to resolve
connections. Without that token, terminal count = 0, triggering CMI-2116.

**The token approach itself is not reliable for one-off verification**: even `ExplorerRun.0`
with a valid `adespe` token can fail with CMI-2116 if the OA database state is inconsistent.

### Fix: Build Topology-Complete Standalone Netlist

**Step 1 — Extract terminal connections from OA via SKILL bridge**

```skill
; Open schematic read-only
cv = dbOpenCellViewByType("LIB" "CELL" "schematic" nil "r")

; For each instance: print name + terminal→net mapping
foreach(inst cv~>instances
    printf("%-6s" inst~>name)
    foreach(it inst~>instTerms
        printf(" %s=%s" it~>term~>name it~>net~>name))
    printf("\n"))
```

**Step 2 — Map CDF terminal order to Spectre order**

For SMIC 0.13µm mmRF (see [[smic-mosfet-terminal-order]] for full table):

| Model | CDF instTerms order | Spectre (d g s b) order |
|-------|--------------------|-----------------------|
| `n33` | S B G D | D=T[3], G=T[2], S=T[0], B=T[1] |
| `p33` | S G D B | D=T[2], G=T[1], S=T[0], B=T[3] |

**Step 3 — Write complete subckt with all terminals explicit**

```spectre
subckt CMOP BIAS VDD VIN VIP VOUT VSS
    M1 (VB1 VIN net19 VSS) n33 w=280n l=350n ... m=2
    M2 (VB2 VIP net19 VSS) n33 w=280n l=350n ... m=2
    ...
ends CMOP
```

**Step 4 — Run spectre directly**

```bash
spectre input.scs +escchars +log psf/spectre.out \
  -format psfascii -raw psf +mt -maxw 5 -maxn 5
```

Use `-format psfascii` for human-readable output (easier to parse without Ocean session).

### Body Terminals (e.g. SUB1)

Internal body-tie nets connected only to MOSFET body terminals (no other device) should be
tied to VSS in the standalone netlist. These are fine to leave as explicit internal nets too.

### Testbench VDD Wiring

Check: the CMOP VDD port should connect to `vdd!` (supply), not to a node created by a
series voltage source (which produces VDD = 0 V). Use:
```spectre
I0 (net4 vdd! net1 net3 VOUT 0) CMOP
```
not `I0 (net4 net14 ...) CMOP` where `net14` is set to 0 V by a current-measuring V-source.

## When to Use
- When `spectre input.scs` on an ADE-generated netlist gives CMI-2116
- When the `-env ade` approach is unavailable, unreliable, or explicitly avoided
- When building a standalone Spectre testbench from scratch for a cell in the OA database

## Context Links
- Based on: [[smic-mosfet-terminal-order]] — CDF vs Spectre terminal order
- Based on: [[spectre-ade-model-path]] — other ADE→standalone translation issues
- Leads to: [[2026-04-19-standalone-spectre-for-one-off-verification]]

# SMIC 0.13µm mmRF: CDF Terminal Order vs Spectre Order

## Source
Session 2026-04-18: FT0001A_SH/Bandgap/schematic DC simulation.
VBG measured as 3.14 V (should be ~1.2 V). Root cause: all MOSFET drain/source
positions swapped in standalone netlist due to CDF vs Spectre terminal order mismatch.

## Summary
SMIC 0.13µm mmRF p33/n33 CDF defines terminals as (S G D B) but Spectre BSIM3v3
expects (D G S B) — writing CDF order directly into .scs swaps drain and source on
every transistor, destroying circuit function.

## Content

### Terminal Order Table

| Model | CDF Order (Virtuoso/SKILL) | Spectre .scs Order |
|-------|---------------------------|---------------------|
| `p33` (PMOS 3.3V IO) | **S G D B** | **D G S B** |
| `n33` (NMOS 3.3V IO) | **S G D B** | **D G S B** |
| `pnp33a4` | Per-instance (varies) | **C B E** |

### Symptom

- DC-OP shows output node at rail or supply voltage (VBG = 3.14 V for a ~1.2 V bandgap)
- All PMOS drains wired to VDDA (supply) instead of actual drain nets
- All NMOS sources wired to GNDA instead of actual source nets
- Simulation converges without error — this is a **silent functional failure**

### Diagnosis

Check a known-good transistor. For a diode-connected PMOS (`M33` in bandgap):
- Correct Spectre: `M33 (VBG net068 VDDA VDDA) p33 ...` — drain=VBG, source=VDDA
- Wrong (CDF order): `M33 (VDDA VBG net068 VDDA) p33 ...` — drain=VDDA (rail!)

### Fix Workflow for Standalone Netlists

1. Query terminal names via SKILL (not positions):
```skill
; For each instance, get name→net mapping
let((cv)
    cv = dbOpenCellViewByType("LIB" "CELL" "schematic" nil "r")
    foreach(inst cv~>instances
        printf("%-6s" inst~>name)
        foreach(it inst~>instTerms
            printf(" %s=%s" it~>term~>name it~>net~>name))
        printf("\n")))
```

2. Build a terminal dict keyed by name, then write in Spectre order:
```python
# CDF gives (S G D B) → write Spectre (D G S B)
TERMINALS = {
    "M33": "(VBG net068 VDDA VDDA)",   # D=VBG G=net068 S=VDDA B=VDDA
    "Q0":  "(SUB SUB net0100)",          # C=SUB B=SUB E=net0100
}
```

3. For BJTs: always resolve by terminal name. `pnp33a4` CDF term order varies
   per instance due to how Virtuoso stores instTerms for PCell devices.

### Warning: ADE Re-Netlist Overwrites Fixes

Every `vcli sim netlist --recreate` regenerates `input.scs` in CDF terminal order.
Maintain the terminal fix as a patch script and reapply after each re-netlist.

## When to Use
- When writing or patching standalone Spectre netlists for SMIC 0.13µm mmRF
- When DC simulation converges but output nodes are at wrong voltages (rail, ground)
- When starting from an ADE-generated netlist for direct `spectre input.scs` invocation
- Before running `vcli optim run` with a netlist template (batch jobs inherit the bug)

## Context Links
- Based on: [[spectre-ade-model-path]] — other ADE→standalone translation issues
- Related: [[skill-instterms-cons-reversal]] — SKILL gotcha when querying terminal names
- Related: [[spectre-netlist-gotchas]] — §10 in skill file for CDF/Spectre order table

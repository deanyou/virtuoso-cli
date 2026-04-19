# SMIC rhrpo_3t_ckt PCells: Missing Terminal Connections in Ocean-Generated Netlist

## Source
Session 2026-04-19: `vcli sim run` on FT0001A_SH/Bandgap failed with SFE-45.
Root cause: Ocean netlisted `rhrpo_3t_ckt` schematic view, creating subcircuits
with resistor instances that had zero terminal connections.

## Summary
When Ocean netlists `rhrpo_3t_ckt` from the `smic13mmrf_1233` schematic view,
the generated subcircuit (e.g. `rhrpo_3t_ckt_pcell_1`) contains resistor instances
with model name and parameters but NO connection nodes — Spectre SFE-45 fatal error.

## Content

### Symptom

```
ERROR (SFE-45): "input.scs" 19: Cannot run the simulation because the instance
    `R23' of `rhrpo_3t_ckt' requires 3 terminals, but has only 0 terminal specified.
```

### What Ocean Generates (Broken)

```spectre
subckt rhrpo_3t_ckt_pcell_1 B MINUS PLUS
parameters segL=10u segW=2u mismod_res=1
    R23 rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
    R22 rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
    ...
ends rhrpo_3t_ckt_pcell_1
```

The resistors have no `(node1 node2 bulk)` connection list before the model name.

### Root Cause

`rhrpo_3t_ckt` in `smic13mmrf_1233` library has a schematic view whose internal
resistor PCell instances have floating/unrouted connections. The PDK intends for
`rhrpo_3t_ckt` to be treated as a primitive (defined in `ms013_io33_v2p6_7p_res_spe.ckt`),
not netlisted through its schematic hierarchy.

### Fix: LLM-Assisted Inline Repair

Since this is an intermittent structural issue tied to a specific PDK version and
the netlist regenerates on each `vcli sim run`, do NOT write a maintenance script.
Instead, let LLM read the broken subcircuit and reconstruct the correct series chain:

**Correct pattern for `rhrpo_3t_ckt_pcell_N` with N segments:**

```spectre
// N=1 (single segment):
subckt rhrpo_3t_ckt_pcell_3 B MINUS PLUS
parameters segL=10u segW=2u mismod_res=1
    R0 (PLUS MINUS B) rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
ends rhrpo_3t_ckt_pcell_3

// N=3 (series chain, PLUS→_n0→_n1→MINUS, bulk=B):
subckt rhrpo_3t_ckt_pcell_4 B MINUS PLUS
parameters segL=10u segW=2u mismod_res=1
    R2 (PLUS _n0 B) rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
    R1 (_n0 _n1 B) rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
    R0 (_n1 MINUS B) rhrpo_3t_ckt m=1 l=segL w=segW mismod_res=mismod_res
ends rhrpo_3t_ckt_pcell_4
```

Terminal order for `rhrpo_3t_ckt`: `(PLUS MINUS BULK)`.
Subcircuit ports are declared as `B MINUS PLUS` (B=bulk is first in port list).

### Known PCells in FT0001A_SH/Bandgap (as of 2026-04-19)

| Subcircuit name | Segment count |
|-----------------|--------------|
| rhrpo_3t_ckt_pcell_1 | 24 |
| rhrpo_3t_ckt_pcell_2 | 2 |
| rhrpo_3t_ckt_pcell_3 | 1 |
| rhrpo_3t_ckt_pcell_4 | 3 |

### Alternative: Run Spectre Standalone on Patched Netlist

Since Ocean regenerates the netlist on each `sim run`, a patched version cannot
survive subsequent Ocean invocations. For standalone Spectre runs (no live ADE):

1. Run `vcli sim netlist` or trigger `createNetlist` once
2. LLM repairs the broken subcircuits inline (Edit tool)
3. Run Spectre directly: `spectre input.scs -raw psf -format psfascii`
4. Read PSF results with `vcli skill exec 'openResults("...")'`

## When to Use
- When `vcli sim run` fails with SFE-45 on a SMIC bandgap or resistor-heavy circuit
- When Ocean-generated netlist has `rhrpo_3t_ckt_pcell_N` subcircuits with empty instance bodies
- When netlisting any cell from `smic13mmrf_1233` that uses `rhrpo_3t_ckt` PCells

## Context Links
- Based on: [[spectre-bandgap-dc-convergence]] — further patching needed after fixing terminals
- Related: [[ocean-createnetlist-prerequisites]] — full netlist generation setup
- Related: [[smic-mosfet-terminal-order]] — similar terminal-ordering issue for MOSFETs

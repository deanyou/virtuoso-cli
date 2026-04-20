# Standalone Spectre for One-Off Circuit Verification

## One-line Conclusion
> For one-off simulation of a single cell, extract OA topology via SKILL and build a
> topology-complete `input.scs`; do not attempt ADE token or Maestro path.

## Context Links
- Based on: [[spectre-cmi-2116-ade-netlist]]
- Based on: [[smic-mosfet-terminal-order]]
- Based on: [[spectre-ade-model-path]]
- Related: [[cadence-ic23-dbopencellviewbytype]]
- Related: [[maestro-netlist-standalone-workflow]] — n12/p12 快捷路径（无需 SKILL 提取）

## Context

Needed to run DC op-point, DC offset sweep, and transient simulation on
FT0001A_SH/CMOP/schematic — a static comparator — without setting up a full ADE session.

## Problem

Three approaches were attempted before the standalone approach succeeded:

1. **`spectre input.scs` on ADE netlist** → CMI-2116 (MOSFET terminals absent)
2. **`spectre input.scs -env ade +adespetkn=adespe`** → CMI-2116 (token resolves OA DB;
   ExplorerRun.0's token `adespe` was stale/inconsistent)
3. **`maeRunSimulation(sess)` via SKILL bridge** → Virtuoso modal dialog blocked bridge for
   30 s; required user to manually dismiss

## Alternatives Considered

- **ADE token approach**: fundamentally depends on a live, consistent OA database state
  accessible from where spectre runs. Unreliable for one-off use. Also requires the exact
  `adespe` token from a current ADE run.
- **Maestro/ADE GUI path via bridge**: `maeRunSimulation` triggers a modal confirmation
  dialog ("ADE Explorer Update and Run"). Virtuoso's single-threaded SKILL evaluator blocks
  all bridge calls until the user clicks OK. Not usable in automated/unattended workflows.
- **Re-running from ADE GUI**: works but requires manual interaction and does not produce a
  reusable netlist for scripted flow.

## Decision

Build a topology-complete standalone `input.scs`:

1. `dbOpenCellViewByType("LIB" "CELL" "schematic" nil "r")` — open read-only
2. `foreach(inst cv~>instances ...)` with `instTerms` — extract terminal→net mapping
3. Map CDF terminal order to Spectre `(d g s b)` order (see [[smic-mosfet-terminal-order]])
4. Write complete `subckt` with all terminals explicit; tie floating body nodes to VSS
5. Run: `spectre input.scs -format psfascii -raw psf +escchars +mt`
6. Parse results with `awk` on `.dc` / `.tran` ASCII PSF files

## Consequence

- spectre ran in <1 s with 0 errors; all three analyses (dcop, DC sweep, tran) completed
- No ADE/Ocean/Maestro dependency at simulation time
- PSF parsed with awk — no Ocean session required
- Results: VOUT=0.913 V (metastable), Voffset=34 mV, tpd≈13.7 ns, swing≈rail-to-rail

## Exploration Reduction

- **What to ask less next time**: "Can the ADE token approach work standalone?" → No.
  CMI-2116 on ADE netlist = go straight to SKILL topology extraction.
  **For n12/p12 cells**: Maestro netlist 端口完整，不需要 SKILL 提取 → 走 [[maestro-netlist-standalone-workflow]]。
- **What to look up less next time**: CDF terminal order for n33/p33 → already in
  [[smic-mosfet-terminal-order]]; SKILL extraction snippet → in [[spectre-cmi-2116-ade-netlist]].
- **Invalidation condition**: If Cadence adds a public API to export topology-complete
  netlists without ADE token (e.g., a `createNetlist` variant with explicit terminals), or
  if the bridge gains Ocean PSF reading capability, reconsider.

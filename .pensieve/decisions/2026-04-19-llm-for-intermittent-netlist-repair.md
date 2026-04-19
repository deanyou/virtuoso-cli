# LLM-Assisted Inline Repair for Intermittent Netlist Issues

## One-line Conclusion
> Use LLM to read and fix broken netlist sections inline; do not write patch scripts for intermittent structural issues.

## Context Links
- Based on: [[smic-rhrpo-netlist-terminals]] — the specific case that motivated this
- Based on: [[smic-mosfet-terminal-order]] — similar terminal-order issues, same fix pattern
- Related: [[ocean-createnetlist-prerequisites]] — general netlisting setup

## Context

Ocean netlisting for SMIC PDK cells occasionally produces subcircuits with missing
or incorrectly ordered terminal connections. Examples seen:
- `rhrpo_3t_ckt` PCells: resistor instances with 0 terminal connections (SFE-45)
- MOSFET instances: missing `(D G S B)` connection list (SFE-45)
- BJT instances: missing `(C B E)` connection list

These issues are:
1. **Intermittent** — they depend on PDK version, schematic version, and which
   subcells Ocean chooses to expand vs. treat as primitives
2. **Transient** — Ocean overwrites the netlist on every `sim run`, so any patch is
   lost immediately

## Problem

When SFE-45 appears, the temptation is to write a Python script that parses and
fixes the netlist. That script must hard-code:
- Which PCells are broken
- How many segments each PCell has
- The correct terminal order for each device type
- The internal node naming convention

All of these change when the schematic changes, making the script a maintenance liability.

## Alternatives Considered

- **Write a Python patch script**: Hard-codes segment counts and terminal orders.
  Breaks on next schematic update. Requires re-running after every Ocean netlist regeneration.

- **Fix at the CDF level (tell Ocean to use .ckt view)**: Correct long-term fix but
  requires PDK admin access and Ocean CDF knowledge; overkill for a simulation task.

- **LLM inline repair (chosen)**: Read the broken subcircuit, understand its
  structure from the context, apply the correct series-chain topology and terminal
  connections using Edit tool. Adapts naturally to any segment count.

## Decision

When a netlist has broken terminal connections due to PDK PCell expansion:

1. Read the broken subcircuit section with the Read tool
2. Ask LLM to reconstruct the correct topology (series chain for resistor PCells,
   correct D/G/S/B for MOSFETs) based on the port declaration and segment count
3. Apply with Edit tool directly — no script, no loop
4. Run Spectre standalone (not via Ocean `sim run`) to preserve the fix

## Consequence

- No scripts to maintain
- Fix is applied per-session (acceptable — we generally don't need to re-run the same broken netlist repeatedly)
- LLM naturally handles variations in segment count and topology

## Exploration Reduction
- What to ask less next time: "should I write a script to fix this?"
- What to look up less next time: segment counts for each PCell (check the netlist itself)
- Invalidation condition: PDK provides a proper Spectre view for rhrpo_3t_ckt that Ocean uses directly

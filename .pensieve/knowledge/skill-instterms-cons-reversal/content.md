# SKILL instTerms: cons/foreach Reversal

## Source
Session 2026-04-18: Building terminal→net mapping for SMIC Bandgap MOSFET instances.
Initial patch used cons-in-foreach to collect terminal names, producing reversed lists
that combined with CDF terminal order mismatch to create compounding errors.

## Summary
`cons` in a `foreach` loop produces a **reversed** list. When building MOSFET terminal
assignment strings from `instTerms`, this reversal multiplies with CDF-order mismatch
and creates wrong Spectre netlist entries that are hard to diagnose.

## Content

### The Trap

```skill
; WRONG — result is in reverse CDF order
result = nil
foreach(it inst~>instTerms
    result = cons(it~>net~>name result))
; CDF order (S G D B) → returns ("B_net" "D_net" "G_net" "S_net")
```

For SMIC p33/n33 with CDF order (S G D B), this produces reversed order (B D G S).
If the developer then tries to correct for CDF→Spectre swap, the starting point is
already wrong, making the correction apply to the wrong positions.

### Correct Approaches

```skill
; Option 1: mapcar — preserves CDF order
mapcar(lambda((it) it~>net~>name) inst~>instTerms)
; returns ("S_net" "G_net" "D_net" "B_net") — CDF order, then apply known swap

; Option 2: name→net alist — order-independent, most robust
let((result)
    result = list()
    foreach(it inst~>instTerms
        result = append(result list(list(it~>term~>name it~>net~>name))))
    ; result = (("S" "S_net") ("G" "G_net") ("D" "D_net") ("B" "B_net"))
    ; Lookup by name: assoc("D" result) → ("D" "D_net")
    )
```

### Recommended Pattern: Name-Based Lookup

Always resolve terminal nets by name, not by position, when building Spectre statements:

```python
# After querying name→net pairs from SKILL:
terms = {"S": "VDDA", "G": "net068", "D": "VBG", "B": "VDDA"}

# Write Spectre order (D G S B) using names:
spectre_terms = f"({terms['D']} {terms['G']} {terms['S']} {terms['B']})"
# → "(VBG net068 VDDA VDDA)"
```

### Verification

After building any terminal string, spot-check a diode-connected device:
- PMOS diode-connected: D=G must be the same net (both at output node)
- NMOS diode-connected: D=G must be the same net
- If D≠G for a diode-connected device, the terminal mapping is wrong

## When to Use
- When writing SKILL code to extract instTerms for netlist generation
- When building a terminal→net dict from `foreach(inst~>instTerms ...)`
- Any time instTerms output doesn't match expected circuit connectivity

## Context Links
- Related: [[smic-mosfet-terminal-order]] — CDF vs Spectre terminal order for SMIC PDK
- Related: [[skill-shell-gotchas]] — other SKILL language gotchas

---
name: ocean-netlist-regen
description: |
  Fix Ocean SKILL simulation failures after schematic edits. Use when:
  (1) run() returns nil in <1s with no spectre.out, (2) createNetlist() returns nil,
  (3) sim results are all nil after changing W/L to design variables,
  (4) netlist file was deleted or is stale, (5) sim run previously worked but
  now fails after calling `sim setup` again. Covers: the resultsDir binding trap
  (MOST COMMON), stale netlists, sim setup disrupting sessions, direct spectre
  invocation as bypass, and PSF signal naming (I0.NM0 prefix).
author: Claude Code
version: 2.0.0
date: 2026-04-06
---

# Ocean run() Reliability & Netlist Regeneration

## Problem

Ocean's `run()` silently returns nil. All measurement results are nil. Spectre
never runs. No error is reported.

## Context / Trigger Conditions

- `run()` completes quickly and returns nil instead of the resultsDir path
- No `spectre.out` or stale one in psf directory  
- `createNetlist(?recreateAll t)` returns nil
- Previously called `sim setup` or changed `resultsDir` to a different path
- Previously working simulation stops producing results

---

## Root Cause 1: resultsDir Change Breaks run() ⚠️ MOST COMMON

**The ADE session binds `run()` to the resultsDir that was active when the session
was first established from the GUI.** Changing it to ANY other path causes `run()`
to silently return nil.

```skill
; ✅ Works — same path as ADE session
resultsDir("/tmp/opt_5t_ota")
run()   ; → "/tmp/opt_5t_ota"

; ❌ Breaks run() — different path
resultsDir("/tmp/new_dir")
run()   ; → nil (silent failure!)

; ✅ Restore original path, run() works again
resultsDir("/tmp/opt_5t_ota")
run()   ; → "/tmp/opt_5t_ota"
```

**Find the canonical resultsDir** from the runSimulation script:
```bash
cat <netlist_dir>/runSimulation | grep "\-raw"
# → -raw ../../../../../../../../tmp/opt_5t_ota/psf
# Canonical path = /tmp/opt_5t_ota
```

**Fix in virtuoso-cli**: `sim run` no longer auto-sets `resultsDir` to a temp dir
(that was overriding the ADE session path). If resultsDir is nil, `sim run` now
returns a clear error message.

---

## Root Cause 2: sim setup Disrupts Active Session

Calling `virtuoso sim setup` sends `design("lib" "cell" "view")` which can reset
the Ocean session state. After this, `run()` returns nil.

**Symptom**: `run()` worked → called `sim setup` → `run()` now returns nil.

**Fix**: Don't call `sim setup` again if a working session exists. Check:
```bash
virtuoso skill exec 'asiGetSession(hiGetCurrentWindow())'
# → stdobj@0x... means session is alive
```

---

## Root Cause 3: Stale or Missing Netlist

After editing a schematic (e.g., adding desVar variables), the netlist becomes stale.
`createNetlist()` requires an ADE L window open for that cell — without it, returns nil.

---

## Solution A: Restore resultsDir (for Root Cause 1)

```bash
virtuoso skill exec 'resultsDir("/tmp/opt_5t_ota")'
virtuoso sim run --analysis dc --timeout 60
```

---

## Solution B: Direct Spectre Invocation (most reliable, bypasses Ocean)

```bash
NETLIST_DIR="/path/to/simulation/cell/spectre/schematic/netlist"
PSF_DIR="/tmp/my_results/psf"
mkdir -p "$PSF_DIR"

cd "$NETLIST_DIR" && spectre input.scs \
  +escchars \
  +log "$PSF_DIR/spectre.out" \
  -format psfxl -raw "$PSF_DIR" \
  +mt -maxw 5 -maxn 5
```

Then read results:
```bash
virtuoso skill exec 'openResults("/tmp/my_results/psf")'
virtuoso sim measure --analysis dcOp \
  --expr 'getData("I0.NM0:gm" ?result "dcOpInfo")'
```

**Advantages**: ~200–300ms, no ADE session dependency, works even when Ocean is broken.

---

## Solution C: Add Analyses to Netlist Directly

The ADE-generated `input.scs` may only have DC. Add AC without opening ADE:

```bash
sed -i 's/^dcOpInfo info what=oppoint where=rawfile$/dcOpInfo info what=oppoint where=rawfile\nacSweep ac start=1 stop=10G dec=20 annotate=status/' input.scs
```

---

## Solution D: Regenerate via ADE GUI (for missing netlist)

1. Open testbench in Virtuoso → Launch ADE L
2. Simulation → Netlist and Run (or Netlist → Create)
3. Confirm `input.scs` is updated, then `virtuoso sim run` works again

---

## PSF Signal Naming

Instances inside a subcircuit instantiated as `I0` in the testbench:

| Context | Correct | Wrong |
|---------|---------|-------|
| Device oppoint | `getData("I0.NM0:gm" ?result "dcOpInfo")` | `getData("/NM0:gm")` |
| Node voltage (AC) | `VF("net1")` or `VF("VOUT")` | `VF("/VOUT")` |
| DC node voltage | `VDC("net1")` | — |

Find actual signal names:
```bash
strings /tmp/results/psf/acSweep.ac | grep "^I0\."
```

Or in SKILL: `openResults("/psf/path") selectResult('acSweep) VF("net1")`
— if it returns `srrWave:0x...`, the signal name is correct.

---

## Diagnostic Checklist

1. `cat runSimulation | grep -raw` → what is the canonical resultsDir?
2. `resultsDir()` → does it match the canonical path?
3. `run()` → returns path (OK) or nil (broken)?
4. PSF dir has `.dc`/`.ac` files → simulation produced data
5. PSF dir only has `simRunData`, `artistLogFile`, `variables_file` → simulation failed

---

## Key Files

```
<project>/simulation/<cell>/spectre/schematic/netlist/
├── input.scs       # Top-level: parameters, model includes, analyses
├── netlist          # Subcircuit from schematic
├── runSimulation    # Actual spectre command ADE uses (shows original PSF path)
└── netlistFooter    # Testbench instances and stimuli
```

## Notes

- `design("lib" "cell" "view")` returns nil when no ADE window is open for that cell
- `sim setup` should only be called once per ADE session
- `system("find / ...")` hangs the SKILL daemon — blocked by virtuoso-cli bridge
- After bridge hang, restart: Ctrl+C in CIW, reload bridge.il + RBStart

See also: SKILL bridge scoping rules in `~/.claude/projects/.../memory/feedback_skill_bridge_scoping.md`

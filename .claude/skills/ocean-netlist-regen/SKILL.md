---
name: ocean-netlist-regen
description: |
  Fix Ocean SKILL simulation failures after schematic edits. Use when:
  (1) run() returns nil in <1s with no spectre.out, (2) createNetlist() returns nil,
  (3) sim results are all nil after changing W/L to design variables,
  (4) netlist file was deleted or is stale, (5) sim run previously worked but
  now fails after calling `sim setup` again, (6) OSSHNL-109 error after SKILL edit,
  (7) library not found / ddGetObj returns nil / Virtuoso started from wrong directory.
  Covers: the resultsDir binding trap (MOST COMMON), OSSHNL-109 extraction timestamp
  stale error, stale netlists, sim setup disrupting sessions, direct spectre
  invocation as bypass, library-not-registered diagnosis, and PSF signal naming.
author: Claude Code
version: 2.3.0
date: 2026-04-20
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

## Root Cause 2b: design() Not Called — evalstring() First-Expression Bug

**Symptom**: `vcli sim netlist` fails even on a registered library. `si.foregnd.log` has NO new
entry (last entry is days old). `design()` from bridge returns nil.

**Root cause**: `evalstring()` in SKILL evaluates only the FIRST top-level expression in the string.
The old `setup_skill` sent three newline-separated expressions:
```skill
unless(simulator()=='spectre ...)   ; ← evalstring stops here
design("lib" "cell" "view")         ; ← silently ignored!
resultsDir()                        ; ← silently ignored!
```
Result: `design()` was never called; `createNetlist` returned nil immediately (no ADE context).

**Fix** (v0.3.1+): `setup_skill` wraps everything in `progn()`:
```skill
progn(unless(simulator()==... ...) design("lib" "cell" "view") resultsDir())
```
One top-level expression → `evalstring` evaluates all three.

**Diagnosis**: Run `vcli skill exec 'design()'` — if output is `nil`, design context is missing.
Manually run `vcli skill exec 'design("LIB" "CELL" "schematic")'` to set it.

---

## Root Cause 3: OSSHNL-109 — Extraction Timestamp Stale

**Symptom**: `createNetlist` returns nil. `si.foregnd.log` contains:
```
ERROR (OSSHNL-109): The cellview '…/schematic' has been modified since the last extraction.
Run Check and Save.
```
**artSimEnvLog** shows: `generate netlist... ...unsuccessful.`

**Root cause**: `dbSave(cv)` writes the OA file but does NOT update the extraction
timestamp. `schCheck(cv)` is what updates it. Calling `dbSave` without `schCheck`
leaves the timestamp stale; the next incremental `createNetlist` sees a mismatch.

**Fix**: Run `schCheck(cv)` before `dbSave(cv)`. Or use `vcli sim netlist` — it
auto-recovers by running `schCheck + dbSave` and retrying when `createNetlist` returns nil.

```bash
# vcli handles OSSHNL-109 automatically (v0.1.7+)
vcli sim netlist --lib FT0001A_SH --cell ota5t
```

Manual fix in SKILL:
```skill
; Get the cv (3-arg form opens "a" mode in IC23; fallback for Ocean-held cv)
cv = dbOpenCellViewByType("lib" "cell" "view")
unless(cv
  cv = car(setof(ocv dbGetOpenCellViews()
                 and(ocv~>libName=="lib" ocv~>cellName=="cell" ocv~>mode=="a"))))
chk = schCheck(cv)
when(car(chk)==0 dbSave(cv))   ; only save if no check errors
```

**Also note**: `dbReplaceProp(inst "w" ...)` sets the display parameter, NOT the
netlisted `simW`. For SMIC PDK n12/p12 width changes, use:
```skill
cdfFindParamByName(cdfGetInstCDF(inst) "simW")~>value = "1.3u"
```

---

## Root Cause 4: Stale or Missing Netlist

After editing a schematic (e.g., adding desVar variables), the netlist becomes stale.
`createNetlist()` requires an ADE L window open for that cell — without it, returns nil.

---

## Root Cause 5: Library Not Registered in Virtuoso Session

**Symptom**: `vcli sim netlist` returns an error like:
```
Library 'FT0001A_SH' is not registered in the current Virtuoso session.
Virtuoso was started from '/home/meow/git/virtuoso-cli'.
```
Or `ddGetObj("FT0001A_SH")` returns nil from the bridge.

**Root cause**: Virtuoso was started from a directory that has no `cds.lib` including
the target library. The DD database is populated at startup from `cds.lib` files found
via CDSHOME, site, and user paths — but NOT automatically from arbitrary directories.

**Diagnosis**: Check Virtuoso's working directory in `CDS.log`:
```bash
grep "Working Directory" ~/CDS.log
# Expected: meowu:/home/meow/projects/ft0001
# Wrong:    meowu:/home/meow/git/virtuoso-cli
```

**Fix**: Start Virtuoso from the project directory:
```bash
cd /home/meow/projects/ft0001 && virtuoso &
```

**Distinguish from OSSHNL-109** (both produce `err_count == -1` in vcli):
```skill
; Returns "found" if library is registered, nil if not
when(car(setof(l ddGetLibList() l~>name=="FT0001A_SH")) "found")
```
vcli v0.1.7+ runs this probe automatically and returns an actionable error message.

---

## Solution A: vcli sim netlist (handles OSSHNL-109 automatically, v0.1.7+)

```bash
# Regenerate netlist — auto-recovers from OSSHNL-109 via schCheck+dbSave retry
vcli sim netlist --lib <lib> --cell <cell>
vcli sim netlist --lib <lib> --cell <cell> --recreate   # force full recreation
```

---

## Solution B: Restore resultsDir (for Root Cause 1)

```bash
virtuoso skill exec 'resultsDir("/tmp/opt_5t_ota")'
virtuoso sim run --analysis dc --timeout 60
```

---

## Solution B: Direct Spectre Invocation (most reliable, bypasses Ocean)

**⚠️ Run from the `schematic/` directory, NOT from `netlist/`** — relative paths inside
the netlist resolve from `schematic/`. Running from `netlist/` causes CMI-2011 (cannot open input file).

```bash
SCHEMATIC_DIR="/path/to/simulation/cell/spectre/schematic"
PSF_DIR="/tmp/my_results/psf"
mkdir -p "$PSF_DIR"

cd "$SCHEMATIC_DIR" && spectre netlist/input.scs \
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

1. `grep "Working Directory" ~/CDS.log` → is Virtuoso's CWD the project directory?
2. `ddGetLibList()` from bridge → does it include the target library?
3. `cat runSimulation | grep -raw` → what is the canonical resultsDir?
4. `resultsDir()` → does it match the canonical path?
5. `run()` → returns path (OK) or nil (broken)?
6. PSF dir has `.dc`/`.ac` files → simulation produced data
7. PSF dir only has `simRunData`, `artistLogFile`, `variables_file` → simulation failed
8. `cat <netlist_dir>/si.foregnd.log` → contains OSSHNL-109? → use `vcli sim netlist` to auto-fix

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

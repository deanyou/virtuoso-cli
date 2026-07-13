---
name: porting-skill
description: |
  Port circuit simulation setups from one PDK or process technology to another.
  Use this skill whenever migrating Maestro/ADE sessions, Ocean setup scripts, or
  Spectre netlists to a different foundry or process node — for example SMIC 28nm
  to TSMC 40nm, a PDK version update within the same node, or an IC23.1 to IC25
  tool upgrade. Triggers on: "migrate to new PDK", "switch process node", "corner
  names changed after PDK update", "model file paths broken", "port simulation to
  new technology", "move design to different foundry", "Lmin changed", "new supply
  voltage", "our .sdb corners don't match new PDK", or any mention of PDK/process
  transition. Invoke this skill proactively when you detect PDK-specific paths or
  corner names in netlists that don't match the target environment.
argument-hint: [source → target, e.g. "SMIC 28nm to TSMC 40nm" or "IC23 to IC25"]
allowed-tools: Bash(vcli *) Bash(virtuoso *) Bash(spectre *) Read Write Edit Grep
---

# PDK / Process Technology Migration

Migrate Virtuoso/Spectre simulation setups to a new process node or PDK version.

**Migration target: `$ARGUMENTS`**

If the argument above is non-empty, use it as the source/target context throughout
(e.g. tailor corner name examples to the specific foundries). If it is empty, ask
the user for the old PDK and the new PDK before proceeding.

The workflow has six phases. Work through them in order — auditing first prevents
broken partial migrations.

---

## Phase 1: Audit — catalog all PDK-specific references

Before changing anything, understand the full scope.

### Find model file references in existing Ocean/netlists

Check generated netlists and Ocean setup scripts for model references.

Use the Grep tool on `*.scs`, `*.ocn`, `*.il` files under the project root for:
- `include "...\.lib"` — model file paths
- `modelFile(` — Ocean setup calls
- Section names (e.g., `section=tt`, `"ff"`, `"ss"`)

Record: **old model path**, **old section names**, **technology name in path**.

### List active design variables

```bash
vcli maestro list-vars --format json
# — or if no Maestro session —
vcli skill exec 'paramList()' --format json
```

Note all variables that encode process assumptions: `L`, `Lmin`, `vdd`, `vth_ref`,
supply names. These often need updating when nodes change.

### Check Maestro session corners

```bash
vcli maestro list-sessions --format json
vcli skill exec 'let((sess) sess=axlGetWindowSession(hiGetCurrentWindow()) maeGetSetup(?session sess))' --format json
```

Open the Maestro GUI and note every corner's model file path and section name —
these are stored in the `.sdb` and won't migrate automatically.

---

## Phase 2: Corner name mapping

Create a mapping table before changing anything:

```
Old section  →  New section
────────────────────────────
tt           →  TT  (or "typical")
ff           →  FF
ss           →  SS
fnsp         →  FS
snfp         →  SF
```

If the new PDK's section names are unknown, read its model file header:

```bash
head -80 /path/to/new_pdk/models/corner.lib | grep -E "^\.lib|^section"
```

Section names appear after `.lib` keywords in SPICE-format files, or after `section`
in Spectre-format files.

---

## Phase 3: Update model file paths

### Ocean / ADE setup scripts

Replace the old `modelFile()` call with the new PDK path and updated section names:

```bash
vcli skill exec 'modelFile(
  list("/new/pdk/path/models.lib" "TT")
  list("/new/pdk/path/models.lib" "res_TT")
)' --format json
```

**Critical**: never include an entry with an empty section name `""` for `.lib` files
— Spectre fails with SFE-675. Only `.ckt` files accept an empty section.

### Standalone .scs netlists

Edit `include` statements directly:

```spectre
// Old:
include "/smic28hpc/models/corner.lib" section=tt

// New:
include "/tsmc40lp/models/corner.lib" section=TT
```

Use Grep + Edit to find all occurrences across testbench files.

---

## Phase 4: Design variable migration

Update process-sensitive variables for the new node. Common differences:

| Variable | Example: 28nm → 40nm | Notes |
|----------|---------------------|-------|
| `L` / `Lmin` | 28e-9 → 40e-9 | Confirm with PDK spec |
| `vdd` | 0.9 V → 1.1 V | Check PDK nominal supply |
| `vth_n` / `vth_p` | node-dependent | Read from model `param` statement |
| Bias resistor `R` | may need rescaling | Keep same bias current, adjust R |

Update each variable:

```bash
vcli maestro set-var --name Lmin --value 40e-9
vcli maestro set-var --name vdd  --value 1.1
```

Or via SKILL if no active Maestro session:

```bash
vcli skill exec 'desVar("Lmin" 40e-9)' --format json
vcli skill exec 'desVar("vdd" 1.1)' --format json
```

---

## Phase 5: Maestro session re-setup

Maestro `.sdb` stores absolute model paths — these don't transfer across PDK
installations. The recommended approach:

1. Open the Maestro/ADE Assembler window in editing mode.
2. Go to **Setup → Model Files** (or equivalent in your IC version).
3. Delete all old model file entries.
4. Add new entries using the new PDK path and the mapped section names.
5. Save: `vcli maestro save --session <sessionName>`

For scripted re-entry (large corner sets), use `modelFile()` in an Ocean batch
script and call it before each run — Ocean model state doesn't persist across
sessions, so re-applying at run time is safer than trying to patch the `.sdb`.

### Verify corners are applied

Check the next generated netlist (`input.scs`) for the `include` lines — these
reflect what Spectre actually sees. If an old path appears there, Maestro is still
using a cached value: save the session and regenerate the netlist.

---

## Phase 6: Re-netlist and validate

```bash
# Force re-netlist from schematic
vcli sim netlist --lib MYLIB --cell MYCELL

# DC operating point
vcli sim run --analysis dc --param saveOppoint=t --timeout 120

# Spot-check a key node
vcli skill exec 'value(getData("/VOUT" ?result "dcOpInfo"))' --format json

# Check bias current
vcli skill exec 'value(getData("/I_bias:id" ?result "dcOpInfo"))' --format json
```

**Expected shifts after a node migration**: Ibias ±20–30% (due to different `µCox`,
`Vth`), `gm/Id` curve shifted along x-axis. Larger deviations suggest a wrong model
section or missing design variable update.

---

## Common Pitfalls

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| `SFE-675` | Empty `""` section in `modelFile()` | Remove blank-section entry |
| All corners give identical results | Section name mapping wrong — all resolve to same model | Verify section names in new PDK `.lib` header |
| Ibias off by 10× | `L` not updated — PDK default changed | Update `desVar("L" ...)` to new Lmin |
| `design()` returns nil after migration | Model path includes `oa/` symlink that broke | Use direct absolute PDK path |
| Maestro runs but results unchanged | Old `.sdb` model paths still active — didn't re-enter corners | Re-enter via Setup → Model Files and save |
| `SFE-30` on `ac=1` in vsource | Syntax unchanged but now in native Spectre mode | Change to `mag=1` — see `/spectre-netlist-gotchas` |

For netlisting nil / OSSHNL errors after migration, see `/ocean-netlist-regen`.
For SFE-30, oprobe noise, PSF parse errors, see `/spectre-netlist-gotchas`.
For re-running simulations after migration, see `/sim-setup` and `/sim-run`.

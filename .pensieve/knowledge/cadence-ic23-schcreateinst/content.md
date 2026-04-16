# Cadence IC23 schCreateInst Signature

## Source
Session investigation 2026-04-16: accidental M1 deletion led to re-creation attempt;
discovered IC23 signature differs from IC6/IC25 documentation.

## Summary
IC23 `schCreateInst` takes a DB cellview object as `masterCV`, not lib/cell/view strings,
and does NOT accept `?params` keyword arguments for initial property values.

## Content

### Correct IC23 Signature

```skill
schCreateInst(cv masterCV instName xy orient)
```

Where:
- `cv`        — destination schematic cellview (open in "a" mode)
- `masterCV`  — **DB cellview object** (from `dbOpenCellViewByType`), NOT strings
- `instName`  — string, e.g. `"M1"`
- `xy`        — coordinate list, e.g. `list(-3.0 3.0)`
- `orient`    — string, e.g. `"R0"`, `"R90"`, `"MY"` etc.

### Full Working Example

```skill
cv = dbOpenCellViewByType("FT0001A_SH" "ota5t" "schematic" "schematic" "a")
masterCV = dbOpenCellViewByType("smic13mmrf_1233" "n12" "symbol" "schematicSymbol" "r")
inst = schCreateInst(cv masterCV "M1" list(-3.0 3.0) "R0")

; Then set properties separately:
cdf = cdfGetInstCDF(inst)
cdfFindParamByName(cdf "simW")~>value = "1.1u"
cdfFindParamByName(cdf "l")~>value    = "500n"
dbSave(cv)
```

### Common Mistakes

| Wrong | Error / Result |
|-------|---------------|
| `schCreateInst(cv "smic13mmrf_1233" "n12" "symbol" "M1" list(0 0) "R0")` | "too many arguments" — lib/cell/view as 3 separate string args not accepted |
| `schCreateInst(cv masterCV "M1" list(0 0) "R0" ?l "500n" ?w "1.1u")` | "too many arguments" — `?params` keyword args not supported in IC23 |
| Using `dbOpenCellViewByType` with wrong view name | Returns nil; `schCreateInst` then fails with nil masterCV |

### After Instance Deletion: Reconnect Gate Labels

Deleting an instance with `dbDeleteObject(inst)` also deletes co-located net labels.
After re-creating the instance, restore connectivity:

```skill
; Example: restore "vin" gate label for M1
cv  = dbOpenCellViewByType("FT0001A_SH" "ota5t" "schematic" "schematic" "a")
m1  = car(setof(i cv~>instances i~>name=="M1"))
net = car(setof(n cv~>nets n~>name=="vin"))
lpp = list("schematic" "wirelabel")   ; or copy from existing label
lbl = dbCreateLabel(cv lpp list(-3.0 3.0) "vin" "centerCenter" "R0" "stick" 0.0625)
lbl~>net = net
schCheck(cv)   ; → (0 0) before saving
dbSave(cv)
```

### Verify Connectivity

```skill
m1   = car(setof(i cv~>instances i~>name=="M1"))
trms = m1~>terminals
foreach(t trms sprintf(nil "%s→%s" t~>name t~>net~>name))
; Should show: D→nd1  G→vin  S→vtail  B→0 (for NMOS in this OTA)
```

### schCheck Before Save

Always run `schCheck(cv)` and verify result is `(0 0)` before `dbSave(cv)`.
Non-zero counts mean floating nets or connectivity errors that will corrupt the netlist.

## When to Use

- When programmatically creating schematic instances via SKILL in IC23
- When re-creating instances after accidental deletion
- When porting SKILL scripts written for IC6 or IC25 documentation

## Context Links
- Related: [[smic-pdk-transistor-w-skill]] — after creating, set simW for correct netlisting

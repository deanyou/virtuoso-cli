# SMIC PDK Transistor Width via SKILL: CDF Chain & Working Path

## Source
Session investigation 2026-04-16: FT0001A_SH/ota5t schematic W/L update via vcli,
confirmed by Cadence-exported netlist showing correct w=1.1u/5u/1.2u.

## Summary
SMIC 0.13um MMRF n12/p12 symbols use a 3-level CDF indirection for transistor width;
setting `simW` as a DB property is the only persistent path that the netlister reads.

## Content

### The CDF Chain

For SMIC 0.13um MMRF (`n12`, `p12`, `p12d`, `n12d` symbols from `smic13mmrf_1233`):

```
CDF display param  w     → not directly netlisted
propMapping entry: w → simW
simW's CDF value:  iPar("fw")   ← evaluated by smic13mm_mosCB callback
fw:                editable=t, default="280n"
```

The netlister reads the OA C++ DB property `simW` (not the SKILL-accessible CDF object).

### What DOESN'T Work

| Approach | Why it fails |
|----------|-------------|
| `cdfFindParamByName(cdfGetInstCDF(inst) "fw")~>value="1.1u"` | `cdfGetInstCDF` returns in-memory merged CDF only; changes not persisted to OA DB; netlister reads OA C++ layer, not SKILL CDF objects |
| `cdfFindParamByName(cdf "w")~>value="1.1u"` | `w` has no direct DB property; it's a display alias resolved through callback |
| Setting `instParamValues` OA named param | In IC23 schematic (non-PCELL) instances, netlister ignores `instParamValues` |
| `inst~>prop` where propName="w" | `w` is NOT in the DB property table; the DB property for netlisting is `simW` |

### What WORKS

```skill
cv = dbOpenCellViewByType("FT0001A_SH" "ota5t" "schematic" "schematic" "a")
m1 = car(setof(i cv~>instances i~>name=="M1"))
cdf = cdfGetInstCDF(m1)
cdfFindParamByName(cdf "simW")~>value = "1.1u"   ; ← key: set simW not w or fw
cdfFindParamByName(cdf "l")~>value   = "500n"
dbSave(cv)
```

Then export netlist:
```skill
simulator('spectre)
design("FT0001A_SH" "ota5t" "schematic")
createNetlist(?recreateAll t ?display nil)
```

The exported `.scs` will contain `w=1.1u l=500n` for M1.

### Parameter Mapping Reference (n12/p12)

| CDF display param | DB property (netlisted) | Notes |
|-------------------|------------------------|-------|
| `w`               | `simW`                 | via propMapping + iPar("fw") chain |
| `l`               | `l`                    | direct, no propMapping |
| `m`               | `m`                    | multiplier, direct |
| `sa`, `sb`        | `SAeff`, `SBeff`       | via propMapping |

### Diagnostic SKILL (read `propMapping`)

```skill
cv = dbOpenCellViewByType("FT0001A_SH" "ota5t" "schematic" "schematic" "r")
m1 = car(setof(i cv~>instances i~>name=="M1"))
cdf = cdfGetInstCDF(m1)
sprintf(nil "%L" cdf~>propMapping)
; → (nil m simM w simW sa SAeff sb SBeff)
```

### Symptoms of Wrong Path

- Cadence-exported netlist shows `w=280n` for all transistors regardless of schematic form values
- `cdfGetInstCDF(inst)~>parms` is empty (`nil`) — instance-level CDF parms overrides don't exist
- `cdfFindParamByName(cdf "fw")~>editable` returns `"t"` but changes don't survive `dbSave`

## When to Use

- When updating transistor sizing in SMIC PDK symbols via SKILL
- When exported netlist shows wrong/default W values (280n = PDK minimum default for `fw`)
- When debugging why `vcli schematic set-param` calls don't affect netlisting

## Context Links
- Related: [[cadence-ic23-schcreateinst]] — instance creation (needed if re-creating instances)
- Related: [[skill-sprintf-float-coords]] — coordinate formatting in SKILL

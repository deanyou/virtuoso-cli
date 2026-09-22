---
name: digital-import
description: |
  Import P&R (Genus + Innovus) products into Virtuoso: GDS layout, Verilog
  schematic/symbol, power labels, and label restyling. Four-step pipeline driven
  entirely from vcli skill exec — no Python bridge or GUI required.

  Use when: (1) user wants to pull a routed GDS or post-P&R netlist into Virtuoso,
  (2) layout labels look giant/unreadable after import, (3) user mentions strmin /
  ihdl / digital import / P&R-to-schematic flow.
allowed-tools:
  - Bash(vcli *)
argument-hint: "[step, e.g. 'import GDS' or 'restyle labels for DIG_OUTPUT/LFSR_32BIT']"
---

# Digital Import — 4-Step Pipeline

| # | Step | Tool | Output |
|---|------|------|--------|
| 1 | **import_gds** | `strmin` | `layout` view |
| 2 | **import_verilog** | `ihdl` | `schematic` + `symbol` |
| 3 | **add_power_labels** | SKILL `dbCreateLabel` | VDD/VSS labels on M1.pin |
| 4 | **restyle_labels** | SKILL `dbSetq` via `~>attr` | Shrink `text/drawing` to 0.05 µm; bump `<layer>/pin` to 0.2 µm + roman |

Steps 3 and 4 are pure SKILL — run directly from `vcli skill exec`.
Steps 1–2 invoke Cadence batch tools via SKILL `system()`.

## Prerequisite — cds.lib DEFINE lines

`strmin` and `ihdl` create cellview directories on disk but **do NOT edit cds.lib**.
If a library has no `DEFINE` line, Virtuoso won't see the result. Add before running:

```
DEFINE DIG_OUTPUT      /home/you/work/DIG_OUTPUT
DEFINE tsmcN28         /cad/process/.../tsmcN28
DEFINE tcbn28hpcplus   /cad/process/.../bwp12t30p140
```

---

## Step 1 — Import GDS (strmin)

```bash
# Standard: provide a ref-libs directory
vcli skill exec 'system("strmin -library DIG_OUTPUT \
  -strmFile /path/to/foo.route_tapeout.gds \
  -techLib tsmcN28 \
  -refLibList /path/to/ref_libs_dir \
  -logFile /tmp/strmin.log")'

# Shortcut: if cds.lib already DEFINEs every referenced lib
# Pass the magic literal XST_CDS_LIB — strmin consults the CWD cds.lib instead
vcli skill exec 'system("strmin -library DIG_OUTPUT \
  -strmFile /path/to/foo.route_tapeout.gds \
  -techLib tsmcN28 \
  -refLibList XST_CDS_LIB \
  -logFile /tmp/strmin.log")'

# Verify
vcli skill exec 'dbOpenCellViewByType("DIG_OUTPUT" "LFSR_32BIT" "layout" nil "r")~>bBox'
```

`XST_CDS_LIB` is a strmin magic literal — mutually exclusive with a real ref file.
Use it when the project's cds.lib is already curated (every dependency has a DEFINE).

### ⚠️ Verify every expected cell actually landed

> ⚠️ **Source**: [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite),
> commit `1ae2156` (MIT, 2026-06-05).

`strmin`'s exit code and `Translation completed` line are **necessary but not
sufficient**. GDSII files can have undef-rec / unresolved-cell references that
strmin accepts as a no-op: it still prints "Translation completed", exits 0,
and leaves the cell with a default empty layout. The `dbOpenCellView...~>bBox`
check above only confirms the cell exists in the DB, not that it's a real
layout vs a placeholder. The reliable check is `dbCompareCell` (or
`dbOpenCellView...~>shapes~>??`):

```bash
# Reliable strmin verification — every expected cell is a real layout, not a
# stub. Returned as a single space-separated status string.
vcli skill exec 'let((lib cells results n ok missing)
  lib = "DIG_OUTPUT"
  cells = list("LFSR_32BIT" "FIFO_8x32" "CLK_DIV_4")
  results = nil
  n = 0
  foreach(cell cells
    n = n + 1
    cv = dbOpenCellViewByType(lib cell "layout" nil "r")
    when(cv
      n_shapes = length(cv~>shapes)
      n_insts  = length(cv~>instances)
      ok = (n_shapes + n_insts) > 0
      results = cons(sprintf(nil "%s=%d shapes+%d insts%s"
                                    cell n_shapes n_insts
                                    (ok ? "" : " [EMPTY]"))
                    results)
      dbClose(cv)
      unless(ok missing = cons(cell missing))
    )
    when(!cv missing = cons(cell missing))
  )
  sprintf(nil "%s\nmissing: %s" reverse(results) missing))'
```

**When to use this**: any time you import a multi-cell GDS block, the
import "succeeded" but the *downstream* tool reports an empty cell or
broken connectivity. This check costs 3-5 SKILL calls — run it once after
every block import.

Why the existing `dbOpenCellViewByType...~>bBox` check is not enough:
strmin's placeholder cell is `(0,0,0,0)` bBox, so a non-nil bBox proves
the cellview is opened, not that it has contents. The `shapes + instances > 0`
check catches the silent-stub case.

---

## Step 2 — Import Verilog (ihdl)

```bash
vcli skill exec 'let((f)
  f = outfile("/tmp/ihdl_param" "w")
  fprintf(f "reference_libraries := tcbn28hpcplusbwp12t30p140\n")
  fprintf(f "design_library := DIG_OUTPUT\n")
  fprintf(f "input_file := /path/to/foo_import.v\n")
  fprintf(f "structural_views := 5\n")   ; schematic + functional (IC618 encoding)
  close(f)
  system("ihdl -ihdlFile /tmp/ihdl_param -log /tmp/ihdl.log"))'
```

If ihdl fails, check `<virtuoso_workdir>/verilogIn.batch.log` for diagnostics.

### Verify every expected module created a view

ihdl exits 0 on a partial import: a Verilog with 5 modules where 4 have
unresolved references (e.g. bus ports in `module foo(a[3:0])` mapped to a
missing `tech library`) leaves those modules unreferenced and creates only
the "good" ones. The `design_library` has all the imported cells but the
top-level may be missing. Use the same `ddGetObj` + `views~>name` check
from R3-7 / commit `1ae2156`:

```bash
# After ihdl: confirm each top module has BOTH a schematic and a symbol view
# (ihdl's typical contract). Misses are not always fatal, but you want to
# know about them.
vcli skill exec 'let((lib modules expected missing)
  lib = "DIG_OUTPUT"
  modules = list("LFSR_32BIT" "FIFO_8x32" "CLK_DIV_4")
  expected = list("schematic" "symbol")
  missing = nil
  foreach(mod modules
    foreach(view expected
      r = ddGetObj(lib mod view)
      unless(r
        missing = cons(sprintf(nil "%s/%s" mod view) missing)
      )
    )
  )
  sprintf(nil "missing views: %s" (missing ? reverse(missing) : "none")))'
```

**When to use this**: any time you import a Verilog block and the schematic
"looks fine" but the symbol generation in Step 3 says "no terminals found".
Most often the top module is missing the `schematic` view because ihdl
silently failed on a parameter declaration.

---

## Step 3 — Add Power Labels

Walk instances to find one with VDD/VSS terminals, read pin geometry, transform
through instance xform, drop labels at the layout midline.

```bash
vcli skill exec 'let((cv vddY vssY midX)
  cv = dbOpenCellViewByType("DIG_OUTPUT" "LFSR_32BIT" "layout" nil "a")
  ; find VDD/VSS Y coords from instance pin geometry + xform (simplified)
  ; then place labels
  dbCreateLabel(cv (list "M1" "pin") (list midX vddY) "VDD!" "centerLeft" "R0" "roman" 1.0)
  dbCreateLabel(cv (list "M1" "pin") (list midX vssY) "VSS!" "centerLeft" "R0" "roman" 1.0)
  dbSave(cv)
  dbClose(cv))'
```

TSMC defaults: layer `M1`, purpose `pin`, font `roman`, height `1.0`.
Sky130: use `VPWR`/`VGND`, height `0.4` (5T row ≈ 0.5 µm).

---

## Step 4 — Restyle Labels

Innovus stamps hundreds of `text/drawing` labels at 1 µm — taller than the cells
themselves on a tiny digital block. This single SKILL traversal fixes both classes:

```bash
# Quick version (no floorplan, bbox heuristic for pin orientation)
vcli skill exec 'let((cv bb xmin ymin xmax ymax thr n_text n_pin n_or pin_shapes)
  cv = dbOpenCellViewByType("DIG_OUTPUT" "LFSR_32BIT" "layout" nil "a")
  thr = 4.0   ; distance threshold (µm) to edge
  n_text = 0  n_pin = 0  n_or = 0
  pin_shapes = nil
  ; Pass A: set heights/font — read bBox AFTER this pass (see gotcha below)
  foreach(s cv~>shapes
    when(s~>objType == "label"
      cond(
        (s~>layerName == "text" && s~>purpose == "drawing"
           s~>height = 0.05
           n_text = n_text + 1)
        (s~>purpose == "pin"
           s~>height = 0.2
           s~>font = "roman"
           s~>justify = "centerLeft"
           n_pin = n_pin + 1
           pin_shapes = cons(s pin_shapes))
      )
    )
  )
  ; Pass B: orient pin labels by bbox distance (read bBox AFTER pass A)
  bb = cv~>bBox
  xmin = caar(bb)  ymin = cadar(bb)
  xmax = caadr(bb) ymax = cadadr(bb)
  foreach(s pin_shapes
    let((x y dl dr db dt mn)
      x = car(s~>xy)  y = cadr(s~>xy)
      dl = x - xmin  dr = xmax - x
      db = y - ymin  dt = ymax - y
      mn = min(min(dl dr) min(db dt))
      when(mn < thr
        cond(
          (mn == db  s~>orient = "R270"  n_or = n_or + 1)
          (mn == dt  s~>orient = "R90"   n_or = n_or + 1)
          (mn == dl  s~>orient = "R180"  n_or = n_or + 1)
          (mn == dr  s~>orient = "R0"    n_or = n_or + 1)
        )
      )
    )
  )
  dbSave(cv)
  dbClose(cv)
  sprintf(nil "text/drawing: %d -> 0.05 | pin: %d -> 0.2 roman | oriented: %d"
              n_text n_pin n_or))'
```

Expected output: `text/drawing: 505 -> 0.05 | pin: 30 -> 0.2 roman | oriented: 28`

---

## Critical SKILL Gotchas

### `~>attr = val` not `dbSet` for label properties

```skill
; WRONG — silently no-ops on label height in IC618/IC23
dbSet(s 'height 0.05)

; CORRECT — compiles to dbSetq, actually works
s~>height = 0.05
s~>font   = "roman"
s~>orient = "R90"
```

This applies to `height`, `font`, `justify`, `orient` on label shapes.
`dbSet` returns `t` with no error but the value doesn't change.

### Two-pass approach for pin orientation (bbox timing bug)

If you read `cv~>bBox` BEFORE resizing `text/drawing` labels, the bbox is inflated
by 1 µm labels — corner pins appear farther from the edge than they are and get
missed by the `mn < thr` check. Always:

1. **Pass A**: set heights on all labels (bBox still wrong)
2. **Pass B**: read `cv~>bBox`, then classify pin edges — bBox now reflects final sizes

Alternatively, parse `editPin -side X` from the Innovus floorplan Tcl for source-of-truth
orientation (bypasses the heuristic entirely):
- `Top` → `R90`, `Bottom` → `R270`, `Left` → `R180`, `Right` → `R0`
- `-edge N` (integer form) is Innovus-version-dependent — skip it, fall back to bbox

### strmin does not update cds.lib

Always add `DEFINE` lines to cds.lib before running strmin. Missing a lib → Virtuoso
silently ignores the imported cells (no error from strmin itself).

### Bus bracket rewriting

strmin with `-replaceBusBitChar` rewrites `signal[3]` → `signal<3>` in labels.
When matching pin names from Innovus floorplan Tcl against imported labels, replace
`[` → `<` and `]` → `>`.

### The 10-min trap — strmin/ihdl die silently

> ⚠️ **Source**: [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite),
> AGENTS.md (MIT, 2026-05). Observed 2026-05-14 on
> `examples/01_virtuoso/digital_import/import_gds.py`.

`strmout` / `strmin` / `ihdl` `system()` return codes are **unreliable** — the
parent process returns 0 even when the child tool died with a fatal error in
under 2 seconds. A wrapper that polls for the expected output artifact will
happily sleep for its full 10-minute timeout waiting for a file that will
*never* appear because the tool already aborted with a sentinel like
`XSTRM-273: Translation failed` or `ihdl: OPEN_FAILED`.

**Symptom (diagnostic)**: strmin/ihdl call returns "successfully" within
~2 s of wall time, but the expected output file is missing AND
`/tmp/strmin.log` (or the tool log) contains one of:

```
XSTRM-273: Translation failed
XSTRM-210: cannot open reference library
ihdl: OPEN_FAILED
ihdl: ERROR <message>
```

**Dual-defense template** — apply whenever you wrap a `system()`-launched
tool in a poll-for-output loop:

1. **Before invoking the tool**: stage any local file args to the tool's cwd
   via `client.upload_file()` so file-not-found can't happen. (For our
   `strmin` example, the GDS path is already remote, but the `-logFile`
   path needs to be a writable remote path.)

2. **In the poll loop**, on every iteration `tail -n 200 <tool.log>` and
   fast-exit with the offending line if you see a terminal-failure marker:

   ```bash
   # pseudo-loop
   for i in $(seq 1 600); do  # 600 × 1s = 10 min max
       # Did the artifact appear?
       if ssh remote "test -s /path/to/output"; then
           break
       fi
       # Did the tool log a fatal error in the meantime?
       if ssh remote "tail -n 200 /tmp/strmin.log | grep -qE '(Translation failed|OPEN_FAILED|ERROR|FATAL)'"; then
           err=$(ssh remote "grep -m1 -E '(Translation failed|OPEN_FAILED|ERROR|FATAL)' /tmp/strmin.log")
           echo "strmin aborted: $err" >&2
           exit 1
       fi
       sleep 1
   done
   ```

   Without (2), a 2-second tool death manifests as a 10-minute hang —
   the most expensive possible failure mode in a CI loop.

3. **Bonus**: set `-logFile` to a known path **and** parse the final
   `tail -n 1` of that log; if it doesn't contain a known success sentinel
   (e.g. `Translation completed` / `ihdl: done`) treat that as failure too.

---

## PDK Portability

| Flag | Step | TSMC N28 default | Sky130 override |
|------|------|-----------------|-----------------|
| tech library | 1 | `tsmcN28` | `sky130A` |
| ref library | 2 | `tcbn28hpcplusbwp12t30p140` | `sky130_fd_sc_hd` |
| power/ground pin names | 3 | `VDD` / `VSS` | `VPWR` / `VGND` |
| label height | 3 | `1.0` µm | `0.3–0.4` µm (5T row ≈ 0.5 µm) |

`ihdl structural_views := 5` is the IC618 SP201 encoding for schematic + functional.
On a different IC release, check the *Verilog In for Virtuoso Design Environment User Guide*
for the correct integer.

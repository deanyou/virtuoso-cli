---
name: cell-explore
description: Explore Virtuoso cellviews - list libraries, cells, instances, nets, layers, and hierarchy. Use when browsing a design, understanding circuit topology, or inspecting layout/schematic contents.
allowed-tools: Bash(*/virtuoso *)
---

# Explore Virtuoso Cellviews

Query design hierarchy, instances, nets, and properties via SKILL.

## Library browsing

```bash
# List all libraries
virtuoso skill exec 'foreach(mapcar lib ddGetLibList() lib~>name)' --format json

# List cells in a library
virtuoso skill exec 'let((lib) lib=ddGetObj("LIB") foreach(mapcar c lib~>cells c~>name))' --format json

# List views for a cell (use v~>name, NOT v~>viewName)
virtuoso skill exec 'let((cell) cell=ddGetObj("LIB" "CELL") foreach(mapcar v cell~>views v~>name))' --format json
```

## Cellview inspection

```bash
# Open and inspect (read-only)
virtuoso skill exec 'let((cv) cv=dbOpenCellViewByType("LIB" "CELL" "VIEW" nil "r") when(cv sprintf(nil "inst=%d nets=%d shapes=%d" length(cv~>instances) length(cv~>nets) length(cv~>shapes))))' --format json

# Instance details (first N)
virtuoso skill exec 'let((cv result n) cv=dbOpenCellViewByType("LIB" "CELL" "schematic" nil "r") result=nil n=0 foreach(inst cv~>instances when(n<20 result=cons(sprintf(nil "%s: %s/%s" inst~>name inst~>cellName inst~>viewName) result) n=n+1)) result)' --format json

# Net connectivity
virtuoso skill exec 'let((cv result n) cv=dbOpenCellViewByType("LIB" "CELL" "schematic" nil "r") result=nil n=0 foreach(net cv~>nets when(n<20 result=cons(sprintf(nil "%s [%d terms]" net~>name length(net~>instTerms)) result) n=n+1)) result)' --format json

# Layer/purpose pairs in layout
virtuoso skill exec 'let((cv lpps) cv=geGetEditCellView() lpps=nil foreach(shape cv~>shapes let((lpp) lpp=sprintf(nil "%s:%s" car(shape~>lpp) cadr(shape~>lpp)) unless(member(lpp lpps) lpps=cons(lpp lpps)))) lpps)' --format json

# Shape types
virtuoso skill exec 'let((cv types) cv=geGetEditCellView() types=nil foreach(shape cv~>shapes let((tp) tp=shape~>objType unless(member(tp types) types=cons(tp types)))) types)' --format json
```

## Cell management

```bash
# Open cellview
virtuoso cell open --lib LIB --cell CELL --view layout --format json

# Current cellview info
virtuoso cell info --format json

# Save
virtuoso cell save --format json
```

## Key gotcha

- **Always open cellviews in `"r"` (read-only) mode** — using `"w"` creates an empty `.oa` file on disk, destroying the schematic. The fifth argument to `dbOpenCellViewByType` must be `"r"` for any exploration/inspection call.
- **View property is `v~>name`**, not `v~>viewName` — the latter returns nil in IC23.1
- **`nthCdr` and `subList` don't exist** — use `foreach` with counter for pagination
- **`buildString` may not work** with nil values — guard with `when(views ...)`

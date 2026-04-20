---
id: cadence-ic23-ade-window-title
type: knowledge
title: IC23.1 ADE window title format
status: active
created: 2026-04-20
updated: 2026-04-20
tags: [maestro, ade, ic23, window-title]
---

# IC23.1 ADE window title format

## Source
`src/commands/maestro.rs::parse_ade_title` — bug root-caused 2026-04-20

## Summary
ADE window titles on IC23.1 start with `"Virtuoso® "` prefix, not directly with `"ADE "`.

## Content

### Actual format (IC23.1-64b.500)
```
Virtuoso® ADE Explorer Editing: FT0001A_SH 5T_OTA_D_TO_S_sim maestro* Version: -UNMANAGED
Virtuoso® ADE Assembler Editing: LIB CELL VIEW
```

The `®` is Unicode character U+00AE (encoded as `\256` in SKILL strings).

### Grammar
```
<title> ::= [<prefix> " "] "ADE " <app> " " <mode> ": " <lib> " " <cell> " " <view>["*"] [" Version: " <ver>]
<prefix> ::= "Virtuoso®" | (anything)
<app>    ::= "Assembler" | "Explorer"
<mode>   ::= "Editing" | "Reading"
```

### Parsing rule
Use `title.find("ADE ")` then slice from that position.
Do **not** use `strip_prefix("ADE ")` — it fails when any prefix is present.

```rust
// CORRECT
let ade_pos = title.find("ADE ")?;
let rest = &title[ade_pos + 4..];

// WRONG — only works if title literally starts with "ADE "
let rest = title.strip_prefix("ADE ")?;
```

### Modes
| Window text | `editable` | meaning |
|-------------|-----------|---------|
| `Editing: ` | true | Can modify & run simulation |
| `Reading: ` | false | Read-only; mae* mutation calls will fail |

Unsaved changes: view name ends with `*` (e.g. `maestro*`).

## When to Use
Any code that parses or interprets `hiGetWindowName()` output for ADE windows.

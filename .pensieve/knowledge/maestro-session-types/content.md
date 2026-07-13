---
id: maestro-session-types
type: knowledge
title: Maestro session name vs Ocean session name
status: active
created: 2026-04-20
updated: 2026-04-20
tags: [maestro, skill, ade, session]
---

# Maestro session name vs Ocean session name

## Source
`src/commands/maestro.rs`, `src/client/maestro_ops.rs` — session confusion root-caused 2026-04-20

## Summary
Two completely different "session" concepts exist in Virtuoso; mixing them causes silent wrong behavior.

## Content

| Concept | Example value | How to obtain | Used by |
|---------|--------------|---------------|---------|
| **Maestro UI session** | `"fnxSession0"` | `maeGetSessions()` | `maeRunSimulation`, `maeSaveSetup`, `maeGetAllExplorerHistoryNames`, `maeSetAnalysis` (IC23), etc. |
| **Ocean/Spectre session** | `"spectre1"` | `asiGetCurrentSession()~>name` | `asiGetDesignVarList`, `asiGetAnalogRunDir` |

### Rule
- `mae*` functions that take a session name expect the **Maestro UI session** (e.g. `"fnxSession0"`).
- `asi*` functions that take a session expect the **Ocean session object** or its `~>name`.
- `maeGetAllExplorerHistoryNames("fnxSession0")` — positional Maestro session name.
- `asiGetAnalogRunDir(sess)` where `sess = asiGetSession("fnxSession0")` — passes the Ocean session *object* (not the name string).

### Anti-pattern that causes failure
```skill
; WRONG: passes Ocean session name where Maestro session name expected
let((sess) sess = asiGetCurrentSession() maeGetAllExplorerHistoryNames(sess~>name))
; sess~>name → "spectre1"  ← wrong, not a Maestro session name
```

```rust
// CORRECT in vcli: history-list command requires explicit --session fnxSession0
// so user provides the Maestro session name, not derived from asiGetCurrentSession
```

## When to Use
Before writing any new `mae*` or `asi*` SKILL expression that takes a session argument.

## Context Links
- Related: [[cadence-ic23-dbopencellviewbytype]]

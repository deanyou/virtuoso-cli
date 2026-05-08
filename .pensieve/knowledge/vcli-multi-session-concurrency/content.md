---
id: vcli-multi-session-concurrency
type: knowledge
title: vcli Multi-Session Concurrency Patterns
status: active
created: 2026-05-07
updated: 2026-05-07
tags: [vcli, session, concurrency, broadcast, subagent]
---

# vcli Multi-Session Concurrency Patterns

## Source
Session 2026-05-07: implemented `vcli skill broadcast`, validated with two live sessions.

## Summary
Two mechanisms for targeting multiple Virtuoso sessions concurrently; the right choice depends on whether the operation is uniform or heterogeneous.

## Content

### Session targeting mechanism

`VirtuosoClient::from_env()` resolves the session port via:
1. `VB_SESSION=<id>` env var → reads `~/.cache/virtuoso_bridge/sessions/<id>.json` → port
2. Auto-select if exactly one alive session
3. Error if multiple sessions and no `VB_SESSION`

Prefixing any vcli command with `VB_SESSION=<id>` routes it to that specific session's TCP port. Shell process isolation means concurrent commands with different `VB_SESSION` values don't interfere.

### Mechanism 1: `vcli skill broadcast`

```bash
vcli skill broadcast 'getVersion(t)'
```

- Implemented via `std::thread::scope` — one thread per alive session
- Each thread opens its own `TcpStream` to the session's port
- All threads fire simultaneously; result collected after all join
- Returns `{"status": "success"|"partial"|"error", "sessions": N, "ok": N, "results": [...]}`
- Exit non-zero only when every session fails
- **Best for**: same SKILL expression across all sessions (health checks, save-all, version queries)

### Mechanism 2: Main agent + parallel subagents

```
Main agent: vcli session list → plan tasks per session
Single message, N Agent tool calls:
  Agent A: VB_SESSION=<id1> vcli <command A>  (multi-step workflow)
  Agent B: VB_SESSION=<id2> vcli <command B>  (multi-step workflow)
Main: collect JSON results, synthesize
```

- Total wait ≈ max(t_A, t_B), not sum
- Each subagent has full reasoning capability for multi-step per-session work
- **Best for**: heterogeneous tasks (session A runs DC sim, session B checks schematic)

### Verified behavior (2026-05-07)

Sessions: `meowu-meow-39717` (port 39717), `meowu-meow-44791` (port 44791)

```json
vcli skill broadcast 'let((cv) cv=geGetEditCellView() ...'
→ session 39717: "nil"  (no open cellview)
→ session 44791: "(\"FT0001A_SH\" \"Bandgap\" \"schematic\")"
```

Both respond independently and concurrently.

## When to Use

- Read this before implementing any operation that touches multiple Virtuoso sessions
- When a skill or command needs to fan out to all sessions

## Context Links
- Based on: [[vcli-session-history-layout]]
- Related: [[ramic-bridge-callback-file-ipc]]

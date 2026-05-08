# Broadcast vs Subagent for Multi-Session Operations

## One-line Conclusion
> Use `vcli skill broadcast` for uniform SKILL calls; use main+subagent for heterogeneous multi-step workflows per session.

## Context Links
- Based on: [[vcli-multi-session-concurrency]]
- Related: [[ramic-bridge-callback-file-ipc]]

## Context

vcli supports two Virtuoso session multiplexing patterns, both validated on 2026-05-07 with two live sessions.

## Problem

When an operation needs to touch multiple Virtuoso sessions, it's unclear whether to use the built-in `broadcast` command or orchestrate via Claude Code subagents.

## Alternatives Considered

- **broadcast only**: simple, but limited to one SKILL expression; can't do multi-step reasoning per session
- **subagents only**: flexible, but LLM context overhead per session; wasteful for a simple `getVersion(t)` query
- **tokio async**: overkill — SKILL bridge is single-threaded per session; `std::thread::scope` is sufficient and avoids the async runtime dependency

## Decision

| Criterion | Use `broadcast` | Use main+subagent |
|-----------|----------------|-------------------|
| Same SKILL on all sessions | ✓ | |
| Different tasks per session | | ✓ |
| Multi-step workflow per session | | ✓ |
| Minimal overhead | ✓ | |
| Per-session reasoning | | ✓ |

## Consequence

- `broadcast` is the fast path for health checks, save-all, global queries
- Subagents are the right tool when sessions need different inputs or multi-step execution
- Both patterns can coexist: main agent uses `broadcast` to survey state, then spawns targeted subagents only for sessions that need work

## Exploration Reduction
- What to ask less next time: "should I loop over sessions or use broadcast?" — check the table above
- What to look up less next time: the `VB_SESSION=<id>` prefix mechanism; it's shell env var, fully isolated per process
- Invalidation condition: if vcli gains a native `--session` flag for all commands, the env var prefix approach may be superseded

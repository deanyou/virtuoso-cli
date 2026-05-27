---
id: 2026-05-07-async-deferred
type: decision
title: Async Runtime Deferred — Use std::thread::scope for Concurrency
status: active
created: 2026-05-07
updated: 2026-05-27
tags: ["decision"]
---

# Async Runtime Deferred — Use std::thread::scope for Concurrency

## One-line Conclusion
> Do not introduce tokio; use `std::thread::scope` for multi-session concurrency until a genuine multi-instance use case forces the issue.

## Context Links
- Based on: [[vcli-multi-session-concurrency]]
- Based on: [[ramic-bridge-callback-file-ipc]]

## Context

During si-view/via analysis (2026-05-07), the via project's tokio+oneshot router was identified as a potential pattern. The question was whether vcli should adopt async.

## Problem

The SKILL bridge is inherently serial per session (Virtuoso SKILL is single-threaded). Async would not increase per-session throughput. The only genuine parallel I/O opportunities are: (1) multiple independent session connections, (2) TUI background polling.

## Alternatives Considered

- **tokio + oneshot router**: via's pattern. Clients non-blocking on their side, but Virtuoso still processes one request at a time. Adds ~500KB dependency and async complexity throughout the call stack.
- **`std::thread::scope`**: scoped threads, no Arc needed, borrows from enclosing scope. Sufficient for broadcast (N sessions × 1 thread). Composes naturally with existing synchronous code.
- **`rayon`**: parallel iterators — overkill; no data parallelism here, just I/O fan-out.

## Decision

`std::thread::scope` in `broadcast()`. Revisit only if:
- vcli needs to manage multiple Virtuoso daemons with a persistent connection pool
- TUI refresh latency becomes a real complaint (currently threads work fine)

## Consequence

No tokio dependency added. `broadcast` implementation is ~40 lines of synchronous Rust using scoped threads. If async is eventually needed, the boundary is clear: `execute_skill` is the natural `async fn` conversion point.

## Exploration Reduction
- What to ask less next time: "should I use async here?" — only if persistent connection pool or reactive streams are needed
- What to look up less next time: `std::thread::scope` borrows from enclosing scope without Arc — this is the idiomatic Rust pattern for short-lived fan-out
- Invalidation condition: if vcli adds a long-lived daemon that maintains N persistent SKILL connections simultaneously

---
name: vcli-architecture
description: VCLI architecture intelligence — invariants, boundaries, and change gates for modifying virtuoso-cli codebase. Use before any Rust/Python/SKILL modification that touches runtime, router, executor, verifier, evidence, or recovery.
---

# VCLI Architecture Intelligence

## Purpose

This skill defines the architectural invariants and boundaries that must be preserved when modifying `virtuoso-cli`. It works alongside the code knowledge graph (`.ua/knowledge-graph.json` when Understand Anything is available) to ensure changes don't violate established design principles.

## Two Governance Principles

```
Learn Above, Freeze Below
  Skill/Knowledge layer learns; Runtime/Executor stays deterministic.

No Evidence, No Architecture Change
  Nothing changes without repeated, material real-task evidence.
```

## Layer Boundaries

### Python (vgui_runner / scripts)

Owns:
- Scenario DSL and validation
- Router (pure decision, no execution)
- Policy and risk classification
- VerifierResult orchestration
- Evidence manifest assembly
- Recovery strategy (bounded, risk-aware)
- Experience compiler and retrieval

Must NOT own:
- Atomic X11 actions
- Window identity verification at execution time
- Transport/SSH connection management
- Direct xdotool/ssh shell calls in live executor

### Rust (src/)

Owns:
- Atomic GUI actions (click, type, key, drag, screenshot, scroll)
- X11 execution boundary
- Server-side window identity re-validation
- TCP bridge transport (STX/NAK protocol)
- SSH tunnel and ControlMaster management
- Session registry and port assignment
- SKILL string escaping (`escape_skill_string`)

Must NOT own:
- Scenario/policy/retry logic
- Recovery strategy decisions
- Experience compilation
- Arbitrary shell command execution

### SKILL (resources/*.il)

Owns:
- Daemon bootstrap and lifecycle
- IPC communication with Rust daemon
- Virtuoso API calls

Must NOT own:
- GUI form creation (must go through CIW)
- X11 window manipulation

## Critical Invariants

### 1. `VirtuosoResult` two-layer check

```rust
r.ok()        // transport layer only (STX frame received)
r.skill_ok()  // transport + SKILL returned non-nil  ← ALWAYS use this
```

SKILL failures return `nil` over a successful STX frame. Never use `ok()` for SKILL checks.

### 2. Error propagation

Use `VirtuosoError`, not `anyhow`. Variants: `Connection`, `Execution`, `Ssh`, `Io`, `Json`, `Timeout`, `Config`, `NotFound`, `Conflict`.

Validate only at system boundaries (user input, file I/O, external commands). Trust types internally.

### 3. Security

- All user input entering SKILL strings → `bridge::escape_skill_string()`
- External commands → `Command::new()` + separate arguments, no shell concatenation
- Never commit credentials, license paths, fab process data, or PDK model files

### 4. Python/Rust responsibility split

Never duplicate drag/lock/screenshot semantics between Python and Rust. If a behavior exists in Rust atomic executor, Python must not re-implement it — only orchestrate.

### 5. Router is pure function

Router returns `RouteDecision`, never executes. Executor consumes `RouteDecision`. Recovery fallback triggers new route decision with new `route_event_seq`.

### 6. VerifierResult semantics

```
passed     = postcondition verified
failed     = verification executed, state mismatch
skipped    = policy explicitly skipped
unavailable = verification channel unavailable, NOT success
```

`unavailable` ≠ `passed`. Model confidence ≠ execution evidence.

### 7. Recovery safety

```
read_only          → can retry (bounded)
idempotent_write   → verify state before retry
non_idempotent     → NO auto-replay, manual intervention
destructive        → manual only
```

Non-idempotent operations must never be automatically replayed, even on timeout or response loss.

### 8. Evidence closure

Every run must produce:
- `trace.jsonl` (append-only events)
- `manifest.json` (run index, status, artifact hashes)
- `summary.json` (final status, error code, phase)

Failure paths must also produce complete evidence. No silent continuation.

## Change Gate

Before any architecture-affecting change:

1. **Query knowledge graph** (if `.ua/` available): identify affected modules and dependency edges.
2. **Read only relevant source files** — don't grep the entire repo.
3. **Identify which invariant(s) are touched** — list them explicitly.
4. **Minimal change proposal** — smallest possible modification.
5. **Regression** — `cargo test && cargo clippy -- -D warnings && cargo fmt --check` + Python skill tests.
6. **Evidence comparison** — before vs after behavior.
7. **No improvement → reject.**

## Module Map

```
src/
  main.rs            # clap entry, command dispatch
  lib.rs             # shared library root
  error.rs           # VirtuosoError + exit codes
  config.rs          # Config::from_env()
  models.rs          # SessionInfo, JobInfo
  history.rs         # per-session SKILL log
  output.rs          # JSON output helpers
  commands/          # one file per subcommand
  client/
    bridge.rs        # TCP bridge, STX/NAK, escape_skill_string()
    maestro_ops.rs   # SKILL string builders
    window_ops.rs    # SKILL string builders
    skill_sexp.rs    # S-expression parser
  daemon/            # virtuoso-daemon (feature-gated)
  transport/         # SSH tunnel, ControlMaster, native SSH
  spectre/           # standalone Spectre netlist/PSF parsing
  ocean/             # Ocean expression evaluator

.claude/skills/virtuoso-gui-debug/
  scripts/
    vgui_runner/
      engine.py          # state machine: PRECHECK→BASELINE→EXECUTE→VERIFY→RECOVER
      router.py          # pure decision: ActionRequest→RouteDecision
      verifier_result.py # unified VerifierResult model
      evidence.py        # Evidence manifest
      recovery.py        # bounded recovery policy
      planner.py         # template-based planner
      experience.py      # Experience compiler (events→cases)
      experience_retrieval.py  # FTS5 + metadata retrieval
      live_executor.py   # real vcli executor
      local_executor.py  # xdotool executor
      trace.py           # append-only JSONL
```

## References

- Architecture v2.1: `docs/architecture_v2_1.html`
- Evolution Playbook: `docs/evolution_playbook.md`
- GUI Debug Skill: `.claude/skills/virtuoso-gui-debug/SKILL.md`
